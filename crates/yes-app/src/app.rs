use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use chrono::{Local, TimeZone as _};
use gpui_kit::base::{Button as BaseButton, Selectable as _, StyledExt};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Theme, ThemeMode,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_resizable,
    menu::{DropdownMenu as _, PopupMenuItem},
    message_scroller::MessageScrollerState,
    resizable_panel,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use yes_core::mermaid::{ContentSegment, split_mermaid_blocks};
use yes_core::{
    AccentColor, AppSettings, AppType, ChatLayout, Language, PreferredTerminal, ProviderRegistry,
    Session, SessionDetail, SettingsStore, ThemePreference,
    terminal::{TerminalInfo, resume_session, terminal_info},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Experience,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionViewMode {
    Date,
    Directory,
}

#[derive(Clone, PartialEq, Eq)]
struct SourceFileSignature {
    path: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, PartialEq, Eq)]
struct DetailSourceSignature {
    primary: SourceFileSignature,
    sqlite_wal: Option<SourceFileSignature>,
}

fn source_file_signature(path: PathBuf) -> Option<SourceFileSignature> {
    let metadata = std::fs::metadata(&path).ok()?;
    Some(SourceFileSignature {
        path,
        length: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn detail_source_signature(session: &Session) -> Option<DetailSourceSignature> {
    let primary = source_file_signature(session.file_path.clone())?;
    let sqlite_wal = (session.app_type == AppType::OpenCode).then(|| {
        let mut path = session.file_path.as_os_str().to_os_string();
        path.push("-wal");
        source_file_signature(PathBuf::from(path))
    });
    Some(DetailSourceSignature {
        primary,
        sqlite_wal: sqlite_wal.flatten(),
    })
}

fn ancestor_session_ids(sessions: &[Session], session_id: &str) -> Vec<String> {
    let parent_by_id = sessions
        .iter()
        .map(|session| (session.id.as_str(), session.parent_session_id.as_deref()))
        .collect::<HashMap<_, _>>();
    let mut ancestors = Vec::new();
    let mut visited = HashSet::new();
    let mut current = session_id;
    visited.insert(session_id.to_owned());

    while let Some(Some(parent_id)) = parent_by_id.get(current) {
        if !visited.insert((*parent_id).to_owned()) {
            break;
        }
        ancestors.push((*parent_id).to_owned());
        current = parent_id;
    }

    ancestors
}

fn directory_group_labels(path: &str) -> (String, Option<String>) {
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let Some(directory) = parts.last() else {
        return (path.to_owned(), None);
    };
    let parent_parts = &parts[..parts.len() - 1];
    let parent = match parent_parts {
        [] => None,
        [parent] => Some(format!("{parent}/...")),
        [first, second] => Some(format!("/{first}/{second}")),
        parents => Some(format!(
            "../{}/{}...",
            parents[parents.len() - 2],
            parents[parents.len() - 1]
        )),
    };
    ((*directory).to_owned(), parent)
}

fn session_directory_group_key(session: &Session, no_directory_label: &str) -> String {
    session
        .directory
        .as_deref()
        .or_else(|| {
            session
                .file_path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
        })
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| no_directory_label.to_owned())
}

fn navigator_preview(text: &str, max_chars: usize) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return "...".into();
    }
    if clean.chars().count() <= max_chars {
        return clean;
    }
    let mut preview = clean.chars().take(max_chars).collect::<String>();
    while preview.ends_with(char::is_whitespace) {
        preview.pop();
    }
    preview.push_str("...");
    preview
}

fn is_navigable_user_message(message: &yes_core::SessionMessage) -> bool {
    message.message_type == yes_core::MessageType::User
        && message
            .content
            .as_deref()
            .or(message.redacted_content.as_deref())
            .is_some_and(|content| !content.trim().is_empty())
}

fn collect_mermaid_sources(
    messages: &[yes_core::SessionMessage],
    index_offset: usize,
    sources: &mut Vec<((usize, usize), String)>,
) {
    for (message_index, message) in messages.iter().enumerate() {
        let Some(content) = message.content.as_deref() else {
            continue;
        };
        let mut diagram_index = 0;
        for segment in split_mermaid_blocks(content) {
            if let ContentSegment::Mermaid(source) = segment {
                sources.push((
                    (index_offset.saturating_add(message_index), diagram_index),
                    source,
                ));
                diagram_index += 1;
            }
        }
    }
}

fn mermaid_sources_changed(
    previous: &[yes_core::SessionMessage],
    next: &[yes_core::SessionMessage],
) -> bool {
    let mut before = Vec::new();
    let mut after = Vec::new();
    collect_mermaid_sources(previous, 0, &mut before);
    collect_mermaid_sources(next, 0, &mut after);
    before != after
}

fn selection_after_refresh(
    previous: &[Session],
    next: &[Session],
    selected: Option<&str>,
) -> Option<String> {
    if let Some(id) = selected {
        let matches = |session: &&Session| session.id == id || session.uuid.as_deref() == Some(id);
        if let Some(session) = next.iter().find(matches) {
            return Some(session.id.clone());
        }
        // A directly opened subagent may not be enumerated by its provider.
        if !previous
            .iter()
            .any(|session| session.id == id || session.uuid.as_deref() == Some(id))
        {
            return Some(id.to_owned());
        }
    }
    next.iter()
        .find(|session| session.kind == yes_core::model::SessionKind::Main)
        .map(|session| session.id.clone())
}

fn navigator_window(total: usize, active: usize, max_visible: usize) -> Range<usize> {
    if total <= max_visible {
        return 0..total;
    }
    let mut start = active.saturating_sub(max_visible / 2);
    let mut end = (start + max_visible).min(total);
    start = end.saturating_sub(max_visible);
    end = (start + max_visible).min(total);
    start..end
}

fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

#[derive(Clone)]
enum SessionListRow {
    Header {
        key: String,
        label: String,
        collapsed: bool,
    },
    Session {
        session: Box<Session>,
        depth: usize,
        child_count: usize,
    },
}

fn append_session_tree(
    rows: &mut Vec<SessionListRow>,
    session: Session,
    depth: usize,
    children: &HashMap<String, Vec<Session>>,
    expanded: &HashSet<String>,
) {
    let descendants = children.get(&session.id);
    let child_count = descendants.map_or(0, Vec::len);
    let session_id = session.id.clone();
    rows.push(SessionListRow::Session {
        session: Box::new(session),
        depth,
        child_count,
    });
    if expanded.contains(&session_id)
        && let Some(descendants) = descendants
    {
        for child in descendants {
            append_session_tree(rows, child.clone(), depth + 1, children, expanded);
        }
    }
}

use crate::{
    app_assets::ProviderIcon,
    conversation::{
        ConversationOptions, conversation_scroller, conversation_turn_count,
        inline_subagent_scope_base, turn_index_for_message,
    },
    i18n::tr,
    mermaid::{MermaidDiagram, create_mermaid_diagram},
};

pub struct YesSessions {
    registry: Arc<ProviderRegistry>,
    settings_store: SettingsStore,
    pub settings: AppSettings,
    selected_app: AppType,
    sessions: Arc<Vec<Session>>,
    selected_session_id: Option<String>,
    detail: Option<Arc<SessionDetail>>,
    conversation_state: Entity<MessageScrollerState>,
    loading_sessions: bool,
    refreshing_sessions: bool,
    loading_detail: bool,
    error: Option<String>,
    settings_open: bool,
    settings_tab: SettingsTab,
    session_view_mode: SessionViewMode,
    collapsed_groups: HashSet<String>,
    expanded_parents: HashSet<String>,
    expanded_messages: HashSet<usize>,
    expanded_subagent_conversations: HashSet<String>,
    inline_subagent_details: HashMap<String, Arc<SessionDetail>>,
    loading_inline_subagents: HashSet<String>,
    failed_inline_subagents: HashSet<String>,
    marquee_session_id: Option<String>,
    stats_hovered: bool,
    navigator_hovered: bool,
    navigator_active_message: Option<usize>,
    navigator_list_state: ListState,
    mermaid_views: HashMap<(usize, usize), Entity<MermaidDiagram>>,
    terminal_info: TerminalInfo,
    sessions_generation: u64,
    detail_generation: u64,
    detail_source_signature: Option<DetailSourceSignature>,
    refreshing_detail: bool,
}

impl YesSessions {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings_store = SettingsStore::new(SettingsStore::default_path());
        let settings = settings_store.load();
        let selected_app = settings.default_app.unwrap_or(AppType::CodeBuddy);
        let terminal_info = terminal_info(settings.preferred_terminal);
        Self::apply_theme(settings.theme, window, cx);
        Self::apply_accent(settings.accent_color, cx);
        let mut this = Self {
            registry: Arc::new(ProviderRegistry::default()),
            settings_store,
            settings,
            selected_app,
            sessions: Arc::new(Vec::new()),
            selected_session_id: None,
            detail: None,
            conversation_state: cx.new(|cx| MessageScrollerState::new(0, cx)),
            loading_sessions: false,
            refreshing_sessions: false,
            loading_detail: false,
            error: None,
            settings_open: false,
            settings_tab: SettingsTab::General,
            session_view_mode: SessionViewMode::Date,
            collapsed_groups: HashSet::new(),
            expanded_parents: HashSet::new(),
            expanded_messages: HashSet::new(),
            expanded_subagent_conversations: HashSet::new(),
            inline_subagent_details: HashMap::new(),
            loading_inline_subagents: HashSet::new(),
            failed_inline_subagents: HashSet::new(),
            marquee_session_id: None,
            stats_hovered: false,
            navigator_hovered: false,
            navigator_active_message: None,
            navigator_list_state: ListState::new(0, ListAlignment::Top, px(60.))
                .with_uniform_item_height(px(30.)),
            mermaid_views: HashMap::new(),
            terminal_info,
            sessions_generation: 0,
            detail_generation: 0,
            detail_source_signature: None,
            refreshing_detail: false,
        };
        cx.observe_window_appearance(window, |this, window, cx| {
            if this.settings.theme == ThemePreference::System {
                Self::configure_theme(
                    ThemePreference::System,
                    this.settings.accent_color,
                    window,
                    cx,
                );
                this.mermaid_views.clear();
                cx.notify();
            }
        })
        .detach();
        this.load_sessions(cx);
        this.start_live_refresh(cx);
        this
    }

    fn apply_theme(preference: ThemePreference, window: &mut Window, cx: &mut App) {
        let mode = match preference {
            ThemePreference::Light => ThemeMode::Light,
            ThemePreference::Dark => ThemeMode::Dark,
            ThemePreference::System => window.appearance().into(),
        };
        Theme::change(mode, Some(window), cx);
        let theme = Theme::global_mut(cx);
        theme.font_family = ".SystemUIFont".into();
        theme.font_size = px(14.);
        theme.mono_font_family = "Menlo".into();
        theme.mono_font_size = px(12.);
        theme.radius = px(8.);
        theme.radius_lg = px(8.);
        if mode == ThemeMode::Dark {
            let background = hsla(222.2 / 360., 0.84, 0.049, 1.);
            let foreground = hsla(210. / 360., 0.40, 0.98, 1.);
            let surface = hsla(217.2 / 360., 0.326, 0.175, 1.);
            let muted_foreground = hsla(215. / 360., 0.202, 0.651, 1.);
            theme.background = background;
            theme.foreground = foreground;
            theme.popover = background;
            theme.popover_foreground = foreground;
            theme.sidebar = background;
            theme.sidebar_foreground = foreground;
            theme.border = surface;
            theme.input = surface;
            theme.muted = surface;
            theme.secondary = surface;
            theme.accent = surface;
            theme.colors.list = background;
            theme.list_head = background;
            theme.list_hover = surface.opacity(0.5);
            theme.sidebar_border = surface;
            theme.muted_foreground = muted_foreground;
            theme.secondary_foreground = foreground;
            theme.accent_foreground = foreground;
        } else {
            let background = hsla(0., 0., 1., 1.);
            let foreground = hsla(222.2 / 360., 0.84, 0.049, 1.);
            let surface = hsla(210. / 360., 0.40, 0.961, 1.);
            let border = hsla(214.3 / 360., 31.8 / 100., 0.914, 1.);
            let muted_foreground = hsla(215.4 / 360., 0.163, 0.469, 1.);
            theme.background = background;
            theme.foreground = foreground;
            theme.popover = background;
            theme.popover_foreground = foreground;
            theme.sidebar = background;
            theme.sidebar_foreground = foreground;
            theme.border = border;
            theme.input = border;
            theme.muted = surface;
            theme.secondary = surface;
            theme.accent = surface;
            theme.colors.list = background;
            theme.list_head = background;
            theme.list_hover = surface.opacity(0.5);
            theme.sidebar_border = border;
            theme.muted_foreground = muted_foreground;
            theme.secondary_foreground = foreground;
            theme.accent_foreground = foreground;
        }
        Theme::sync_base(cx);
    }

    fn apply_accent(accent: AccentColor, cx: &mut App) {
        let dark = Theme::global(cx).mode == ThemeMode::Dark;
        let (hue, saturation, lightness) = match accent {
            AccentColor::Default if dark => (210., 0.40, 0.98),
            AccentColor::Default => (222.2, 0.474, 0.112),
            AccentColor::Pink => (330., if dark { 0.80 } else { 0.81 }, 0.60),
            AccentColor::Rose => (346., if dark { 0.80 } else { 0.84 }, 0.60),
            AccentColor::Red => (0., if dark { 0.80 } else { 0.84 }, 0.60),
            AccentColor::Orange => (
                24.,
                if dark { 0.90 } else { 0.95 },
                if dark { 0.55 } else { 0.53 },
            ),
            AccentColor::Amber => (
                38.,
                if dark { 0.90 } else { 0.92 },
                if dark { 0.55 } else { 0.50 },
            ),
            AccentColor::Yellow => (
                48.,
                if dark { 0.90 } else { 0.96 },
                if dark { 0.55 } else { 0.53 },
            ),
            AccentColor::Lime => (
                84.,
                if dark { 0.80 } else { 0.81 },
                if dark { 0.50 } else { 0.44 },
            ),
            AccentColor::Green => (
                142.,
                if dark { 0.70 } else { 0.71 },
                if dark { 0.50 } else { 0.45 },
            ),
            AccentColor::Emerald => (
                160.,
                if dark { 0.80 } else { 0.84 },
                if dark { 0.45 } else { 0.39 },
            ),
            AccentColor::Teal => (
                168.,
                if dark { 0.70 } else { 0.76 },
                if dark { 0.45 } else { 0.42 },
            ),
            AccentColor::Cyan => (
                189.,
                if dark { 0.90 } else { 0.94 },
                if dark { 0.48 } else { 0.43 },
            ),
            AccentColor::Sky => (
                199.,
                if dark { 0.85 } else { 0.89 },
                if dark { 0.55 } else { 0.48 },
            ),
            AccentColor::Blue => (217., if dark { 0.85 } else { 0.91 }, 0.60),
            AccentColor::Indigo => (
                239.,
                if dark { 0.80 } else { 0.84 },
                if dark { 0.65 } else { 0.67 },
            ),
            AccentColor::Violet => (
                258.,
                if dark { 0.85 } else { 0.90 },
                if dark { 0.65 } else { 0.66 },
            ),
            AccentColor::Purple => (
                270.,
                if dark { 0.65 } else { 0.67 },
                if dark { 0.60 } else { 0.57 },
            ),
            AccentColor::Fuchsia => (
                292.,
                0.80_f32.max(if dark { 0.80 } else { 0.84 }),
                if dark { 0.60 } else { 0.61 },
            ),
            AccentColor::Slate => (
                215.,
                if dark { 0.20 } else { 0.25 },
                if dark { 0.55 } else { 0.47 },
            ),
            AccentColor::Zinc => (240., 0.05, if dark { 0.55 } else { 0.46 }),
            AccentColor::Neutral => (0., 0., if dark { 0.55 } else { 0.45 }),
        };
        let color = hsla(hue / 360., saturation, lightness, 1.);
        let foreground = if dark && accent == AccentColor::Default {
            hsla(222.2 / 360., 0.474, 0.112, 1.)
        } else if dark && matches!(accent, AccentColor::Yellow | AccentColor::Lime) {
            gpui_kit::black()
        } else {
            hsla(210. / 360., 0.40, 0.98, 1.)
        };
        let hover = hsla(
            hue / 360.,
            saturation,
            if dark {
                (lightness + 0.08).min(0.90)
            } else {
                (lightness - 0.08).max(0.10)
            },
            1.,
        );
        let active = hsla(
            hue / 360.,
            saturation,
            if dark {
                (lightness - 0.12).max(0.)
            } else {
                (lightness - 0.06).max(0.)
            },
            1.,
        );
        let light = hsla(
            hue / 360.,
            saturation.min(0.70),
            if dark { 0.20 } else { 0.94 },
            1.,
        );
        let muted = hsla(
            hue / 360.,
            saturation.min(0.40),
            if dark { 0.15 } else { 0.96 },
            1.,
        );
        let border = hsla(
            hue / 360.,
            saturation.min(0.50),
            if dark { 0.30 } else { 0.88 },
            1.,
        );
        let ring = hsla(
            hue / 360.,
            saturation.min(0.60),
            if dark { 0.50 } else { 0.70 },
            1.,
        );
        let theme = Theme::global_mut(cx);
        theme.primary = color;
        theme.primary_foreground = foreground;
        theme.primary_hover = hover;
        theme.primary_active = active;
        theme.ring = ring;
        theme.selection = color.opacity(if dark { 0.32 } else { 0.22 });
        theme.list_active = light;
        theme.list_active_border = border;
        theme.sidebar_primary = color;
        theme.sidebar_primary_foreground = foreground;
        theme.sidebar_accent = light;
        theme.sidebar_accent_foreground = theme.foreground;
        theme.link = color;
        theme.link_hover = hover;
        theme.link_active = active;
        theme.button = muted;
        theme.button_foreground = theme.foreground;
        theme.button_hover = light;
        theme.button_active = border;
        theme.button_primary = color;
        theme.button_primary_foreground = foreground;
        theme.button_primary_hover = hover;
        theme.button_primary_active = active;
        theme.button_secondary = muted;
        theme.button_secondary_foreground = theme.foreground;
        theme.button_secondary_hover = light;
        theme.button_secondary_active = border;
        Theme::sync_base(cx);
    }

    pub fn configure_theme(
        preference: ThemePreference,
        accent: AccentColor,
        window: &mut Window,
        cx: &mut App,
    ) {
        Self::apply_theme(preference, window, cx);
        Self::apply_accent(accent, cx);
    }

    fn save_settings(&mut self) {
        if let Err(error) = self.settings_store.save(&self.settings) {
            self.error = Some(error.to_string())
        }
    }

    fn load_sessions(&mut self, cx: &mut Context<Self>) {
        self.sessions_generation += 1;
        self.detail_generation += 1;
        let generation = self.sessions_generation;
        self.loading_sessions = true;
        self.refreshing_sessions = false;
        self.loading_detail = false;
        self.refreshing_detail = false;
        self.detail_source_signature = None;
        self.error = None;
        self.sessions = Arc::new(Vec::new());
        self.detail = None;
        self.mermaid_views.clear();
        self.expanded_subagent_conversations.clear();
        self.inline_subagent_details.clear();
        self.loading_inline_subagents.clear();
        self.failed_inline_subagents.clear();
        self.selected_session_id = None;
        self.conversation_state
            .update(cx, |state, cx| state.reset(0, cx));
        let Some(provider) = self.registry.get(self.selected_app) else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { provider.sessions().map_err(|error| error.to_string()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.sessions_generation != generation {
                    return;
                }
                this.loading_sessions = false;
                match result {
                    Ok(sessions) => {
                        this.sessions = Arc::new(sessions);
                        if let Some(first) = this
                            .sessions
                            .iter()
                            .find(|session| session.kind == yes_core::model::SessionKind::Main)
                            .cloned()
                        {
                            this.select_session(first.id, cx);
                        }
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        if self.loading_sessions || self.refreshing_sessions {
            return;
        }
        let Some(provider) = self.registry.get(self.selected_app) else {
            return;
        };
        self.refreshing_sessions = true;
        let generation = self.sessions_generation;
        let task = cx
            .background_executor()
            .spawn(async move { provider.sessions() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                // A provider switch invalidates all requests for the previous list.
                if this.sessions_generation != generation {
                    return;
                }
                this.refreshing_sessions = false;
                let Ok(sessions) = result else { return };
                let selection = selection_after_refresh(
                    &this.sessions,
                    &sessions,
                    this.selected_session_id.as_deref(),
                );
                let changed = this.sessions.as_ref() != &sessions;
                if changed {
                    this.sessions = Arc::new(sessions);
                }
                if selection != this.selected_session_id {
                    if let Some(id) = selection {
                        this.select_session(id, cx);
                    } else {
                        this.reset_detail(cx);
                    }
                    cx.notify();
                } else if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn select_app(&mut self, app_type: AppType, cx: &mut Context<Self>) {
        if self.selected_app == app_type {
            return;
        }
        self.selected_app = app_type;
        self.load_sessions(cx);
    }

    fn reset_detail(&mut self, cx: &mut Context<Self>) {
        self.detail_generation += 1;
        self.selected_session_id = None;
        self.error = None;
        self.loading_detail = false;
        self.refreshing_detail = false;
        self.detail_source_signature = None;
        self.detail = None;
        self.expanded_messages.clear();
        self.expanded_subagent_conversations.clear();
        self.inline_subagent_details.clear();
        self.loading_inline_subagents.clear();
        self.failed_inline_subagents.clear();
        self.navigator_hovered = false;
        self.navigator_active_message = None;
        self.navigator_list_state
            .reset_with_uniform_height(0, px(30.));
        self.mermaid_views.clear();
        self.conversation_state
            .update(cx, |state, cx| state.reset(0, cx));
    }

    fn select_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        let ancestors = ancestor_session_ids(&self.sessions, &session_id);
        for parent_id in &ancestors {
            self.expanded_parents.insert(parent_id.clone());
        }
        if let Some(root_id) = ancestors.last()
            && let Some(root) = self.sessions.iter().find(|session| session.id == *root_id)
        {
            self.collapsed_groups.remove(&self.session_group_key(root));
        }
        self.reset_detail(cx);
        self.selected_session_id = Some(session_id.clone());
        self.loading_detail = true;
        let generation = self.detail_generation;
        let Some(provider) = self.registry.get(self.selected_app) else {
            return;
        };
        let source_signature = self
            .sessions
            .iter()
            .find(|session| {
                session.id == session_id || session.uuid.as_deref() == Some(&session_id)
            })
            .and_then(detail_source_signature);
        let task = cx.background_executor().spawn(async move {
            (
                source_signature,
                provider
                    .session_detail(&session_id)
                    .map_err(|error| error.to_string()),
            )
        });
        cx.spawn(async move |this, cx| {
            let (source_signature, result) = task.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.detail_generation != generation {
                    return;
                }
                this.loading_detail = false;
                match result {
                    Ok(Some(detail)) => {
                        this.detail_source_signature = source_signature;
                        let count = conversation_turn_count(&detail.messages, this.selected_app);
                        let navigator_count = detail
                            .messages
                            .iter()
                            .filter(|message| is_navigable_user_message(message))
                            .count();
                        this.navigator_list_state
                            .reset_with_uniform_height(navigator_count, px(30.));
                        this.navigator_active_message =
                            detail.messages.iter().position(is_navigable_user_message);
                        this.detail = Some(Arc::new(detail));
                        this.conversation_state.update(cx, |state, cx| {
                            state.reset(count, cx);
                            if count > 0 {
                                let _ = state.scroll_to_item(0, cx);
                            }
                        });
                    }
                    Ok(None) => this.error = Some("Session was not found".into()),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_selected_detail(&mut self, cx: &mut Context<Self>) {
        if self.loading_detail || self.refreshing_detail {
            return;
        }
        let Some(session_id) = self.selected_session_id.clone() else {
            return;
        };
        let current_signature = self
            .sessions
            .iter()
            .find(|session| {
                session.id == session_id || session.uuid.as_deref() == Some(&session_id)
            })
            .and_then(detail_source_signature);
        if current_signature.is_some() && current_signature == self.detail_source_signature {
            return;
        }
        let Some(provider) = self.registry.get(self.selected_app) else {
            return;
        };
        self.refreshing_detail = true;
        let generation = self.detail_generation;
        let task = cx.background_executor().spawn(async move {
            (
                current_signature,
                provider
                    .session_detail(&session_id)
                    .map_err(|error| error.to_string()),
            )
        });
        cx.spawn(async move |this, cx| {
            let (source_signature, result) = task.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.detail_generation != generation {
                    return;
                }
                this.refreshing_detail = false;
                if let Ok(Some(detail)) = result {
                    this.detail_source_signature = source_signature;
                    let previous_count = this
                        .detail
                        .as_ref()
                        .map(|detail| conversation_turn_count(&detail.messages, this.selected_app))
                        .unwrap_or_default();
                    let next_count = conversation_turn_count(&detail.messages, this.selected_app);
                    if this.detail.as_deref() != Some(&detail) {
                        let navigator_count = detail
                            .messages
                            .iter()
                            .filter(|message| is_navigable_user_message(message))
                            .count();
                        if navigator_count != this.navigator_list_state.item_count() {
                            this.navigator_list_state
                                .reset_with_uniform_height(navigator_count, px(30.));
                        }
                        if this.detail.as_ref().is_some_and(|previous| {
                            mermaid_sources_changed(&previous.messages, &detail.messages)
                        }) {
                            this.mermaid_views.clear();
                        }
                        this.detail = Some(Arc::new(detail));
                        this.conversation_state.update(cx, |state, cx| {
                            if next_count > previous_count {
                                state.append(next_count - previous_count, cx);
                            } else if next_count == previous_count {
                                state.remeasure(cx);
                            } else {
                                state.reset(next_count, cx);
                            }
                        });
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn start_live_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this
                    .update(cx, |this, cx| {
                        this.refresh_sessions(cx);
                        this.refresh_selected_detail(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn open_sub_agent(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let id = self
            .sessions
            .iter()
            .find(|session| session.id == session_id || session.uuid.as_deref() == Some(session_id))
            .map(|session| session.id.clone())
            .unwrap_or_else(|| session_id.to_owned());
        self.select_session(id, cx);
    }

    pub fn toggle_message(
        &mut self,
        message_index: usize,
        turn_index: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.expanded_messages.remove(&message_index) {
            self.expanded_messages.insert(message_index);
        }
        self.conversation_state.update(cx, |state, cx| {
            let _ = state.remeasure_items(turn_index..turn_index + 1, cx);
        });
        cx.notify();
    }

    pub fn toggle_inline_sub_agent(
        &mut self,
        session_id: String,
        turn_index: usize,
        cx: &mut Context<Self>,
    ) {
        if !self
            .expanded_subagent_conversations
            .insert(session_id.clone())
        {
            self.expanded_subagent_conversations.remove(&session_id);
            self.conversation_state.update(cx, |state, cx| {
                let _ = state.remeasure_items(turn_index..turn_index + 1, cx);
            });
            cx.notify();
            return;
        }

        self.conversation_state.update(cx, |state, cx| {
            let _ = state.remeasure_items(turn_index..turn_index + 1, cx);
        });
        cx.notify();

        if self.inline_subagent_details.contains_key(&session_id)
            || !self.loading_inline_subagents.insert(session_id.clone())
        {
            return;
        }
        self.failed_inline_subagents.remove(&session_id);
        let Some(provider) = self.registry.get(self.selected_app) else {
            self.loading_inline_subagents.remove(&session_id);
            self.failed_inline_subagents.insert(session_id);
            return;
        };
        let generation = self.detail_generation;
        let requested_session_id = session_id.clone();
        let task = cx.background_executor().spawn(async move {
            provider
                .session_detail(&requested_session_id)
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.detail_generation != generation {
                    return;
                }
                this.loading_inline_subagents.remove(&session_id);
                match result {
                    Ok(Some(detail)) => {
                        this.failed_inline_subagents.remove(&session_id);
                        this.inline_subagent_details
                            .insert(session_id.clone(), Arc::new(detail));
                    }
                    Ok(None) | Err(_) => {
                        this.failed_inline_subagents.insert(session_id.clone());
                    }
                }
                this.conversation_state.update(cx, |state, cx| {
                    let _ = state.remeasure_items(turn_index..turn_index + 1, cx);
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        cx.notify();
    }

    fn set_accent(&mut self, accent: AccentColor, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.accent_color = accent;
        Self::apply_theme(self.settings.theme, window, cx);
        Self::apply_accent(accent, cx);
        self.save_settings();
        cx.notify();
    }

    fn set_default_app(&mut self, app_type: AppType, cx: &mut Context<Self>) {
        self.settings.default_app = Some(app_type);
        self.save_settings();
        cx.notify();
    }

    fn set_terminal(&mut self, terminal: PreferredTerminal, cx: &mut Context<Self>) {
        self.settings.preferred_terminal = terminal;
        self.save_settings();
        cx.notify();
    }

    fn set_chat_layout(&mut self, layout: ChatLayout, cx: &mut Context<Self>) {
        self.settings.chat_layout = layout;
        self.save_settings();
        self.conversation_state
            .update(cx, |state, cx| state.remeasure(cx));
        cx.notify();
    }

    fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.settings.language = language;
        self.mermaid_views.clear();
        self.save_settings();
        cx.notify();
    }

    fn set_theme(&mut self, theme: ThemePreference, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.theme = theme;
        Self::apply_theme(theme, window, cx);
        Self::apply_accent(self.settings.accent_color, cx);
        self.mermaid_views.clear();
        self.save_settings();
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.settings.language;
        div()
            .h(px(40.))
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .pl(px(76.))
            .pr_4()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.5))
            .bg(cx.theme().background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        Button::new("toggle-sidebar")
                            .ghost()
                            .compact()
                            .size(px(32.))
                            .icon(IconName::PanelLeft)
                            .tooltip(if self.settings.sidebar_collapsed {
                                tr(language, "app.expandSidebar")
                            } else {
                                tr(language, "app.collapseSidebar")
                            })
                            .accessibility_label(if self.settings.sidebar_collapsed {
                                tr(language, "app.expandSidebar")
                            } else {
                                tr(language, "app.collapseSidebar")
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.sidebar_collapsed = !this.settings.sidebar_collapsed;
                                this.save_settings();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr(language, "app.title")),
                    )
                    .when(self.settings.sidebar_collapsed, |view| {
                        view.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(14.))
                                .text_color(cx.theme().muted_foreground)
                                .child("-")
                                .child(
                                    Icon::new(ProviderIcon::from(self.selected_app)).size(px(18.)),
                                )
                                .child(self.selected_app.display_name()),
                        )
                    }),
            )
            .child(
                Button::new("settings")
                    .ghost()
                    .compact()
                    .size(px(36.))
                    .icon(IconName::Settings)
                    .tooltip(tr(language, "app.settings"))
                    .accessibility_label(tr(language, "app.settings"))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx))),
            )
    }

    fn render_provider_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.weak_entity();
        let selected_app = self.selected_app;
        Button::new("provider-selector")
            .outline()
            .w_full()
            .h(px(36.))
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .icon(ProviderIcon::from(self.selected_app))
            .label(self.selected_app.display_name())
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                AppType::ALL
                    .into_iter()
                    .fold(menu.min_w(px(288.)), |menu, app_type| {
                        let owner = owner.clone();
                        let selected = app_type == selected_app;
                        menu.item(
                            PopupMenuItem::element(move |_, cx| {
                                div()
                                    .w_full()
                                    .h(px(32.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .rounded_md()
                                    .when(selected, |view| {
                                        view.bg(cx.theme().accent)
                                            .text_color(cx.theme().accent_foreground)
                                    })
                                    .text_size(px(14.))
                                    .child(if selected {
                                        Icon::new(IconName::Check)
                                            .size(px(16.))
                                            .text_color(cx.theme().primary)
                                            .into_any_element()
                                    } else {
                                        div().size(px(16.)).into_any_element()
                                    })
                                    .child(Icon::new(ProviderIcon::from(app_type)).size(px(16.)))
                                    .child(app_type.display_name())
                            })
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |this, cx| this.select_app(app_type, cx));
                            }),
                        )
                    })
            })
    }

    fn session_group_key(&self, session: &Session) -> String {
        match self.session_view_mode {
            SessionViewMode::Date => Local
                .timestamp_millis_opt(session.updated_at)
                .single()
                .map(|value| value.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "unknown".into()),
            SessionViewMode::Directory => session_directory_group_key(
                session,
                tr(self.settings.language, "sessions.noDirectory"),
            ),
        }
    }

    fn session_rows(&self) -> Vec<SessionListRow> {
        let mut children = HashMap::<String, Vec<Session>>::new();
        for session in self
            .sessions
            .iter()
            .filter(|session| session.kind == yes_core::model::SessionKind::Subagent)
        {
            if let Some(parent_id) = &session.parent_session_id {
                children
                    .entry(parent_id.clone())
                    .or_default()
                    .push(session.clone());
            }
        }
        for values in children.values_mut() {
            values.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        }
        let mut sessions = self
            .sessions
            .iter()
            .filter(|session| session.kind == yes_core::model::SessionKind::Main)
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut groups: Vec<(String, Vec<Session>)> = Vec::new();
        for session in sessions {
            let key = self.session_group_key(&session);
            if let Some((_, values)) = groups.iter_mut().find(|(group, _)| *group == key) {
                values.push(session);
            } else {
                groups.push((key, vec![session]));
            }
        }
        let today = Local::now().format("%Y-%m-%d").to_string();
        let yesterday = (Local::now() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let mut rows = Vec::new();
        for (key, sessions) in groups {
            let label = match self.session_view_mode {
                SessionViewMode::Date if key == today => {
                    tr(self.settings.language, "sessions.today").into()
                }
                SessionViewMode::Date if key == yesterday => {
                    tr(self.settings.language, "sessions.yesterday").into()
                }
                _ => key.clone(),
            };
            let collapsed = self.collapsed_groups.contains(&key);
            rows.push(SessionListRow::Header {
                key,
                label,
                collapsed,
            });
            if !collapsed {
                for session in sessions {
                    append_session_tree(&mut rows, session, 0, &children, &self.expanded_parents);
                }
            }
        }
        rows
    }

    fn toggle_group(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.collapsed_groups.remove(key) {
            self.collapsed_groups.insert(key.to_owned());
        }
        cx.notify();
    }

    fn expand_all_groups(&mut self, cx: &mut Context<Self>) {
        self.collapsed_groups.clear();
        cx.notify();
    }

    fn collapse_all_groups(&mut self, cx: &mut Context<Self>) {
        self.collapsed_groups = self
            .sessions
            .iter()
            .filter(|session| session.kind == yes_core::model::SessionKind::Main)
            .map(|session| self.session_group_key(session))
            .collect();
        cx.notify();
    }

    fn toggle_parent(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded_parents.remove(id) {
            self.expanded_parents.insert(id.to_owned());
        }
        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.language;
        let sessions = self.sessions.clone();
        let selected = self.selected_session_id.clone();
        let loading = self.loading_sessions;
        let main_count = sessions
            .iter()
            .filter(|session| session.kind == yes_core::model::SessionKind::Main)
            .count();
        let sub_agent_count = sessions.len().saturating_sub(main_count);
        let message_count = sessions
            .iter()
            .map(|session| session.message_count)
            .sum::<usize>();
        let stats_summary = match language {
            Language::Zh => format!(
                "会话: {}  ·  子代理: {}  ·  消息: {}",
                format_count(main_count),
                format_count(sub_agent_count),
                format_count(message_count)
            ),
            Language::En => format!(
                "Sessions: {}  ·  Sub-agents: {}  ·  Messages: {}",
                format_count(main_count),
                format_count(sub_agent_count),
                format_count(message_count)
            ),
        };
        let available = self
            .registry
            .get(self.selected_app)
            .is_some_and(|provider| provider.is_available());
        let rows = Arc::new(self.session_rows());
        let view_mode = self.session_view_mode;
        let expanded_parents = self.expanded_parents.clone();
        let marquee_session_id = self.marquee_session_id.clone();
        let marquee_enabled = self.settings.enable_title_marquee;
        let group_keys = sessions
            .iter()
            .filter(|session| session.kind == yes_core::model::SessionKind::Main)
            .map(|session| self.session_group_key(session))
            .collect::<HashSet<_>>();
        let all_expanded = group_keys
            .iter()
            .all(|key| !self.collapsed_groups.contains(key));
        let all_collapsed = !group_keys.is_empty()
            && group_keys
                .iter()
                .all(|key| self.collapsed_groups.contains(key));
        div()
            .w_full()
            .h_full()
            .v_flex()
            .min_h_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar.opacity(0.5))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border.opacity(0.4))
                    .bg(cx.theme().background)
                    .child(self.render_provider_selector(cx)),
            )
            .child(
                div()
                    .px_3()
                    .py(px(6.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border.opacity(0.4))
                    .child(
                        div()
                            .id("session-stats-region")
                            .relative()
                            .flex_none()
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                if this.stats_hovered != *hovered {
                                    this.stats_hovered = *hovered;
                                    cx.notify();
                                }
                            }))
                            .child(
                                Button::new("session-stats")
                                    .ghost()
                                    .compact()
                                    .size(px(28.))
                                    .icon(IconName::Info)
                                    .accessibility_label(tr(language, "sessions.statistics")),
                            )
                            .when(self.stats_hovered, |view| {
                                view.child(deferred(
                                    div()
                                        .absolute()
                                        .bottom(px(32.))
                                        .left_0()
                                        .h(px(28.))
                                        .whitespace_nowrap()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().popover)
                                        .text_color(cx.theme().popover_foreground)
                                        .shadow_md()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(stats_summary.clone()),
                                ))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .bg(cx.theme().selection)
                            .p(px(2.))
                            .child(
                                Button::new("group-date")
                                    .custom(
                                        ButtonCustomVariant::new(cx)
                                            .color(
                                                if self.session_view_mode == SessionViewMode::Date {
                                                    cx.theme().background
                                                } else {
                                                    cx.theme().transparent
                                                },
                                            )
                                            .foreground(
                                                if self.session_view_mode == SessionViewMode::Date {
                                                    cx.theme().primary
                                                } else {
                                                    cx.theme().muted_foreground
                                                },
                                            )
                                            .hover(cx.theme().background.opacity(0.72))
                                            .active(cx.theme().background)
                                            .shadow(
                                                self.session_view_mode == SessionViewMode::Date,
                                            ),
                                    )
                                    .compact()
                                    .h(px(24.))
                                    .px_2()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .selected(self.session_view_mode == SessionViewMode::Date)
                                    .label(tr(language, "sessions.byDate"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.session_view_mode = SessionViewMode::Date;
                                        this.collapsed_groups.clear();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("group-directory")
                                    .custom(
                                        ButtonCustomVariant::new(cx)
                                            .color(
                                                if self.session_view_mode
                                                    == SessionViewMode::Directory
                                                {
                                                    cx.theme().background
                                                } else {
                                                    cx.theme().transparent
                                                },
                                            )
                                            .foreground(
                                                if self.session_view_mode
                                                    == SessionViewMode::Directory
                                                {
                                                    cx.theme().primary
                                                } else {
                                                    cx.theme().muted_foreground
                                                },
                                            )
                                            .hover(cx.theme().background.opacity(0.72))
                                            .active(cx.theme().background)
                                            .shadow(
                                                self.session_view_mode
                                                    == SessionViewMode::Directory,
                                            ),
                                    )
                                    .compact()
                                    .h(px(24.))
                                    .px_2()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .selected(self.session_view_mode == SessionViewMode::Directory)
                                    .label(tr(language, "sessions.byDirectory"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.session_view_mode = SessionViewMode::Directory;
                                        this.collapsed_groups.clear();
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(if loading {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.))
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(language, "sessions.loading"))
                    .into_any_element()
            } else if !available {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.))
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} {}",
                        self.selected_app.display_name(),
                        tr(language, "sessions.notInstalled")
                    ))
                    .into_any_element()
            } else if main_count == 0 {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.))
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(language, "sessions.empty"))
                    .into_any_element()
            } else {
                uniform_list(
                    "session-list",
                    rows.len(),
                    cx.processor(move |_this, range: Range<usize>, _window, cx| {
                        range
                            .map(|index| {
                                let SessionListRow::Session {
                                    session,
                                    depth,
                                    child_count,
                                } = rows[index].clone()
                                else {
                                    let SessionListRow::Header {
                                        key,
                                        label,
                                        collapsed,
                                    } = rows[index].clone()
                                    else {
                                        unreachable!()
                                    };
                                    let (group_label, parent_path) =
                                        if view_mode == SessionViewMode::Directory {
                                            directory_group_labels(&label)
                                        } else {
                                            (label, None)
                                        };
                                    return div()
                                        .id(("group", index))
                                        .mx_2()
                                        .h(px(36.))
                                        .px_2()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .hover(|style| style.bg(cx.theme().accent.opacity(0.5)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_group(&key, cx)
                                        }))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_1()
                                                .min_w_0()
                                                .items_center()
                                                .gap_2()
                                                .text_size(px(14.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(cx.theme().foreground)
                                                .child(
                                                    Icon::new(if collapsed {
                                                        IconName::ChevronRight
                                                    } else {
                                                        IconName::ChevronDown
                                                    })
                                                    .size(px(16.)),
                                                )
                                                .when(
                                                    view_mode == SessionViewMode::Directory,
                                                    |view| {
                                                        view.child(
                                                            Icon::new(IconName::Folder)
                                                                .size(px(14.))
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                ),
                                                        )
                                                    },
                                                )
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .whitespace_nowrap()
                                                        .child(group_label),
                                                )
                                                .when_some(parent_path, |view, parent_path| {
                                                    view.child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .whitespace_nowrap()
                                                            .text_ellipsis()
                                                            .text_size(px(12.))
                                                            .font_weight(FontWeight::NORMAL)
                                                            .text_color(
                                                                cx.theme()
                                                                    .muted_foreground
                                                                    .opacity(0.6),
                                                            )
                                                            .child(parent_path),
                                                    )
                                                }),
                                        )
                                        .when(index == 0, |view| {
                                            view.child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(2.))
                                                    .child(
                                                        Button::new("expand-all-groups")
                                                            .ghost()
                                                            .compact()
                                                            .size(px(24.))
                                                            .icon(IconName::ChevronDown)
                                                            .disabled(all_expanded)
                                                            .tooltip(tr(
                                                                language,
                                                                "sessions.expandAll",
                                                            ))
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.expand_all_groups(cx)
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("collapse-all-groups")
                                                            .ghost()
                                                            .compact()
                                                            .size(px(24.))
                                                            .icon(IconName::ChevronUp)
                                                            .disabled(all_collapsed)
                                                            .tooltip(tr(
                                                                language,
                                                                "sessions.collapseAll",
                                                            ))
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.collapse_all_groups(cx)
                                                                },
                                                            )),
                                                    ),
                                            )
                                        });
                                };
                                let is_selected = selected.as_deref() == Some(session.id.as_str());
                                let title = if session.first_message.is_empty() {
                                    session.file_name.clone()
                                } else {
                                    session.first_message.clone()
                                };
                                let timestamp = Local
                                    .timestamp_millis_opt(session.updated_at)
                                    .single()
                                    .map(|value| value.format("%m/%d %H:%M").to_string())
                                    .unwrap_or_default();
                                let id = session.id.clone();
                                let hovered_id = session.id.clone();
                                let is_subagent =
                                    session.kind == yes_core::model::SessionKind::Subagent;
                                let children_expanded = expanded_parents.contains(&session.id);
                                let parent_id = session.id.clone();
                                let title_width = title
                                    .chars()
                                    .map(|character| if character.is_ascii() { 6.5 } else { 12. })
                                    .sum::<f32>();
                                let accessibility_title = title.clone();
                                let marquee_distance = (title_width - 190.).max(0.);
                                let should_marquee = marquee_enabled
                                    && marquee_session_id.as_deref() == Some(session.id.as_str())
                                    && marquee_distance > 0.;
                                let title_element = if should_marquee {
                                    let duration = 3.5 + marquee_distance / 45.;
                                    div()
                                        .w(px(title_width.max(1.)))
                                        .flex_none()
                                        .whitespace_nowrap()
                                        .child(title)
                                        .with_animation(
                                            ("session-title-marquee", index),
                                            Animation::new(Duration::from_secs_f32(duration))
                                                .repeat()
                                                .with_easing(|phase| {
                                                    if phase < 0.15 {
                                                        0.
                                                    } else if phase > 0.85 {
                                                        1.
                                                    } else {
                                                        (phase - 0.15) / 0.7
                                                    }
                                                }),
                                            move |element, phase| {
                                                element
                                                    .relative()
                                                    .left(px(-marquee_distance * phase))
                                            },
                                        )
                                        .into_any_element()
                                } else {
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(title)
                                        .into_any_element()
                                };
                                let row = BaseButton::new(("session", index))
                                    .accessibility_label(accessibility_title)
                                    .relative()
                                    .w_full()
                                    .max_w_full()
                                    .overflow_hidden()
                                    .h(px(36.))
                                    .px_2()
                                    .when(child_count > 0, |view| view.pr(px(48.)))
                                    .flex()
                                    .items_center()
                                    .justify_start()
                                    .gap_2()
                                    .rounded(px(4.))
                                    .when(is_subagent, |view| {
                                        view.border_l_1().border_color(hsla(
                                            270. / 360.,
                                            0.67,
                                            0.65,
                                            0.6,
                                        ))
                                    })
                                    .cursor_pointer()
                                    .when(is_selected, |view| {
                                        view.bg(cx.theme().sidebar_accent)
                                            .text_color(cx.theme().primary)
                                            .shadow_sm()
                                    })
                                    .when(!is_selected, |view| {
                                        view.text_color(cx.theme().muted_foreground)
                                            .hover(|style| style.bg(cx.theme().accent.opacity(0.3)))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_session(id.clone(), cx)
                                    }))
                                    .when(marquee_enabled, |view| {
                                        view.on_hover(cx.listener(
                                            move |this, hovered: &bool, _, cx| {
                                                if *hovered {
                                                    this.marquee_session_id =
                                                        Some(hovered_id.clone());
                                                } else if this.marquee_session_id.as_deref()
                                                    == Some(hovered_id.as_str())
                                                {
                                                    this.marquee_session_id = None;
                                                }
                                                cx.notify();
                                            },
                                        ))
                                    })
                                    .when(is_selected, |view| {
                                        view.child(
                                            div()
                                                .absolute()
                                                .left_0()
                                                .top_0()
                                                .bottom_0()
                                                .w(px(2.))
                                                .rounded_r_full()
                                                .bg(cx.theme().primary),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(px(12.))
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .when(is_subagent, |view| {
                                                view.child(
                                                    Icon::new(IconName::Bot)
                                                        .size(px(14.))
                                                        .text_color(hsla(
                                                            270. / 360.,
                                                            0.67,
                                                            0.55,
                                                            1.,
                                                        )),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .child(title_element),
                                            )
                                            .when_some(
                                                session.agent_type.clone(),
                                                |view, agent| {
                                                    view.child(
                                                        div()
                                                            .rounded(px(4.))
                                                            .bg(hsla(270. / 360., 0.67, 0.94, 1.))
                                                            .px(px(6.))
                                                            .py(px(2.))
                                                            .text_size(px(10.))
                                                            .text_color(hsla(
                                                                270. / 360.,
                                                                0.67,
                                                                0.42,
                                                                1.,
                                                            ))
                                                            .child(agent),
                                                    )
                                                },
                                            ),
                                    )
                                    .when(view_mode == SessionViewMode::Directory, |view| {
                                        view.child(
                                            div()
                                                .flex_none()
                                                .rounded(px(4.))
                                                .bg(if is_selected {
                                                    cx.theme().selection
                                                } else {
                                                    cx.theme().muted
                                                })
                                                .px(px(6.))
                                                .py(px(2.))
                                                .text_size(px(10.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(if is_selected {
                                                    cx.theme().primary
                                                } else {
                                                    cx.theme().muted_foreground
                                                })
                                                .child(timestamp),
                                        )
                                    })
                                    .when(child_count > 0, |view| {
                                        view.child(
                                            Button::new(("toggle-children", index))
                                                .ghost()
                                                .compact()
                                                .absolute()
                                                .right_1()
                                                .h(px(24.))
                                                .px(px(6.))
                                                .text_color(hsla(270. / 360., 0.67, 0.62, 1.))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .child(
                                                            Icon::new(if children_expanded {
                                                                IconName::ChevronDown
                                                            } else {
                                                                IconName::ChevronRight
                                                            })
                                                            .size(px(14.)),
                                                        )
                                                        .child(
                                                            Icon::new(IconName::Bot).size(px(12.)),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(10.))
                                                                .child(child_count.to_string()),
                                                        ),
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.toggle_parent(&parent_id, cx)
                                                })),
                                        )
                                    });
                                div()
                                    .id(("session-wrapper", index))
                                    .w_full()
                                    .h(px(36.))
                                    .pl(px(8. + (depth.min(4) * 20) as f32))
                                    .pr_2()
                                    .child(row)
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .h_full()
                .into_any_element()
            })
            .into_any_element()
    }

    fn prepare_mermaid_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for diagram in self.mermaid_views.values() {
            MermaidDiagram::hide(diagram, cx);
        }
        let Some(detail) = &self.detail else { return };
        let dark = cx.theme().mode == ThemeMode::Dark;
        let mut sources = Vec::new();
        collect_mermaid_sources(&detail.messages, 0, &mut sources);
        for inline_detail in self.inline_subagent_details.values() {
            collect_mermaid_sources(
                &inline_detail.messages,
                inline_subagent_scope_base(&inline_detail.session.id),
                &mut sources,
            );
        }
        for (key, source) in sources {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.mermaid_views.entry(key)
            {
                match create_mermaid_diagram(&source, dark, self.settings.language, window, cx) {
                    Ok(diagram) => {
                        entry.insert(diagram);
                    }
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
        }
    }

    fn render_user_navigator(
        &self,
        messages: &[yes_core::SessionMessage],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let user_messages = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| is_navigable_user_message(message))
            .map(|(index, message)| {
                (
                    index,
                    message
                        .content
                        .as_deref()
                        .or(message.redacted_content.as_deref())
                        .unwrap_or_default()
                        .to_owned(),
                    turn_index_for_message(messages, index, self.selected_app),
                )
            })
            .collect::<Vec<_>>();
        if user_messages.len() <= 1 {
            return div().into_any_element();
        }
        let active_position = self
            .navigator_active_message
            .and_then(|active| {
                user_messages
                    .iter()
                    .position(|(message_index, _, _)| *message_index == active)
            })
            .unwrap_or(0);
        let visible_range = navigator_window(user_messages.len(), active_position, 8);
        let visible_messages = user_messages[visible_range.clone()].to_vec();
        let hovered = self.navigator_hovered;
        let expanded_item_count = user_messages.len().min(8) as f32;
        let expanded_height = if user_messages.len() > 8 {
            256.
        } else {
            16. + expanded_item_count * 28. + (expanded_item_count - 1.).max(0.) * 2.
        };
        let collapsed_item_count = visible_messages.len() as f32;
        let collapsed_height =
            12. + collapsed_item_count * 4. + (collapsed_item_count - 1.).max(0.) * 6.;
        let navigator_list_state = self.navigator_list_state.clone();
        let navigator_messages = Arc::new(user_messages);
        let navigator_owner = cx.weak_entity();
        let navigator_conversation_state = self.conversation_state.clone();
        let navigator_scroll_owner = navigator_owner.clone();
        let navigator_scroll_state = navigator_list_state.clone();
        let background = if cx.theme().mode == ThemeMode::Dark {
            cx.theme().background.opacity(0.90)
        } else {
            cx.theme().background.opacity(0.94)
        };
        div()
            .id("user-message-navigator")
            .absolute()
            .right_3()
            .top(relative(0.5))
            .mt(px(if hovered {
                -expanded_height / 2.
            } else {
                -collapsed_height / 2.
            }))
            .v_flex()
            .items_center()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.navigator_hovered != *hovered {
                    this.navigator_hovered = *hovered;
                    cx.notify();
                }
            }))
            .when(!hovered, |view| {
                view.rounded_full()
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.2))
                    .bg(background)
                    .px_2()
                    .py(px(6.))
                    .gap(px(6.))
                    .children(visible_messages.into_iter().enumerate().map(
                        |(visible_position, (message_index, _, turn_index))| {
                            let state = navigator_conversation_state.clone();
                            let actual_position = visible_range.start + visible_position;
                            let active = actual_position == active_position;
                            Button::new(("jump-user", message_index))
                                .text()
                                .compact()
                                .h(px(4.))
                                .w(if active { px(20.) } else { px(10.) })
                                .accessibility_label(format!(
                                    "{} {}",
                                    tr(self.settings.language, "message.user"),
                                    actual_position + 1
                                ))
                                .child(div().h(px(4.)).w_full().rounded_full().bg(if active {
                                    cx.theme().primary
                                } else {
                                    cx.theme().muted_foreground.opacity(0.3)
                                }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.navigator_active_message = Some(message_index);
                                    state.update(cx, |state, cx| {
                                        let _ = state.scroll_to_item(turn_index, cx);
                                    });
                                    cx.notify();
                                }))
                        },
                    ))
            })
            .when(hovered, |view| {
                view.w(px(180.))
                    .h(px(expanded_height))
                    .occlude()
                    .overflow_hidden()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.3))
                    .bg(background)
                    .shadow_lg()
                    .child(
                        div()
                            .relative()
                            .size_full()
                            .overflow_hidden()
                            .child(
                                list(navigator_list_state.clone(), move |position, _, cx| {
                                    let (message_index, content, turn_index) =
                                        navigator_messages[position].clone();
                                    let state = navigator_conversation_state.clone();
                                    let owner = navigator_owner.clone();
                                    let active = position == active_position;
                                    let preview = navigator_preview(&content, 28);
                                    div()
                                        .h(px(30.))
                                        .px(px(6.))
                                        .pb(px(2.))
                                        .child(
                                            Button::new(("jump-user-preview", message_index))
                                                .custom(
                                                    ButtonCustomVariant::new(cx)
                                                        .color(if active {
                                                            cx.theme().primary.opacity(0.2)
                                                        } else {
                                                            cx.theme().transparent
                                                        })
                                                        .foreground(if active {
                                                            cx.theme().primary
                                                        } else {
                                                            cx.theme().muted_foreground
                                                        })
                                                        .hover(
                                                            if cx.theme().mode == ThemeMode::Dark {
                                                                gpui_kit::white().opacity(0.05)
                                                            } else {
                                                                gpui_kit::white().opacity(0.10)
                                                            },
                                                        )
                                                        .active(if active {
                                                            cx.theme().primary.opacity(0.2)
                                                        } else {
                                                            cx.theme().foreground.opacity(0.12)
                                                        }),
                                                )
                                                .compact()
                                                .w_full()
                                                .h(px(28.))
                                                .px(px(10.))
                                                .text_size(px(12.))
                                                .selected(active)
                                                .accessibility_label(preview.clone())
                                                .tooltip(preview.clone())
                                                .child(
                                                    div()
                                                        .w_full()
                                                        .min_w_0()
                                                        .text_left()
                                                        .whitespace_nowrap()
                                                        .text_ellipsis()
                                                        .when(active, |view| {
                                                            view.font_weight(FontWeight::MEDIUM)
                                                        })
                                                        .child(preview),
                                                )
                                                .on_click(move |_, _, cx| {
                                                    let _ = owner.update(cx, |this, cx| {
                                                        this.navigator_active_message =
                                                            Some(message_index);
                                                        state.update(cx, |state, cx| {
                                                            let _ = state
                                                                .scroll_to_item(turn_index, cx);
                                                        });
                                                        cx.notify();
                                                    });
                                                }),
                                        )
                                        .into_any_element()
                                })
                                .size_full()
                                .py_2(),
                            )
                            .child(div().absolute().size_full().on_scroll_wheel(
                                move |event, window, cx| {
                                    let mut offset =
                                        navigator_scroll_state.scroll_px_offset_for_scrollbar();
                                    let max_offset =
                                        navigator_scroll_state.max_offset_for_scrollbar().y;
                                    let delta = event.delta.pixel_delta(window.line_height());
                                    offset.y = (offset.y + delta.y).clamp(-max_offset, px(0.));
                                    navigator_scroll_state.set_offset_from_scrollbar(offset);
                                    let _ = navigator_scroll_owner.update(cx, |_, cx| cx.notify());
                                    cx.stop_propagation();
                                },
                            )),
                    )
            })
            .into_any_element()
    }

    fn render_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.language;
        if self.loading_detail {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(14.))
                .text_color(cx.theme().muted_foreground)
                .child(tr(language, "sessions.loadingDetail"))
                .into_any_element();
        }
        let Some(detail) = self.detail.clone() else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(14.))
                .text_color(cx.theme().muted_foreground)
                .child(tr(language, "sessions.select"))
                .into_any_element();
        };
        let session = &detail.session;
        let updated = Local
            .timestamp_millis_opt(session.updated_at)
            .single()
            .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let resume_session_id = session.id.clone();
        let resume_app = session.app_type;
        let resume_dir = session.directory.clone();
        let terminal = self.settings.preferred_terminal;
        let parent_id = session.parent_session_id.clone();
        let agent_type = session.agent_type.clone();
        let copy_session_id = session.id.clone();
        let title = if session.first_message.is_empty() {
            session.file_name.clone()
        } else {
            session.first_message.clone()
        };
        let title_view = div()
            .min_w_0()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(title);
        let messages = Arc::new(detail.messages.clone());
        if self.settings_open {
            for diagram in self.mermaid_views.values() {
                MermaidDiagram::hide(diagram, cx);
            }
        } else {
            self.prepare_mermaid_views(window, cx);
        }
        let mermaid_views = if self.settings_open {
            Arc::new(HashMap::new())
        } else {
            Arc::new(self.mermaid_views.clone())
        };
        let scroller = conversation_scroller(
            messages,
            self.conversation_state.clone(),
            ConversationOptions {
                language,
                provider: self.selected_app,
                show_thinking: self.settings.show_thinking_content,
                chat_bubbles: self.settings.chat_layout == ChatLayout::Bubble,
                collapse_tool_blocks: self.settings.collapse_bash_blocks,
            },
            self.expanded_messages.clone(),
            self.expanded_subagent_conversations.clone(),
            Arc::new(self.inline_subagent_details.clone()),
            self.loading_inline_subagents.clone(),
            self.failed_inline_subagents.clone(),
            mermaid_views,
            cx.weak_entity(),
        );
        let navigator = self.render_user_navigator(&detail.messages, cx);
        div()
            .flex_1()
            .h_full()
            .min_w_0()
            .v_flex()
            .min_h_0()
            .child(
                div()
                    .p_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .v_flex()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(
                                session.kind == yes_core::model::SessionKind::Subagent,
                                |view| {
                                    view.child(
                                        div()
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .rounded(px(4.))
                                            .bg(hsla(270. / 360., 0.67, 0.94, 1.))
                                            .px_2()
                                            .py_1()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(hsla(270. / 360., 0.67, 0.42, 1.))
                                            .child(Icon::new(IconName::Bot).size(px(14.)))
                                            .child(tr(language, "sessions.subAgent"))
                                            .when_some(agent_type, |view, agent_type| {
                                                view.child(format!(" · {agent_type}"))
                                            }),
                                    )
                                },
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .overflow_hidden()
                                    .child(title_view),
                            ),
                    )
                    .when_some(parent_id, |view, parent_id| {
                        view.child(
                            div().mt_1().w_full().flex().justify_center().child(
                                Button::new("back-to-parent")
                                    .text()
                                    .compact()
                                    .h(px(20.))
                                    .text_size(px(12.))
                                    .icon(IconName::ArrowLeft)
                                    .label(tr(language, "sessions.backToParent"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_sub_agent(&parent_id, cx)
                                    })),
                            ),
                        )
                    })
                    .child(
                        div()
                            .mt_2()
                            .v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(px(10.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(tr(language, "sessions.sessionId")),
                                    )
                                    .child(
                                        Button::new("copy-session-id")
                                            .text()
                                            .compact()
                                            .h(px(20.))
                                            .max_w(relative(0.72))
                                            .text_size(px(12.))
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .label(session.id.clone())
                                            .on_click(move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copy_session_id.clone(),
                                                ));
                                            }),
                                    )
                                    .when(
                                        session.kind != yes_core::model::SessionKind::Subagent,
                                        |view| {
                                            view.child(
                                                Button::new("resume-session")
                                                    .custom(
                                                        ButtonCustomVariant::new(cx)
                                                            .color(cx.theme().foreground)
                                                            .foreground(cx.theme().background)
                                                            .hover(
                                                                cx.theme().foreground.opacity(0.9),
                                                            )
                                                            .active(
                                                                cx.theme().foreground.opacity(0.82),
                                                            ),
                                                    )
                                                    .compact()
                                                    .h(px(24.))
                                                    .px_2()
                                                    .text_size(px(10.))
                                                    .icon(IconName::Play)
                                                    .label(tr(language, "sessions.resume"))
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            if let Err(error) = resume_session(
                                                                resume_app,
                                                                &resume_session_id,
                                                                resume_dir.as_deref(),
                                                                terminal,
                                                            ) {
                                                                this.error =
                                                                    Some(error.to_string());
                                                                cx.notify();
                                                            }
                                                        },
                                                    )),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(tr(language, "sessions.updated")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_color(cx.theme().muted_foreground)
                                            .child(updated),
                                    ),
                            )
                            .when_some(session.directory.clone(), |view, directory| {
                                view.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_size(px(10.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child(tr(language, "sessions.work")),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .text_size(px(12.))
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_color(cx.theme().muted_foreground)
                                                .child(directory.display().to_string()),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(scroller.size_full())
                    .child(navigator),
            )
            .into_any_element()
    }

    fn option_button(
        &self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .outline()
            .compact()
            .selected(selected)
            .label(label)
            .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
    }

    fn tab_button(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        selected: bool,
        tab: SettingsTab,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .text()
            .compact()
            .h(px(40.))
            .rounded(px(0.))
            .border_b_2()
            .border_color(if selected {
                cx.theme().primary
            } else {
                cx.theme().transparent
            })
            .text_color(if selected {
                cx.theme().primary
            } else {
                cx.theme().muted_foreground
            })
            .icon(icon)
            .label(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.settings_tab = tab;
                cx.notify();
            }))
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.language;
        let tab = self.settings_tab;
        let accent_owner = cx.weak_entity();
        let accent_control = Button::new("accent-picker")
            .outline()
            .compact()
            .h(px(32.))
            .icon(IconName::Palette)
            .label(match self.settings.accent_color {
                AccentColor::Default => tr(language, "settings.accentDefault"),
                AccentColor::Blue => tr(language, "settings.accentBlue"),
                AccentColor::Green => tr(language, "settings.accentGreen"),
                AccentColor::Orange => tr(language, "settings.accentOrange"),
                AccentColor::Red => tr(language, "settings.accentRed"),
                AccentColor::Purple => tr(language, "settings.accentPurple"),
                AccentColor::Pink => tr(language, "settings.accentPink"),
                accent => accent.display_name(),
            })
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                AccentColor::ALL.into_iter().fold(menu, |menu, accent| {
                    let owner = accent_owner.clone();
                    menu.item(PopupMenuItem::new(accent.display_name()).on_click(
                        move |_, window, cx| {
                            let _ =
                                owner.update(cx, |this, cx| this.set_accent(accent, window, cx));
                        },
                    ))
                })
            });
        let default_owner = cx.weak_entity();
        let default_control = Button::new("default-provider")
            .outline()
            .compact()
            .w(px(220.))
            .h(px(32.))
            .icon(ProviderIcon::from(
                self.settings.default_app.unwrap_or(AppType::CodeBuddy),
            ))
            .label(
                self.settings
                    .default_app
                    .unwrap_or(AppType::CodeBuddy)
                    .display_name(),
            )
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                AppType::ALL.into_iter().fold(menu, |menu, app_type| {
                    let owner = default_owner.clone();
                    menu.item(
                        PopupMenuItem::new(app_type.display_name())
                            .icon(ProviderIcon::from(app_type))
                            .on_click(move |_, _, cx| {
                                let _ =
                                    owner.update(cx, |this, cx| this.set_default_app(app_type, cx));
                            }),
                    )
                })
            });
        div()
            .absolute()
            .inset_0()
            .bg(gpui_kit::black().opacity(0.8))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w(px(576.))
                    .max_h(relative(0.85))
                    .v_flex()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .shadow_lg()
                    .child(
                        div()
                            .px_6()
                            .pt_6()
                            .pb_2()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tr(language, "settings.title")),
                            )
                            .child(
                                Button::new("close-settings")
                                    .ghost()
                                    .compact()
                                    .absolute()
                                    .top_4()
                                    .right_4()
                                    .size(px(32.))
                                    .icon(IconName::Close)
                                    .tooltip(tr(language, "settings.close"))
                                    .accessibility_label(tr(language, "settings.close"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.toggle_settings(cx)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .h(px(40.))
                            .px_6()
                            .flex()
                            .items_center()
                            .gap_6()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(self.tab_button(
                                "tab-general",
                                IconName::Settings,
                                tr(language, "settings.general"),
                                tab == SettingsTab::General,
                                SettingsTab::General,
                                cx,
                            ))
                            .child(self.tab_button(
                                "tab-experience",
                                IconName::Star,
                                tr(language, "settings.experience"),
                                tab == SettingsTab::Experience,
                                SettingsTab::Experience,
                                cx,
                            ))
                            .child(self.tab_button(
                                "tab-terminal",
                                IconName::SquareTerminal,
                                tr(language, "settings.terminal"),
                                tab == SettingsTab::Terminal,
                                SettingsTab::Terminal,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .px_6()
                            .py_4()
                            .v_flex()
                            .gap_6()
                            .when(tab == SettingsTab::General, |view| {
                                view.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(cx.theme().border.opacity(0.6))
                                        .pb_6()
                                        .child(
                                            div()
                                                .v_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(tr(language, "settings.language")),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(tr(
                                                            language,
                                                            "settings.languageDescription",
                                                        )),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .gap_2()
                                                .child(self.option_button(
                                                    "language-en",
                                                    "English",
                                                    language == Language::En,
                                                    |this, _, cx| {
                                                        this.set_language(Language::En, cx)
                                                    },
                                                    cx,
                                                ))
                                                .child(self.option_button(
                                                    "language-zh",
                                                    "简体中文",
                                                    language == Language::Zh,
                                                    |this, _, cx| {
                                                        this.set_language(Language::Zh, cx)
                                                    },
                                                    cx,
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(cx.theme().border.opacity(0.6))
                                        .pb_6()
                                        .child(
                                            div()
                                                .v_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(tr(language, "settings.theme")),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(tr(
                                                            language,
                                                            "settings.themeDescription",
                                                        )),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .gap_2()
                                                .child(self.option_button(
                                                    "theme-system",
                                                    tr(language, "settings.themeSystem"),
                                                    self.settings.theme == ThemePreference::System,
                                                    |this, window, cx| {
                                                        this.set_theme(
                                                            ThemePreference::System,
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                    cx,
                                                ))
                                                .child(self.option_button(
                                                    "theme-light",
                                                    tr(language, "settings.themeLight"),
                                                    self.settings.theme == ThemePreference::Light,
                                                    |this, window, cx| {
                                                        this.set_theme(
                                                            ThemePreference::Light,
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                    cx,
                                                ))
                                                .child(self.option_button(
                                                    "theme-dark",
                                                    tr(language, "settings.themeDark"),
                                                    self.settings.theme == ThemePreference::Dark,
                                                    |this, window, cx| {
                                                        this.set_theme(
                                                            ThemePreference::Dark,
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                    cx,
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(cx.theme().border.opacity(0.6))
                                        .pb_6()
                                        .child(
                                            div()
                                                .v_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(tr(language, "settings.accent")),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(tr(
                                                            language,
                                                            "settings.accentDescription",
                                                        )),
                                                ),
                                        )
                                        .child(accent_control),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .v_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(tr(language, "settings.defaultApp")),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(tr(
                                                            language,
                                                            "settings.defaultAppDescription",
                                                        )),
                                                ),
                                        )
                                        .child(default_control),
                                )
                            })
                            .when(tab == SettingsTab::Experience, |view| {
                                view.gap_4()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(tr(language, "settings.experienceDescription")),
                                    )
                                    .child(self.boolean_setting(
                                        "title-marquee",
                                        IconName::CaseSensitive,
                                        tr(language, "settings.titleMarquee"),
                                        tr(language, "settings.titleMarqueeDescription"),
                                        self.settings.enable_title_marquee,
                                        |settings| &mut settings.enable_title_marquee,
                                        cx,
                                    ))
                                    .child(self.boolean_setting(
                                        "collapse-bash",
                                        IconName::SquareTerminal,
                                        tr(language, "settings.collapseBash"),
                                        tr(language, "settings.collapseBashDescription"),
                                        self.settings.collapse_bash_blocks,
                                        |settings| &mut settings.collapse_bash_blocks,
                                        cx,
                                    ))
                                    .child(self.boolean_setting(
                                        "show-thinking",
                                        IconName::Asterisk,
                                        tr(language, "settings.showThinking"),
                                        tr(language, "settings.showThinkingDescription"),
                                        self.settings.show_thinking_content,
                                        |settings| &mut settings.show_thinking_content,
                                        cx,
                                    ))
                                    .child(self.chat_layout_setting(cx))
                            })
                            .when(tab == SettingsTab::Terminal, |view| {
                                view.gap_4()
                                    .child(
                                        div()
                                            .v_flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(tr(language, "settings.terminal")),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(tr(
                                                        language,
                                                        "settings.terminalDescription",
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .v_flex()
                                            .gap_2()
                                            .child(self.terminal_setting(
                                                "terminal-auto",
                                                PreferredTerminal::Auto,
                                                IconName::SquareTerminal,
                                                tr(language, "settings.terminalAuto"),
                                                tr(language, "settings.terminalAutoDescription"),
                                                cx,
                                            ))
                                            .when(self.terminal_info.ghostty_installed, |view| {
                                                view.child(self.terminal_setting(
                                                    "terminal-ghostty",
                                                    PreferredTerminal::Ghostty,
                                                    IconName::SquareTerminal,
                                                    "Ghostty",
                                                    tr(
                                                        language,
                                                        "settings.terminalGhosttyDescription",
                                                    ),
                                                    cx,
                                                ))
                                            })
                                            .when(self.terminal_info.kitty_installed, |view| {
                                                view.child(self.terminal_setting(
                                                    "terminal-kitty",
                                                    PreferredTerminal::Kitty,
                                                    IconName::SquareTerminal,
                                                    "Kitty",
                                                    tr(
                                                        language,
                                                        "settings.terminalKittyDescription",
                                                    ),
                                                    cx,
                                                ))
                                            })
                                            .child(self.terminal_setting(
                                                "terminal-macos",
                                                PreferredTerminal::Terminal,
                                                IconName::SquareTerminal,
                                                "Terminal.app",
                                                tr(language, "settings.terminalMacDescription"),
                                                cx,
                                            )),
                                    )
                            }),
                    )
                    .child(
                        div()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .py_3()
                            .text_size(px(10.))
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                    ),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn boolean_setting(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        description: &'static str,
        value: bool,
        field: fn(&mut AppSettings) -> &mut bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        BaseButton::new(id)
            .accessibility_label(label)
            .w_full()
            .flex()
            .items_center()
            .justify_start()
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if value {
                cx.theme().primary.opacity(0.5)
            } else {
                cx.theme().border
            })
            .bg(if value {
                cx.theme().selection
            } else {
                cx.theme().background
            })
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().accent.opacity(0.5)))
            .on_click(cx.listener(move |this, _, _, cx| {
                let current = field(&mut this.settings);
                *current = !*current;
                this.save_settings();
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .size(px(36.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if value {
                        cx.theme().primary
                    } else {
                        cx.theme().muted
                    })
                    .text_color(if value {
                        cx.theme().primary_foreground
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(Icon::new(icon).size(px(16.))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .v_flex()
                    .gap_1()
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(description),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(40.))
                    .h(px(20.))
                    .rounded_full()
                    .bg(if value {
                        cx.theme().primary
                    } else {
                        cx.theme().muted
                    })
                    .child(
                        div()
                            .absolute()
                            .top(px(4.))
                            .left(if value { px(24.) } else { px(4.) })
                            .size(px(12.))
                            .rounded_full()
                            .bg(gpui_kit::white()),
                    ),
            )
    }

    fn chat_layout_setting(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.settings.chat_layout == ChatLayout::Bubble;
        BaseButton::new("chat-layout-setting")
            .accessibility_label(tr(self.settings.language, "settings.chatLayout"))
            .w_full()
            .flex()
            .items_center()
            .justify_start()
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if enabled {
                cx.theme().primary.opacity(0.5)
            } else {
                cx.theme().border
            })
            .bg(if enabled {
                cx.theme().selection
            } else {
                cx.theme().background
            })
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().accent.opacity(0.5)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_chat_layout(
                    if enabled {
                        ChatLayout::Left
                    } else {
                        ChatLayout::Bubble
                    },
                    cx,
                )
            }))
            .child(
                div()
                    .flex_none()
                    .size(px(36.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if enabled {
                        cx.theme().primary
                    } else {
                        cx.theme().muted
                    })
                    .text_color(if enabled {
                        cx.theme().primary_foreground
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(Icon::new(IconName::LayoutDashboard).size(px(16.))),
            )
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(tr(self.settings.language, "settings.chatLayout")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr(self.settings.language, "settings.chatLayoutDescription")),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(40.))
                    .h(px(20.))
                    .rounded_full()
                    .bg(if enabled {
                        cx.theme().primary
                    } else {
                        cx.theme().muted
                    })
                    .child(
                        div()
                            .absolute()
                            .top(px(4.))
                            .left(if enabled { px(24.) } else { px(4.) })
                            .size(px(12.))
                            .rounded_full()
                            .bg(gpui_kit::white()),
                    ),
            )
    }

    fn terminal_setting(
        &self,
        id: &'static str,
        terminal: PreferredTerminal,
        icon: IconName,
        label: &'static str,
        description: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.settings.preferred_terminal == terminal;
        BaseButton::new(id)
            .accessibility_label(label)
            .w_full()
            .flex()
            .items_center()
            .justify_start()
            .gap_3()
            .p(px(10.))
            .rounded_md()
            .border_1()
            .border_color(if selected {
                cx.theme().list_active_border
            } else {
                cx.theme().border.opacity(0.6)
            })
            .bg(if selected {
                cx.theme().selection
            } else {
                cx.theme().background
            })
            .cursor_pointer()
            .hover(|style| {
                style
                    .border_color(cx.theme().list_active_border)
                    .bg(cx.theme().selection)
            })
            .on_click(cx.listener(move |this, _, _, cx| this.set_terminal(terminal, cx)))
            .child(
                div()
                    .flex_none()
                    .size(px(32.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if selected {
                        cx.theme().primary
                    } else {
                        cx.theme().muted
                    })
                    .text_color(if selected {
                        cx.theme().primary_foreground
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(Icon::new(icon).size(px(16.))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .v_flex()
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(description),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .size(px(16.))
                    .rounded_full()
                    .border_2()
                    .border_color(if selected {
                        cx.theme().primary
                    } else {
                        cx.theme().muted_foreground.opacity(0.3)
                    })
                    .bg(if selected {
                        cx.theme().primary
                    } else {
                        cx.theme().transparent
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |view| {
                        view.child(div().size(px(6.)).rounded_full().bg(gpui_kit::white()))
                    }),
            )
    }
}

impl Render for YesSessions {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .p_4()
                    .when(self.settings.sidebar_collapsed, |view| {
                        view.child(self.render_detail(window, cx))
                    })
                    .when(!self.settings.sidebar_collapsed, |view| {
                        view.child(
                            h_resizable("sessions-workspace")
                                .child(
                                    resizable_panel()
                                        .size(px(320.))
                                        .size_range(px(160.)..px(960.))
                                        .child(self.render_sidebar(cx)),
                                )
                                .child(resizable_panel().child(self.render_detail(window, cx))),
                        )
                    }),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    div()
                        .absolute()
                        .bottom_4()
                        .right_4()
                        .max_w(px(420.))
                        .rounded_lg()
                        .bg(cx.theme().danger)
                        .text_color(cx.theme().danger_foreground)
                        .px_4()
                        .py_3()
                        .child(error),
                )
            })
            .when(self.settings_open, |view| {
                view.child(self.render_settings(cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{
        ancestor_session_ids, collect_mermaid_sources, detail_source_signature,
        directory_group_labels, format_count, mermaid_sources_changed, navigator_preview,
        navigator_window, selection_after_refresh, session_directory_group_key,
    };
    use crate::conversation::inline_subagent_scope_base;
    use yes_core::{AppType, MessageType, Session, SessionMessage, model::SessionKind};

    fn session(id: &str, parent_session_id: Option<&str>) -> Session {
        Session {
            id: id.into(),
            app_type: AppType::Claude,
            file_name: format!("{id}.jsonl"),
            file_path: PathBuf::from(format!("/{id}.jsonl")),
            created_at: 0,
            updated_at: 0,
            message_count: 0,
            first_message: String::new(),
            last_message: String::new(),
            directory: None,
            uuid: None,
            kind: if parent_session_id.is_some() {
                SessionKind::Subagent
            } else {
                SessionKind::Main
            },
            parent_session_id: parent_session_id.map(str::to_owned),
            agent_type: None,
        }
    }

    #[test]
    fn refresh_preserves_selection_and_handles_new_or_deleted_sessions() {
        let old = vec![session("old", None)];
        let next = vec![session("new", None), session("old", None)];
        assert_eq!(
            selection_after_refresh(&old, &next, Some("old")).as_deref(),
            Some("old")
        );
        assert_eq!(
            selection_after_refresh(&[], &next, None).as_deref(),
            Some("new")
        );
        assert_eq!(
            selection_after_refresh(&old, &next[..1], Some("old")).as_deref(),
            Some("new")
        );
        assert_eq!(selection_after_refresh(&old, &[], Some("old")), None);
        assert_eq!(
            selection_after_refresh(&old, &next, Some("hidden-subagent")).as_deref(),
            Some("hidden-subagent")
        );
        assert_eq!(selection_after_refresh(&[], &[], None), None);
    }

    #[test]
    fn mermaid_cache_invalidates_changed_removed_and_reindexed_diagrams() {
        let message = |text: &str| SessionMessage::text(MessageType::Assistant, "", text);
        let old = vec![message("```mermaid\ngraph TD\nA-->B\n```")];
        assert!(!mermaid_sources_changed(&old, &old));
        let edited = vec![message("```mermaid\ngraph TD\nA-->C\n```")];
        assert!(mermaid_sources_changed(&old, &edited));
        assert!(mermaid_sources_changed(&old, &[]));
        assert!(mermaid_sources_changed(
            &old,
            &[message("new text"), old[0].clone()]
        ));
        assert!(!mermaid_sources_changed(
            &old,
            &[old[0].clone(), message("extra prose")]
        ));
    }

    #[test]
    fn selected_subagent_expands_every_ancestor() {
        let sessions = vec![
            session("root", None),
            session("child", Some("root")),
            session("grandchild", Some("child")),
        ];

        assert_eq!(
            ancestor_session_ids(&sessions, "grandchild"),
            vec!["child", "root"]
        );
        assert!(ancestor_session_ids(&sessions, "root").is_empty());
    }

    #[test]
    fn cyclic_parent_data_does_not_loop() {
        let sessions = vec![
            session("first", Some("second")),
            session("second", Some("first")),
        ];

        assert_eq!(ancestor_session_ids(&sessions, "first"), vec!["second"]);
    }

    #[test]
    fn directory_labels_match_legacy_compaction() {
        assert_eq!(directory_group_labels("project"), ("project".into(), None));
        assert_eq!(
            directory_group_labels("/Users/project"),
            ("project".into(), Some("Users/...".into()))
        );
        assert_eq!(
            directory_group_labels("/Users/me/project"),
            ("project".into(), Some("/Users/me".into()))
        );
        assert_eq!(
            directory_group_labels("/Users/me/work/project"),
            ("project".into(), Some("../me/work...".into()))
        );
    }

    #[test]
    fn directory_group_falls_back_to_session_file_parent() {
        let mut item = session("fallback", None);
        item.file_path = PathBuf::from("/Users/me/.claude/projects/fallback.jsonl");
        assert_eq!(
            session_directory_group_key(&item, "no directory"),
            "/Users/me/.claude/projects"
        );

        item.directory = Some(PathBuf::from("/Users/me/Workspace/project"));
        assert_eq!(
            session_directory_group_key(&item, "no directory"),
            "/Users/me/Workspace/project"
        );
    }

    #[test]
    fn navigator_preview_normalizes_and_truncates_unicode() {
        assert_eq!(navigator_preview("  first\n   second ", 28), "first second");
        assert_eq!(navigator_preview("", 28), "...");
        assert_eq!(navigator_preview("你好世界", 3), "你好世...");
    }

    #[test]
    fn navigator_window_stays_centered_and_in_bounds() {
        assert_eq!(navigator_window(6, 3, 8), 0..6);
        assert_eq!(navigator_window(20, 0, 8), 0..8);
        assert_eq!(navigator_window(20, 10, 8), 6..14);
        assert_eq!(navigator_window(20, 19, 8), 12..20);
    }

    #[test]
    fn inline_mermaid_sources_use_the_subagent_message_scope() {
        let messages = vec![SessionMessage::text(
            MessageType::Assistant,
            "2026-09-05T00:00:00Z",
            "```mermaid\ngraph TD\nA --> B\n```",
        )];
        let scope = inline_subagent_scope_base("child-session");
        let mut sources = Vec::new();

        collect_mermaid_sources(&messages, scope, &mut sources);

        assert_eq!(sources, vec![((scope, 0), "graph TD\nA --> B".into())]);
    }

    #[test]
    fn counts_are_grouped_for_stats_tooltip() {
        assert_eq!(format_count(12), "12");
        assert_eq!(format_count(1_234), "1,234");
        assert_eq!(format_count(9_876_543), "9,876,543");
    }

    #[test]
    fn opencode_detail_signature_tracks_the_sqlite_wal() {
        let directory = std::env::temp_dir().join(format!(
            "yes-sessions-signature-test-{}",
            std::process::id()
        ));
        let database = directory.join("opencode.db");
        let wal = directory.join("opencode.db-wal");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&database, b"database").unwrap();

        let mut item = session("opencode", None);
        item.app_type = AppType::OpenCode;
        item.file_path = database;
        assert!(detail_source_signature(&item).unwrap().sqlite_wal.is_none());

        fs::write(&wal, b"pending transaction").unwrap();
        assert!(detail_source_signature(&item).unwrap().sqlite_wal.is_some());

        fs::remove_dir_all(directory).unwrap();
    }
}
