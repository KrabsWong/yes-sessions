#[path = "session_search.rs"]
mod session_search;
use session_search::SessionSearch;
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use chrono::{Local, TimeZone as _};
use gpui_kit::base::{Button as BaseButton, Selectable as _, StyledExt};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Theme, ThemeMode, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_resizable,
    menu::{DropdownMenu as _, PopupMenuItem},
    message_scroller::MessageScrollerState,
    notification::Notification,
    resizable_panel,
    tooltip::Tooltip,
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

fn directory_group_root(paths: &HashSet<String>) -> Option<PathBuf> {
    let mut parents = paths
        .iter()
        .filter_map(|path| std::path::Path::new(path).parent());
    let mut root = parents.next()?.to_path_buf();
    for parent in parents {
        while !parent.starts_with(&root) {
            if !root.pop() {
                return None;
            }
        }
    }
    Some(root)
}

fn directory_group_labels(path: &str, root: Option<&std::path::Path>) -> (String, Option<String>) {
    let path = std::path::Path::new(path);
    let directory = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    let parent = path.parent().and_then(|parent| {
        let parent = root
            .and_then(|root| parent.strip_prefix(root).ok())
            .unwrap_or(parent);
        let parts = parent
            .components()
            .filter_map(|part| match part {
                std::path::Component::Normal(name) => Some(name.to_string_lossy()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if parts.is_empty() {
            None
        } else {
            // Closest ancestors carry more context than common workspace prefixes.
            Some(parts[parts.len().saturating_sub(2)..].join("/"))
        }
    });
    (directory, parent)
}

fn detail_directory_label(path: &std::path::Path) -> String {
    let parts = path.iter().collect::<Vec<_>>();
    let directories = parts
        .iter()
        .filter(|part| **part != std::ffi::OsStr::new("/"))
        .count();
    if directories <= 3 {
        return path.display().to_string();
    }
    let tail = parts[parts.len() - 3..].iter().collect::<PathBuf>();
    format!("…/{}", tail.display())
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

pub(crate) fn is_navigable_user_message(message: &yes_core::SessionMessage) -> bool {
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

fn update_conversation_scroll(
    state: &mut MessageScrollerState,
    previous_count: usize,
    next_count: usize,
    cx: &mut Context<MessageScrollerState>,
) {
    if next_count > previous_count {
        state.append(next_count - previous_count, cx);
    } else if next_count < previous_count {
        state.splice(next_count..previous_count, 0, cx);
    }
    // Preserve the absolute position within a turn when streaming changes its height.
    state.remeasure_items(0..next_count, cx);
}

fn unread_after_refresh(current: usize, previous: usize, next: usize, scrolled_up: bool) -> usize {
    if scrolled_up {
        current.saturating_add(next.saturating_sub(previous))
    } else {
        0
    }
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

fn navigator_tick_width(position: usize, focus: Option<usize>, active: bool) -> f32 {
    let width: f32 = match focus.map(|focus| position.abs_diff(focus)) {
        Some(0) => 28.,
        Some(1) => 21.,
        Some(2) => 15.,
        Some(3) => 10.,
        _ => 6.,
    };
    if active { width.max(18.) } else { width }
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
        ConversationOptions, conversation_scroller, conversation_turn_count, turn_index_for_message,
    },
    i18n::tr,
    mermaid::{MermaidDiagram, create_mermaid_diagram},
    preview::WorkspacePreview,
};

// Keep the virtual list itself, including the offset within a message, while visiting children.
struct ParentConversation {
    session_id: String,
    detail: Arc<SessionDetail>,
    scroll: Entity<MessageScrollerState>,
    expanded_messages: HashSet<usize>,
    claude_xml_expanded: HashMap<(usize, usize), bool>,
    unread: usize,
    active_message: Option<usize>,
    source_signature: Option<DetailSourceSignature>,
}

pub struct YesSessions {
    root_focus: FocusHandle,
    pub(crate) search_landing_scroll_pending: bool,
    pub(crate) search_landing: Option<(usize, Instant)>,
    session_search: SessionSearch,
    copied_metadata: Option<(&'static str, Instant)>,
    registry: Arc<ProviderRegistry>,
    settings_store: SettingsStore,
    pub settings: AppSettings,
    selected_app: AppType,
    sessions: Arc<Vec<Session>>,
    selected_session_id: Option<String>,
    detail: Option<Arc<SessionDetail>>,
    conversation_state: Entity<MessageScrollerState>,
    parent_conversations: Vec<ParentConversation>,
    unread_message_count: usize,
    loading_sessions: bool,
    refreshing_sessions: bool,
    loading_detail: bool,
    error: Option<String>,
    workspace_preview: Option<Entity<WorkspacePreview>>,
    preview_subscription: Option<Subscription>,
    preview_open: bool,
    preview_icon_transition: Option<(Instant, f32)>,
    settings_open: bool,
    settings_tab: SettingsTab,
    session_view_mode: SessionViewMode,
    collapsed_groups: HashSet<String>,
    expanded_parents: HashSet<String>,
    parent_transition: Option<(String, Instant, bool)>,
    expanded_messages: HashSet<usize>,
    pub(crate) claude_xml_expanded: HashMap<(usize, usize), bool>,
    marquee_session_id: Option<String>,
    sidebar_icon_transition: Option<(Instant, f32)>,
    stats_hovered: bool,
    navigator_active_message: Option<usize>,
    navigator_focus: Option<usize>,
    navigator_previous_focus: Option<usize>,
    navigator_motion: Option<Instant>,
    navigator_start: Option<usize>,
    navigator_wheel: f32,
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
            root_focus: cx.focus_handle(),
            search_landing: None,
            search_landing_scroll_pending: false,
            session_search: SessionSearch::default(),
            copied_metadata: None,
            registry: Arc::new(ProviderRegistry::default()),
            settings_store,
            settings,
            selected_app,
            sessions: Arc::new(Vec::new()),
            selected_session_id: None,
            detail: None,
            conversation_state: cx.new(|cx| MessageScrollerState::new(0, cx)),
            parent_conversations: Vec::new(),
            unread_message_count: 0,
            loading_sessions: false,
            refreshing_sessions: false,
            loading_detail: false,
            error: None,
            workspace_preview: None,
            preview_subscription: None,
            preview_open: false,
            preview_icon_transition: None,
            settings_open: false,
            settings_tab: SettingsTab::General,
            session_view_mode: SessionViewMode::Date,
            collapsed_groups: HashSet::new(),
            expanded_parents: HashSet::new(),
            parent_transition: None,
            expanded_messages: HashSet::new(),
            claude_xml_expanded: HashMap::new(),
            marquee_session_id: None,
            sidebar_icon_transition: None,
            stats_hovered: false,
            navigator_active_message: None,
            navigator_focus: None,
            navigator_previous_focus: None,
            navigator_motion: None,
            navigator_start: None,
            navigator_wheel: 0.,
            mermaid_views: HashMap::new(),
            terminal_info,
            sessions_generation: 0,
            detail_generation: 0,
            detail_source_signature: None,
            refreshing_detail: false,
        };
        cx.observe_window_appearance(window, |this, window, cx| {
            // AppKit may report appearance again when restoring/activating a window.
            // Do not rebuild themes and Mermaid views unless light/dark actually changed.
            if this.settings.theme == ThemePreference::System
                && ThemeMode::from(window.appearance()) != cx.theme().mode
            {
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
        this.root_focus.focus(window, cx);
        this.observe_conversation_scroll(cx);
        crate::commands::update_menus(this.settings.language, cx);
        this.load_sessions(cx);
        this.start_live_refresh(cx);
        this
    }

    pub(crate) fn sync_navigator_message(
        &mut self,
        message_index: usize,
        state: &Entity<MessageScrollerState>,
        cx: &mut Context<Self>,
    ) {
        if state != &self.conversation_state || self.navigator_active_message == Some(message_index)
        {
            return;
        }
        self.navigator_active_message = Some(message_index);
        if self.navigator_focus.is_none() {
            self.navigator_start = None;
        }
        cx.notify();
    }

    fn observe_conversation_scroll(&mut self, cx: &mut Context<Self>) {
        let mut was_scrolled_up = false;
        cx.observe(&self.conversation_state, move |this, state, cx| {
            if state != this.conversation_state {
                return;
            }
            let state = state.read(cx);
            let scrolled_up = state.is_scrolled_up();
            let clear_unread = this.unread_message_count > 0 && state.is_following_tail();
            if clear_unread {
                this.unread_message_count = 0;
            }
            if clear_unread || scrolled_up != was_scrolled_up {
                was_scrolled_up = scrolled_up;
                cx.notify();
            }
        })
        .detach();
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
        theme.notification.placement = Anchor::TopCenter;
        theme.notification.width = px(360.);
        theme.notification.max_items = 1;
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
        } else if accent == AccentColor::Default {
            hsla(210. / 360., 0.40, 0.98, 1.)
        } else {
            gpui_kit::white()
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
        // Apply secondary/accent presets along with the derived primary surfaces.
        if accent != AccentColor::Default {
            let surface_saturation = match accent {
                AccentColor::Slate => 0.20,
                AccentColor::Zinc => 0.05,
                AccentColor::Neutral => 0.,
                AccentColor::Purple => {
                    if dark {
                        0.50
                    } else {
                        0.60
                    }
                }
                _ if dark => 0.60,
                AccentColor::Orange | AccentColor::Amber | AccentColor::Yellow => 0.90,
                AccentColor::Green | AccentColor::Teal => 0.70,
                _ => 0.80,
            };
            theme.secondary = hsla(
                hue / 360.,
                surface_saturation,
                if dark { 0.25 } else { 0.94 },
                1.,
            );
            theme.accent = hsla(
                hue / 360.,
                surface_saturation,
                if dark { 0.20 } else { 0.96 },
                1.,
            );
        }
        theme.secondary_foreground = if dark { gpui_kit::white() } else { color };
        theme.accent_foreground = theme.secondary_foreground;
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
        self.parent_conversations.clear();
        self.unread_message_count = 0;
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
        self.workspace_preview = None;
        self.preview_subscription = None;
        self.preview_open = false;
        self.preview_icon_transition = None;
        self.mermaid_views.clear();
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
        self.copied_metadata = None;
        self.search_landing = None;
        self.search_landing_scroll_pending = false;
        self.session_search = SessionSearch::default();
        self.unread_message_count = 0;
        self.detail_generation += 1;
        self.selected_session_id = None;
        self.error = None;
        self.loading_detail = false;
        self.refreshing_detail = false;
        self.detail_source_signature = None;
        self.detail = None;
        self.workspace_preview = None;
        self.preview_subscription = None;
        self.preview_open = false;
        self.preview_icon_transition = None;
        self.expanded_messages.clear();
        self.claude_xml_expanded.clear();
        self.navigator_active_message = None;
        self.navigator_focus = None;
        self.navigator_previous_focus = None;
        self.navigator_start = None;
        self.navigator_motion = None;
        self.navigator_wheel = 0.;
        self.mermaid_views.clear();
        self.conversation_state
            .update(cx, |state, cx| state.reset(0, cx));
    }

    fn select_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        let ancestors = ancestor_session_ids(&self.sessions, &session_id);
        if self
            .parent_conversations
            .iter()
            .any(|saved| saved.session_id == session_id || ancestors.contains(&saved.session_id))
            || self
                .detail
                .as_ref()
                .is_some_and(|detail| ancestors.contains(&detail.session.id))
        {
            self.open_sub_agent(&session_id, cx);
            return;
        }
        self.parent_conversations.clear();
        self.load_session(session_id, cx);
    }

    fn load_session(&mut self, session_id: String, cx: &mut Context<Self>) {
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
                        if this.detail.as_ref().is_some_and(|previous| {
                            mermaid_sources_changed(&previous.messages, &detail.messages)
                        }) {
                            this.mermaid_views.clear();
                        }
                        this.unread_message_count = unread_after_refresh(
                            this.unread_message_count,
                            this.detail
                                .as_ref()
                                .map_or(0, |detail| detail.messages.len()),
                            detail.messages.len(),
                            !this.conversation_state.read(cx).is_following_tail(),
                        );
                        this.detail = Some(Arc::new(detail));
                        this.conversation_state.update(cx, |state, cx| {
                            update_conversation_scroll(state, previous_count, next_count, cx);
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
                        if this.preview_open {
                            if let Some(preview) = &this.workspace_preview {
                                preview.update(cx, |preview, cx| preview.check_for_changes(cx));
                            }
                        }
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
        if self.selected_session_id.as_deref() == Some(&id) {
            return;
        }
        if let Some(index) = self
            .parent_conversations
            .iter()
            .rposition(|saved| saved.session_id == id)
        {
            let saved = self.parent_conversations.remove(index);
            self.parent_conversations.truncate(index);
            self.reset_detail(cx);
            self.selected_session_id = Some(saved.session_id);
            self.detail = Some(saved.detail);
            self.conversation_state = saved.scroll;
            self.expanded_messages = saved.expanded_messages;
            self.claude_xml_expanded = saved.claude_xml_expanded;
            self.unread_message_count = saved.unread;
            self.navigator_active_message = saved.active_message;
            self.detail_source_signature = saved.source_signature;
            // Refresh changed content through the normal anchor-preserving update path.
            self.refresh_selected_detail(cx);
            cx.notify();
            return;
        }
        let ancestors = ancestor_session_ids(&self.sessions, &id);
        if let Some(detail) = self
            .detail
            .as_ref()
            .filter(|detail| ancestors.contains(&detail.session.id))
        {
            self.parent_conversations.push(ParentConversation {
                session_id: detail.session.id.clone(),
                detail: detail.clone(),
                scroll: self.conversation_state.clone(),
                expanded_messages: self.expanded_messages.clone(),
                claude_xml_expanded: self.claude_xml_expanded.clone(),
                unread: self.unread_message_count,
                active_message: self.navigator_active_message,
                source_signature: self.detail_source_signature.clone(),
            });
            self.conversation_state = cx.new(|cx| MessageScrollerState::new(0, cx));
            self.observe_conversation_scroll(cx);
            self.load_session(id, cx);
        } else {
            let retained = self
                .parent_conversations
                .iter()
                .rposition(|saved| ancestors.contains(&saved.session_id))
                .map_or(0, |index| index + 1);
            self.parent_conversations.truncate(retained);
            self.load_session(id, cx);
        }
    }

    pub(crate) fn toggle_claude_xml(
        &mut self,
        turn_index: usize,
        message_index: usize,
        item_indices: Vec<usize>,
        expanded: bool,
        cx: &mut Context<Self>,
    ) {
        for index in item_indices {
            self.claude_xml_expanded
                .insert((message_index, index), expanded);
        }
        self.conversation_state.update(cx, |state, cx| {
            let _ = state.remeasure_items(turn_index..turn_index + 1, cx);
        });
        cx.notify();
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
        crate::commands::update_menus(language, cx);
        if let Some(preview) = &self.workspace_preview {
            preview.update(cx, |preview, cx| preview.set_language(language, cx));
        }
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

    fn sidebar_icon_scale(&self) -> f32 {
        let target = if self.settings.sidebar_collapsed {
            -1.
        } else {
            1.
        };
        let Some((started, from)) = self.sidebar_icon_transition else {
            return target;
        };
        let progress = (started.elapsed().as_secs_f32() / 0.2).min(1.);
        from + (target - from) * ease_in_out(progress)
    }

    fn preview_icon_scale(&self) -> f32 {
        let target = if self.preview_open { -1. } else { 1. };
        let Some((started, from)) = self.preview_icon_transition else {
            return target;
        };
        let progress = (started.elapsed().as_secs_f32() / 0.2).min(1.);
        from + (target - from) * ease_in_out(progress)
    }

    fn render_header(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self
            .parent_transition
            .as_ref()
            .is_some_and(|(_, started, _)| started.elapsed() < Duration::from_millis(160))
        {
            window.request_animation_frame();
        }
        if self
            .navigator_motion
            .is_some_and(|started| started.elapsed() < Duration::from_millis(140))
        {
            window.request_animation_frame();
        }

        if self
            .sidebar_icon_transition
            .is_some_and(|(started, _)| started.elapsed() < Duration::from_millis(200))
        {
            window.request_animation_frame();
        }
        if self
            .preview_icon_transition
            .is_some_and(|(started, _)| started.elapsed() < Duration::from_millis(200))
        {
            window.request_animation_frame();
        }
        let language = self.settings.language;
        div()
            // Traffic lights start at y=18 and are 14pt tall, centered at y=25.
            .h(px(50.))
            .flex_none()
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
                            .child(Icon::new(IconName::PanelLeft).size(px(16.)).transform(
                                Transformation::scale(size(self.sidebar_icon_scale(), 1.)),
                            ))
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
                                let from = this.sidebar_icon_scale();
                                this.settings.sidebar_collapsed = !this.settings.sidebar_collapsed;
                                this.sidebar_icon_transition = Some((Instant::now(), from));
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
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("search-session")
                            .ghost()
                            .icon(IconName::Search)
                            .tooltip(tr(language, "search.open"))
                            .disabled(self.detail.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_session_search(window, cx)
                            })),
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
                    .child(
                        Button::new("toggle-workspace")
                            .ghost()
                            .compact()
                            .size(px(32.))
                            .disabled(!self.has_workspace())
                            .child(Icon::new(IconName::PanelLeft).size(px(16.)).transform(
                                Transformation::scale(size(self.preview_icon_scale(), 1.)),
                            ))
                            .tooltip(tr(
                                language,
                                if self.preview_open {
                                    "preview.collapsePanel"
                                } else {
                                    "preview.expandPanel"
                                },
                            ))
                            .accessibility_label(tr(
                                language,
                                if self.preview_open {
                                    "preview.collapsePanel"
                                } else {
                                    "preview.expandPanel"
                                },
                            ))
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.toggle_workspace(window, cx)
                                }),
                            ),
                    ),
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
            .accessibility_label(self.selected_app.display_name())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(ProviderIcon::from(self.selected_app)).size(px(16.)))
                    .child(self.selected_app.display_name()),
            )
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
        let expanding = !self.expanded_parents.contains(id);
        self.parent_transition = Some((id.to_owned(), Instant::now(), expanding));
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
        let parent_transition = self.parent_transition.clone();
        let marquee_session_id = self.marquee_session_id.clone();
        let marquee_enabled = self.settings.enable_title_marquee;
        let group_keys = sessions
            .iter()
            .filter(|session| session.kind == yes_core::model::SessionKind::Main)
            .map(|session| self.session_group_key(session))
            .collect::<HashSet<_>>();
        let directory_root = directory_group_root(&group_keys);
        let all_expanded = group_keys
            .iter()
            .all(|key| !self.collapsed_groups.contains(key));
        let all_collapsed = !group_keys.is_empty()
            && group_keys
                .iter()
                .all(|key| self.collapsed_groups.contains(key));
        div()
            .debug_selector(|| "sessions-sidebar".into())
            .w_full()
            .min_w_0()
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
                            .bg(cx.theme().button)
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
                                    let (group_label, parent_path) = if view_mode
                                        == SessionViewMode::Directory
                                    {
                                        directory_group_labels(&label, directory_root.as_deref())
                                    } else {
                                        (label, None)
                                    };
                                    let full_path = key.clone();
                                    return div()
                                        .id(("group", index))
                                        .debug_selector(move || {
                                            format!("session-group-{index}").into()
                                        })
                                        .tooltip(move |window, cx| {
                                            Tooltip::new(full_path.clone()).build(window, cx)
                                        })
                                        .w_full()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .h(px(36.))
                                        .px_4()
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
                                                    Icon::new(match (view_mode, collapsed) {
                                                        (SessionViewMode::Directory, true) => {
                                                            IconName::Folder
                                                        }
                                                        (SessionViewMode::Directory, false) => {
                                                            IconName::FolderOpen
                                                        }
                                                        (_, true) => IconName::ChevronRight,
                                                        (_, false) => IconName::ChevronDown,
                                                    })
                                                    .size(px(16.))
                                                    .when(
                                                        view_mode == SessionViewMode::Directory,
                                                        |icon| {
                                                            icon.text_color(
                                                                cx.theme().muted_foreground,
                                                            )
                                                        },
                                                    ),
                                                )
                                                .child(
                                                    div().min_w_0().truncate().child(group_label),
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
                                                    .flex_none()
                                                    .items_center()
                                                    .gap(px(2.))
                                                    .child(
                                                        Button::new("expand-all-groups")
                                                            .debug_selector(|| {
                                                                "expand-all-groups".into()
                                                            })
                                                            .ghost()
                                                            .compact()
                                                            .size(px(24.))
                                                            .icon(IconName::ChevronDown)
                                                            .disabled(
                                                                loading
                                                                    || main_count == 0
                                                                    || all_expanded,
                                                            )
                                                            .tooltip(tr(
                                                                language,
                                                                "sessions.expandAll",
                                                            ))
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    cx.stop_propagation();
                                                                    this.expand_all_groups(cx);
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("collapse-all-groups")
                                                            .debug_selector(|| {
                                                                "collapse-all-groups".into()
                                                            })
                                                            .ghost()
                                                            .compact()
                                                            .size(px(24.))
                                                            .icon(IconName::ChevronUp)
                                                            .disabled(
                                                                loading
                                                                    || main_count == 0
                                                                    || all_collapsed,
                                                            )
                                                            .tooltip(tr(
                                                                language,
                                                                "sessions.collapseAll",
                                                            ))
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    cx.stop_propagation();
                                                                    this.collapse_all_groups(cx);
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
                                let transition =
                                    parent_transition.as_ref().map(|(id, started, expanding)| {
                                        let t = (started.elapsed().as_secs_f32() / 0.16).min(1.);
                                        (id, 1. - (1. - t).powi(3), *expanding)
                                    });
                                let arrow_progress =
                                    transition.filter(|(id, _, _)| **id == session.id).map_or(
                                        if children_expanded { 1. } else { 0. },
                                        |(_, t, expanding)| {
                                            if expanding { t } else { 1. - t }
                                        },
                                    );
                                let reveal = transition
                                    .filter(|(id, _, expanding)| {
                                        *expanding
                                            && session.parent_session_id.as_ref() == Some(*id)
                                    })
                                    .map_or(1., |(_, t, _)| t);
                                let purple = if cx.theme().mode == ThemeMode::Dark {
                                    rgb(0xd8b4fe)
                                } else {
                                    rgb(0x9333ea)
                                };
                                let purple_hover = if cx.theme().mode == ThemeMode::Dark {
                                    Hsla::from(rgb(0x581c87)).opacity(0.4)
                                } else {
                                    rgb(0xf3e8ff).into()
                                };
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
                                                    .line_height(px(20.))
                                                    .py(px(1.))
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
                                                    cx.theme().button
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
                                                .custom(
                                                    ButtonCustomVariant::new(cx)
                                                        .color(cx.theme().transparent)
                                                        .foreground(purple.into())
                                                        .hover(purple_hover)
                                                        .active(purple_hover),
                                                )
                                                .rounded(px(4.))
                                                .compact()
                                                .absolute()
                                                .right_1()
                                                .h(px(24.))
                                                .px(px(6.))
                                                .text_color(purple)
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .child(
                                                            Icon::new(IconName::ChevronRight)
                                                                .size(px(14.))
                                                                .rotate(percentage(
                                                                    arrow_progress * 0.25,
                                                                )),
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
                                    .relative()
                                    .left(px((1. - reveal) * -4.))
                                    .opacity(reveal)
                                    .w_full()
                                    .h(px(36.))
                                    .pl(px(16. + (depth.min(4) * 20) as f32))
                                    .pr_2()
                                    .child(row)
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .w_full()
                .min_w_0()
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
                    .position(|(index, _, _)| *index == active)
            })
            .unwrap_or(0);
        let total = user_messages.len();
        let count = total.min(32);
        let start = self
            .navigator_start
            .unwrap_or_else(|| navigator_window(total, active_position, 32).start)
            .min(total - count);
        let height = count as f32 * 10.;
        let focus = self
            .navigator_focus
            .filter(|index| *index >= start && *index < start + count);
        let progress = self.navigator_motion.map_or(1., |started| {
            (started.elapsed().as_secs_f32() / 0.14).min(1.)
        });
        let progress = 1. - (1. - progress).powi(3);
        let language = self.settings.language;
        let sender_label =
            tr(
                language,
                if self.detail.as_ref().is_some_and(|detail| {
                    detail.session.kind == yes_core::model::SessionKind::Subagent
                }) {
                    "message.mainAgent"
                } else {
                    "message.user"
                },
            );
        let mut rail = div()
            .id("user-message-navigator")
            .debug_selector(|| "user-message-navigator".into())
            .absolute()
            .right_3()
            .top(relative(0.5))
            .mt(px(-height / 2.))
            .w(px(300.))
            .h(px(height))
            .v_flex()
            .items_end()
            .when(focus.is_some(), |view| view.occlude())
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.navigator_start = Some(start);
                } else {
                    this.navigator_previous_focus = this.navigator_focus;
                    this.navigator_focus = None;
                    this.navigator_motion = Some(Instant::now());
                    this.navigator_start = None;
                    this.navigator_wheel = 0.;
                }
                cx.notify();
            }));
        for position in start..start + count {
            let (message_index, _, turn_index) = user_messages[position];
            let active = position == active_position;
            let from = navigator_tick_width(position, self.navigator_previous_focus, active);
            let to = navigator_tick_width(position, focus, active);
            let width = from + (to - from) * progress;
            rail = rail.child(
                div()
                    .id(("navigator-tick", message_index))
                    .debug_selector(move || format!("navigator-tick-{position}").into())
                    .w(px(32.))
                    .h(px(10.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.navigator_focus != Some(position) {
                            this.navigator_previous_focus = this.navigator_focus;
                            this.navigator_focus = Some(position);
                            this.navigator_motion = Some(Instant::now());
                            cx.notify();
                        }
                    }))
                    .child(
                        Button::new(("navigator-jump", message_index))
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(cx.theme().transparent)
                                    .foreground(cx.theme().foreground)
                                    .hover(cx.theme().transparent)
                                    .active(cx.theme().transparent),
                            )
                            .compact()
                            .rounded_none()
                            .w_full()
                            .h(px(10.))
                            .p_0()
                            .justify_end()
                            .accessibility_label(format!("{} {}", sender_label, position + 1))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.navigator_active_message = Some(message_index);
                                this.conversation_state.update(cx, |state, cx| {
                                    state.scroll_to_item(turn_index, cx);
                                });
                                cx.notify();
                            }))
                            .child(div().w(px(width)).h(px(2.)).rounded_full().bg(
                                if focus == Some(position) || active {
                                    cx.theme().foreground.opacity(0.85)
                                } else {
                                    cx.theme().muted_foreground.opacity(0.30)
                                },
                            )),
                    ),
            );
        }
        rail = rail.child(
            div()
                .id("navigator-scroll-mask")
                .debug_selector(|| "navigator-scroll-mask".into())
                .absolute()
                .right_0()
                .top_0()
                .w(px(32.))
                .h(px(height))
                .on_scroll_wheel(
                    cx.listener(move |this, event: &ScrollWheelEvent, window, cx| {
                        if total > count && this.navigator_focus.is_some() {
                            this.navigator_wheel +=
                                f32::from(event.delta.pixel_delta(window.line_height()).y);
                            let steps = (this.navigator_wheel / 24.).trunc() as isize;
                            if steps != 0 {
                                this.navigator_wheel -= steps as f32 * 24.;
                                let previous_start = this.navigator_start.unwrap_or(start);
                                let next_start = (previous_start as isize - steps)
                                    .clamp(0, (total - count) as isize)
                                    as usize;
                                this.navigator_start = Some(next_start);
                                this.navigator_focus = this.navigator_focus.map(|position| {
                                    next_start
                                        + position.saturating_sub(previous_start).min(count - 1)
                                });
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }
                    }),
                ),
        );
        if let Some(position) = focus {
            let (message_index, ref content, turn_index) = user_messages[position];
            let response = messages[message_index + 1..]
                .iter()
                .take_while(|message| !is_navigable_user_message(message))
                .find(|message| {
                    message.message_type == yes_core::MessageType::Assistant
                        && message
                            .content
                            .as_deref()
                            .is_some_and(|text| !text.trim().is_empty())
                })
                .and_then(|message| message.content.as_deref())
                .unwrap_or_default();
            rail = rail.child(
                div()
                    .id("navigator-preview-card")
                    .debug_selector(|| "navigator-preview-card".into())
                    .absolute()
                    .right(px(40.))
                    .top(px(
                        ((position - start) as f32 * 10. - 40.).clamp(0., (height - 106.).max(0.))
                    ))
                    .w(px(260.))
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.65))
                    .bg(cx.theme().background)
                    .shadow_lg()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigator_active_message = Some(message_index);
                        this.conversation_state.update(cx, |state, cx| {
                            state.scroll_to_item(turn_index, cx);
                        });
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(navigator_preview(content, 80)),
                    )
                    .when(!response.is_empty(), |view| {
                        view.child(
                            div()
                                .mt_1()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .max_h(px(54.))
                                .overflow_hidden()
                                .whitespace_normal()
                                .child(navigator_preview(response, 140)),
                        )
                    })
                    .child(
                        div()
                            .mt_2()
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} · {} / {}", sender_label, position + 1, total)),
                    ),
            );
        }
        deferred(rail).with_priority(2).into_any_element()
    }

    fn set_preview_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.preview_open != open {
            let from = self.preview_icon_scale();
            self.preview_open = open;
            self.preview_icon_transition = Some((Instant::now(), from));
            cx.notify();
        }
    }

    fn toggle_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview_open {
            self.set_preview_open(false, cx);
            return;
        }
        self.ensure_workspace_preview(window, cx);
        if let Some(preview) = &self.workspace_preview {
            preview.update(cx, |preview, cx| preview.reveal(cx));
            self.set_preview_open(true, cx);
        }
    }

    fn ensure_workspace_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace_preview.is_some() {
            return;
        }
        let Some(root) = self
            .detail
            .as_ref()
            .and_then(|detail| detail.session.directory.clone())
        else {
            return;
        };
        let language = self.settings.language;
        let preview = cx.new(|cx| WorkspacePreview::new(root, language, window, cx));
        self.preview_subscription = Some(cx.observe(&preview, |_, _, cx| cx.notify()));
        self.workspace_preview = Some(preview);
    }

    pub fn has_workspace(&self) -> bool {
        self.detail
            .as_ref()
            .is_some_and(|detail| detail.session.directory.is_some())
    }

    pub fn open_workspace_file(
        &mut self,
        path: PathBuf,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_workspace_preview(window, cx);
        if let Some(preview) = &self.workspace_preview {
            preview.update(cx, |preview, cx| preview.open_path(path, line, cx));
            self.set_preview_open(true, cx);
        }
    }

    fn render_workspace_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(preview) = self.workspace_preview.clone() else {
            return div().into_any_element();
        };
        div()
            .v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(div().flex_1().min_h_0().child(preview))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(self.settings.language, "preview.currentWorkspace")),
            )
            .into_any_element()
    }

    fn metadata_button(
        &self,
        id: &'static str,
        title_key: &'static str,
        value: String,
        icon: Option<Icon>,
        label: Option<String>,
        cx: &mut Context<Self>,
    ) -> Button {
        let copied = self.copied_metadata.is_some_and(|(field, _)| field == id);
        let language = self.settings.language;
        let tooltip_value = value.clone();
        Button::new(id)
            .debug_selector(move || id.into())
            .accessibility_label(tr(language, title_key))
            .ghost()
            .compact()
            .h(px(24.))
            .px_1()
            .text_size(px(12.))
            .text_color(cx.theme().muted_foreground)
            .when_some(icon, |button, icon| {
                button.icon(
                    if copied {
                        Icon::new(IconName::Check)
                    } else {
                        icon
                    }
                    .size(px(14.)),
                )
            })
            .when_some(label, |button, label| button.label(label))
            .map(|mut button| {
                button.interactivity().tooltip(move |window, cx| {
                    let value = tooltip_value.clone();
                    Tooltip::element(move |_, cx| {
                        div()
                            .flex()
                            .items_baseline()
                            .gap_1()
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{}：", tr(language, title_key))),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .child(value.clone()),
                            )
                    })
                    .build(window, cx)
                });
                button
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                crate::toast::copy_success(language, window, cx);
                let feedback = (id, Instant::now());
                this.copied_metadata = Some(feedback);
                cx.notify();
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.copied_metadata == Some(feedback) {
                            this.copied_metadata = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }))
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
        self.ensure_workspace_preview(window, cx);
        let session = &detail.session;
        let updated_date = Local
            .timestamp_millis_opt(session.updated_at)
            .single()
            .map(|value| value.format("%Y.%m.%d").to_string())
            .unwrap_or_default();
        let resume_session_id = session.id.clone();
        let resume_app = session.app_type;
        let resume_dir = session.directory.clone();
        let terminal = self.settings.preferred_terminal;
        let parent_id = session.parent_session_id.clone();
        let agent_type = session.agent_type.clone();
        let title = if session.first_message.is_empty() {
            session.file_name.clone()
        } else {
            session.first_message.clone()
        };
        let title_view = div()
            .id("session-detail-title")
            .debug_selector(|| "session-detail-title".into())
            .min_w_0()
            .w_full()
            .whitespace_normal()
            .child(title);
        let messages = Arc::new(detail.messages.clone());
        if self.settings_open || self.session_search.input.is_some() {
            for diagram in self.mermaid_views.values() {
                MermaidDiagram::hide(diagram, cx);
            }
        } else {
            self.prepare_mermaid_views(window, cx);
        }
        let mermaid_views = if self.settings_open || self.session_search.input.is_some() {
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
                is_subagent: detail.session.kind == yes_core::model::SessionKind::Subagent,
                show_thinking: self.settings.show_thinking_content,
                chat_bubbles: self.settings.chat_layout == ChatLayout::Bubble,
                collapse_tool_blocks: self.settings.collapse_bash_blocks,
            },
            self.expanded_messages.clone(),
            mermaid_views,
            cx.weak_entity(),
        );
        let unread = self.unread_message_count;
        let has_navigator = detail
            .messages
            .iter()
            .filter(|message| is_navigable_user_message(message))
            .take(2)
            .count()
            > 1;
        let mut row_style = StyleRefinement::default();
        if has_navigator {
            // Preserve the usual 12px row inset and reserve 44px for navigation.
            row_style.padding.right = Some(px(56.).into());
        }
        let scroller = scroller.jump_button(false).with_row_style(row_style);
        let show_jump = unread > 0 || self.conversation_state.read(cx).is_scrolled_up();
        let navigator = self.render_user_navigator(&detail.messages, cx);
        div()
            .flex_1()
            .h_full()
            .min_w_0()
            .v_flex()
            .min_h_0()
            .child(
                div()
                    .flex_none()
                    .min_w_0()
                    .p_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .v_flex()
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_start()
                            .gap_2()
                            .when_some(parent_id, |view, parent_id| {
                                view.child(
                                    Button::new("back-to-parent")
                                        .debug_selector(|| "back-to-parent".into())
                                        .accessibility_label(tr(language, "sessions.backToParent"))
                                        .tooltip(tr(language, "sessions.backToParent"))
                                        .ghost()
                                        .compact()
                                        .flex_none()
                                        .size(px(24.))
                                        .icon(Icon::new(IconName::ArrowLeft).size(px(16.)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_sub_agent(&parent_id, cx)
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_start()
                                    .gap_1()
                                    .child(
                                        self.metadata_button(
                                            "copy-session-id",
                                            "sessions.sessionId",
                                            session.id.clone(),
                                            Some(Icon::empty().path("icons/session-id.svg")),
                                            None,
                                            cx,
                                        )
                                        .flex_none()
                                        .w(px(16.))
                                        .px_0(),
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
                            ),
                    )
                    .child(
                        div().mt_2().v_flex().gap_1().child(
                            div()
                                .debug_selector(|| "session-metadata-toolbar".into())
                                .flex()
                                .items_center()
                                .min_w_0()
                                .gap_1()
                                .when(
                                    session.kind == yes_core::model::SessionKind::Subagent,
                                    |view| {
                                        view.child(
                                            div()
                                                .debug_selector(|| "session-agent-badge".into())
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .rounded(px(4.))
                                                .bg(if cx.theme().mode.is_dark() {
                                                    rgb(0x292338).into()
                                                } else {
                                                    hsla(270. / 360., 0.67, 0.94, 1.)
                                                })
                                                .px_2()
                                                .h(px(24.))
                                                .text_size(px(12.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(if cx.theme().mode.is_dark() {
                                                    rgb(0xc4b5d9).into()
                                                } else {
                                                    hsla(270. / 360., 0.67, 0.42, 1.)
                                                })
                                                .child(Icon::new(IconName::Bot).size(px(14.)))
                                                .child(tr(language, "sessions.subAgent"))
                                                .when_some(agent_type, |view, agent_type| {
                                                    view.child(format!(" · {agent_type}"))
                                                }),
                                        )
                                    },
                                )
                                .when_some(session.directory.as_ref(), |view, directory| {
                                    let label = detail_directory_label(directory);
                                    view.child(
                                        self.metadata_button(
                                            "copy-session-directory",
                                            "sessions.work",
                                            directory.display().to_string(),
                                            None,
                                            Some(label),
                                            cx,
                                        )
                                        .min_w_0()
                                        .flex_shrink_1()
                                        .max_w(relative(0.65)),
                                    )
                                })
                                .when(session.directory.is_some(), |view| {
                                    view.child(
                                        div()
                                            .flex_none()
                                            .text_size(px(12.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child("/"),
                                    )
                                })
                                .child(
                                    div()
                                        .debug_selector(|| "session-updated-date".into())
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .h(px(24.))
                                        .text_size(px(12.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(updated_date),
                                )
                                .child(div().flex_1())
                                .when(
                                    session.kind != yes_core::model::SessionKind::Subagent,
                                    |view| {
                                        view.child(
                                            Button::new("resume-session")
                                                .flex_none()
                                                .custom(
                                                    ButtonCustomVariant::new(cx)
                                                        .color(cx.theme().foreground)
                                                        .foreground(cx.theme().background)
                                                        .hover(cx.theme().foreground.opacity(0.9))
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
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if let Err(error) = resume_session(
                                                        resume_app,
                                                        &resume_session_id,
                                                        resume_dir.as_deref(),
                                                        terminal,
                                                    ) {
                                                        this.error = Some(error.to_string());
                                                        cx.notify();
                                                    }
                                                })),
                                        )
                                    },
                                ),
                        ),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .debug_selector(|| "conversation-viewport".into())
                    .child(scroller.size_full())
                    .child(navigator)
                    .when(show_jump, |view| {
                        view.child(
                            div()
                                .absolute()
                                .bottom_4()
                                .left_0()
                                .right(px(if has_navigator { 44. } else { 0. }))
                                .flex()
                                .justify_center()
                                .child(
                                    Button::new("conversation-jump-latest")
                                        .debug_selector(|| "conversation-jump-latest".into())
                                        .rounded_full()
                                        .primary()
                                        .h(px(32.))
                                        .px_4()
                                        .py_2()
                                        .shadow_lg()
                                        .when(unread == 0, |button| {
                                            button
                                                .accessibility_label(tr(
                                                    language,
                                                    "sessions.jumpLatest",
                                                ))
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(tr(language, "sessions.jumpLatest")),
                                                )
                                        })
                                        .when(unread > 0, |button| {
                                            button
                                                .accessibility_label(
                                                    tr(language, "sessions.newMessages")
                                                        .replace("{count}", &unread.to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_2()
                                                        .text_size(px(12.))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(
                                                            div()
                                                                .debug_selector(|| {
                                                                    "new-messages-indicator".into()
                                                                })
                                                                .relative()
                                                                .size(px(16.))
                                                                .flex_none()
                                                                .child(
                                                                    div()
                                                                        .absolute()
                                                                        .rounded_full()
                                                                        .bg(cx
                                                                            .theme()
                                                                            .primary_foreground)
                                                                        .with_animation(
                                                                            "new-messages-ping",
                                                                            Animation::new(
                                                                                Duration::from_secs(
                                                                                    1,
                                                                                ),
                                                                            )
                                                                            .repeat()
                                                                            .with_easing(|t| {
                                                                                1. - (1. - t)
                                                                                    .powi(3)
                                                                            }),
                                                                            |dot, phase| {
                                                                                dot.size(px(
                                                                                    8. + phase * 8.
                                                                                ))
                                                                                .left(px(
                                                                                    4. - phase * 4.
                                                                                ))
                                                                                .top(px(
                                                                                    4. - phase * 4.
                                                                                ))
                                                                                .opacity(
                                                                                    0.75 * (1.
                                                                                        - phase),
                                                                                )
                                                                            },
                                                                        ),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .absolute()
                                                                        .left(px(4.))
                                                                        .top(px(4.))
                                                                        .size(px(8.))
                                                                        .rounded_full()
                                                                        .bg(cx
                                                                            .theme()
                                                                            .primary_foreground),
                                                                ),
                                                        )
                                                        .child(
                                                            tr(language, "sessions.newMessages")
                                                                .replace(
                                                                    "{count}",
                                                                    &unread.to_string(),
                                                                ),
                                                        ),
                                                )
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.unread_message_count = 0;
                                            this.conversation_state
                                                .update(cx, |state, cx| state.scroll_to_end(cx));
                                            cx.notify();
                                        })),
                                ),
                        )
                    }),
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
                cx.theme().button
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
                cx.theme().button
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
                cx.theme().button
            } else {
                cx.theme().background
            })
            .cursor_pointer()
            .hover(|style| {
                style
                    .border_color(cx.theme().list_active_border)
                    .bg(cx.theme().button)
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
        if let Some(error) = self.error.take() {
            // Root owns the notification layer; update it after this render completes.
            window.defer(cx, move |window, cx| {
                window.push_notification(Notification::error(error).id::<YesSessions>(), cx);
            });
        }
        if window.focused(cx).is_none() {
            self.root_focus.focus(window, cx);
        }
        div()
            .id("yes-sessions-root")
            .debug_selector(|| "yes-sessions-root".into())
            .track_focus(&self.root_focus)
            .on_action(
                cx.listener(|this, _: &crate::commands::FindInSession, window, cx| {
                    this.open_session_search(window, cx)
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.session_search.input.is_some()
                    && matches!(event.keystroke.key.as_str(), "up" | "down")
                {
                    this.step_session_search(event.keystroke.key == "up", cx);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.session_search.input.is_some() {
                    this.session_search = SessionSearch::default();
                    this.root_focus.focus(window, cx);
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.preview_open && !this.settings_open {
                    let returned = this.workspace_preview.as_ref().is_some_and(|preview| {
                        preview.update(cx, |preview, cx| preview.return_to_list(cx))
                    });
                    if !returned {
                        this.set_preview_open(false, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .relative()
            .v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(window, cx))
            .child(div().flex_1().min_h_0().flex().p_4().child({
                let detail = self.render_detail(window, cx);
                let sessions = if self.settings.sidebar_collapsed {
                    detail
                } else {
                    h_resizable("sessions-workspace")
                        .child(
                            resizable_panel()
                                .size(px(320.))
                                .size_range(px(160.)..px(960.))
                                .child(self.render_sidebar(cx)),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(280.)..px(4000.))
                                .child(detail),
                        )
                        .into_any_element()
                };
                if self.preview_open {
                    h_resizable("app-workspace-preview")
                        .child(
                            resizable_panel()
                                .size_range(
                                    px(if self.settings.sidebar_collapsed {
                                        280.
                                    } else {
                                        440.
                                    })..px(4000.),
                                )
                                .child(sessions),
                        )
                        .child(
                            resizable_panel()
                                .size(px(400.))
                                .size_range(px(320.)..px(2400.))
                                .child(self.render_workspace_preview(cx)),
                        )
                        .into_any_element()
                } else {
                    sessions
                }
            }))
            .when(
                self.session_search.input.is_some() && self.detail.is_some(),
                |view| view.child(self.render_session_search(cx)),
            )
            .when(self.settings_open, |view| {
                view.child(self.render_settings(cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{
        ancestor_session_ids, detail_source_signature, directory_group_labels, format_count,
        mermaid_sources_changed, navigator_preview, navigator_window, selection_after_refresh,
        session_directory_group_key, unread_after_refresh, update_conversation_scroll,
    };
    use yes_core::{AppType, MessageType, Session, SessionMessage, model::SessionKind};

    #[gpui_kit::test]
    fn returning_from_nested_subagents_restores_parent_scroll_and_expansion(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{px, size};
        use std::sync::Arc;
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(800.)), super::YesSessions::new);
        let app = window.root(cx).unwrap();
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.loading_sessions = false;
            app.selected_app = AppType::Claude;
            let parent = session("parent", None);
            let child = session("child", Some("parent"));
            let grandchild = session("grandchild", Some("child"));
            app.sessions = Arc::new(vec![
                parent.clone(),
                child.clone(),
                grandchild,
                session("sibling", Some("parent")),
            ]);
            app.selected_session_id = Some(parent.id.clone());
            let parent_detail = Arc::new(yes_core::SessionDetail {
                session: parent,
                messages: Vec::new(),
            });
            app.detail = Some(parent_detail.clone());
            app.conversation_state.update(cx, |state, cx| {
                state.reset(50, cx);
                state.scroll_to_item(23, cx);
            });
            let parent_scroll = app.conversation_state.clone();
            app.expanded_messages.insert(23);
            app.claude_xml_expanded.insert((23, 1), true);
            app.navigator_active_message = Some(23);
            app.unread_message_count = 4;
            app.select_session("child".into(), cx);
            assert_ne!(app.conversation_state, parent_scroll);
            assert_eq!(parent_scroll.read(cx).item_count(), 50);
            assert!(!parent_scroll.read(cx).is_following_tail());
            assert_eq!(app.parent_conversations.len(), 1);
            // Supply child content before the asynchronous load returns.
            app.detail = Some(Arc::new(yes_core::SessionDetail {
                session: child,
                messages: Vec::new(),
            }));
            app.loading_detail = false;
            app.conversation_state.update(cx, |state, cx| {
                state.reset(12, cx);
                state.scroll_to_item(7, cx);
            });
            let child_scroll = app.conversation_state.clone();
            app.expanded_messages.insert(7);
            app.open_sub_agent("grandchild", cx);
            assert_eq!(app.parent_conversations.len(), 2);
            app.open_sub_agent("child", cx);
            assert_eq!(app.conversation_state, child_scroll);
            assert_eq!(app.conversation_state.read(cx).item_count(), 12);
            assert!(app.expanded_messages.contains(&7));
            app.select_session("sibling".into(), cx);
            assert_eq!(app.parent_conversations.len(), 1);
            assert_eq!(app.parent_conversations[0].scroll, parent_scroll);
            app.open_sub_agent("parent", cx);
            assert_eq!(app.conversation_state, parent_scroll);
            assert!(Arc::ptr_eq(app.detail.as_ref().unwrap(), &parent_detail));
            assert!(!app.loading_detail);
            assert!(app.parent_conversations.is_empty());
            assert_eq!(app.expanded_messages, std::collections::HashSet::from([23]));
            assert_eq!(app.claude_xml_expanded.get(&(23, 1)), Some(&true));
            assert_eq!(app.navigator_active_message, Some(23));
            assert_eq!(app.unread_message_count, 4);
            app.open_sub_agent("child", cx);
            app.select_session("unrelated".into(), cx);
            assert!(app.parent_conversations.is_empty());
            // Invalidate outstanding fixture loads; they must not overwrite the restored state.
            app.detail_generation += 1;
        });
    }

    #[gpui_kit::test]
    fn claude_xml_cards_expand_and_collapse_in_message(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{px, size};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(800.)), super::YesSessions::new);
        let app = window.root(cx).unwrap();
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.loading_sessions = false;
            app.settings.sidebar_collapsed = true;
            app.settings_open = false;
            app.selected_app = AppType::Claude;
            let mut fixture = session("xml-test", None);
            fixture.app_type = AppType::Claude;
            app.detail = Some(std::sync::Arc::new(yes_core::SessionDetail {
                session: fixture,
                messages: vec![SessionMessage::text(MessageType::Assistant,
                    "2026-09-07T00:00:00Z",
                    "Before\n<path>/src/main.rs</path><type>file</type><content>fn main() {}</content>\nBetween\n<path>/src/</path><type>directory</type><entries>main.rs\nchild/</entries>\nAfter")],
            }));
            app.conversation_state.update(cx, |state, cx| state.reset(1, cx));
            cx.notify();
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        assert!(visual.debug_bounds("xml-body-0-1").is_some());
        assert!(visual.debug_bounds("xml-body-0-3").is_none());
        let target = visual.debug_bounds("xml-card-0-3").unwrap().center();
        visual.simulate_click(target, Default::default());
        cx.run_until_parked();
        assert!(visual.debug_bounds("xml-body-0-3").is_some());
        let target = visual.debug_bounds("xml-action-false").unwrap().center();
        visual.simulate_click(target, Default::default());
        cx.run_until_parked();
        assert!(visual.debug_bounds("xml-body-0-1").is_none());
        assert!(visual.debug_bounds("xml-body-0-3").is_none());
        let target = visual.debug_bounds("xml-action-true").unwrap().center();
        visual.simulate_click(target, Default::default());
        cx.run_until_parked();
        assert!(visual.debug_bounds("xml-body-0-1").is_some());
        assert!(visual.debug_bounds("xml-body-0-3").is_some());
        app.update(cx, |app, cx| {
            app.selected_app = AppType::CodeBuddy;
            app.conversation_state
                .update(cx, |state, cx| state.reset(1, cx));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(visual.debug_bounds("xml-card-0-1").is_none());
        app.update(cx, |app, cx| {
            app.selected_app = AppType::Claude;
            std::sync::Arc::make_mut(app.detail.as_mut().unwrap()).messages[0].message_type =
                MessageType::User;
            app.conversation_state
                .update(cx, |state, cx| state.reset(1, cx));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(visual.debug_bounds("xml-card-0-1").is_none());
        app.update(cx, |app, cx| {
            app.reset_detail(cx);
        });
        assert!(app.read_with(cx, |app, _| app.claude_xml_expanded.is_empty()));
    }

    #[gpui_kit::test]
    fn navigator_keeps_its_rail_stable_and_preview_reachable(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{ScrollDelta, ScrollWheelEvent, TouchPhase, point, px, size};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(600.)), super::YesSessions::new);
        let app = window.root(cx).unwrap();
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.loading_sessions = false;
            app.settings.sidebar_collapsed = true;
            app.settings_open = false;
            app.detail = Some(std::sync::Arc::new(yes_core::SessionDetail {
                session: session("navigator-test", None),
                messages: (0..50)
                    .flat_map(|i| {
                        [
                            SessionMessage::text(
                                MessageType::User,
                                "2026-09-07T00:00:00Z",
                                format!("Question {i}"),
                            ),
                            SessionMessage::text(
                                MessageType::Assistant,
                                "2026-09-07T00:00:00Z",
                                format!("Answer {i}"),
                            ),
                        ]
                    })
                    .collect(),
            }));
            app.conversation_state
                .update(cx, |state, cx| state.reset(50, cx));
            cx.notify();
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        let before = visual.debug_bounds("user-message-navigator").unwrap();
        let tick = visual.debug_bounds("navigator-tick-2").unwrap();
        let content = visual.debug_bounds("conversation-end").unwrap();
        assert!(
            content.right() <= tick.left() - px(12.),
            "content must stop before the navigation gutter"
        );

        visual.simulate_mouse_move(tick.center(), None, Default::default());
        cx.run_until_parked();
        assert_eq!(
            before,
            visual.debug_bounds("user-message-navigator").unwrap()
        );
        let card = visual.debug_bounds("navigator-preview-card").unwrap();
        assert!(card.left() >= px(0.));
        visual.simulate_mouse_move(
            point(tick.left() - px(4.), tick.center().y),
            None,
            Default::default(),
        );
        assert!(visual.debug_bounds("navigator-preview-card").is_some());
        visual.simulate_mouse_move(card.center(), None, Default::default());
        assert!(visual.debug_bounds("navigator-preview-card").is_some());
        visual.simulate_click(card.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(
            app.read_with(cx, |app, _| app.navigator_active_message),
            Some(4)
        );
        let tick = visual.debug_bounds("navigator-tick-2").unwrap();
        visual.simulate_mouse_move(tick.center(), None, Default::default());
        visual.simulate_event(ScrollWheelEvent {
            position: tick.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Started,
        });
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.navigator_start.unwrap_or_default()) > 0);
        assert_eq!(
            before,
            visual.debug_bounds("user-message-navigator").unwrap()
        );
        visual.simulate_event(ScrollWheelEvent {
            position: tick.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.navigator_start), Some(10));
        assert_eq!(
            app.read_with(cx, |app, _| app.navigator_active_message),
            Some(4)
        );
        visual.simulate_mouse_move(point(px(10.), px(100.)), None, Default::default());
        assert!(visual.debug_bounds("navigator-preview-card").is_none());
        app.update(cx, |app, cx| {
            app.conversation_state.update(cx, |state, cx| {
                state.scroll_to_item(20, cx);
            });
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            app.read_with(cx, |app, _| app.navigator_active_message),
            Some(40)
        );
        app.update(cx, |app, cx| {
            app.conversation_state.update(cx, |state, cx| {
                state.scroll_to_item(5, cx);
            });
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            app.read_with(cx, |app, _| app.navigator_active_message),
            Some(10)
        );
        visual.simulate_event(ScrollWheelEvent {
            position: point(px(400.), px(400.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-600.))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Started,
        });
        visual.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.navigator_active_message.unwrap()) > 10);

        app.update(cx, |app, cx| {
            app.reset_detail(cx);
            app.detail = Some(std::sync::Arc::new(yes_core::SessionDetail {
                session: session("single-message", None),
                messages: vec![SessionMessage::text(
                    MessageType::User,
                    "2026-09-07T00:00:00Z",
                    "One question",
                )],
            }));
            app.conversation_state
                .update(cx, |state, cx| state.reset(1, cx));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(visual.debug_bounds("user-message-navigator").is_none());
        let single_content = visual.debug_bounds("conversation-end").unwrap();
        assert_eq!(single_content.size.width, content.size.width + px(44.));
    }

    #[gpui_kit::test]
    fn tool_activity_collapses_expands_and_reveals_search_results(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{px, size};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(600.), px(600.)), super::YesSessions::new);
        let app = window.root(cx).unwrap();
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.loading_sessions = false;
            app.settings.sidebar_collapsed = true;
            app.settings_open = false;
            app.selected_app = AppType::Codex;
            app.settings.collapse_bash_blocks = true;
            let messages = (0..6)
                .map(|index| {
                    let mut message = SessionMessage::text(
                        if index % 2 == 0 {
                            MessageType::ToolUse
                        } else {
                            MessageType::ToolResult
                        },
                        "2026-09-09T08:05:00Z",
                        "",
                    );
                    message.tool_name = Some("Exec".into());
                    message.call_id = Some(format!("call-{}", index / 2));
                    message
                })
                .collect();
            app.detail = Some(std::sync::Arc::new(yes_core::SessionDetail {
                session: session("tools", None),
                messages,
            }));
            app.conversation_state
                .update(cx, |state, cx| state.reset(1, cx));
            cx.notify();
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        let group = visual.debug_bounds("tool-activity-0").unwrap();
        let preview = visual.debug_bounds("tool-activity-preview-0").unwrap();
        assert_eq!(preview.size.height, px(76.));
        assert!(visual.debug_bounds("tool-card-0").is_some());
        assert!(visual.debug_bounds("tool-card-4").is_none());
        visual.simulate_click(group.center(), Default::default());
        assert!(visual.debug_bounds("tool-card-4").is_some());
        assert!(
            visual
                .debug_bounds("tool-activity-preview-0")
                .unwrap()
                .size
                .height
                > preview.size.height
        );
        let collapse = visual.debug_bounds("tool-activity-0").unwrap();
        visual.simulate_click(collapse.center(), Default::default());
        assert!(visual.debug_bounds("tool-card-4").is_none());
        visual.simulate_click(preview.center(), Default::default());
        assert!(visual.debug_bounds("tool-card-4").is_some());
        let collapse = visual.debug_bounds("tool-activity-0").unwrap();
        visual.simulate_click(collapse.center(), Default::default());
        assert!(visual.debug_bounds("tool-card-4").is_none());
        app.update(cx, |app, cx| {
            app.search_landing = Some((5, std::time::Instant::now()));
            cx.notify();
        });
        assert!(visual.debug_bounds("tool-card-4").is_some());
        visual.simulate_resize(size(px(440.), px(600.)));
        assert!(visual.debug_bounds("tool-activity-0").unwrap().right() <= px(440.));
    }

    #[gpui_kit::test]
    fn action_errors_use_a_single_dismissible_toast(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{AppContext as _, px, size};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(800.), px(600.)), |window, cx| {
            let app = cx.new(|cx| super::YesSessions::new(window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        let root = window.root(cx).unwrap();
        let app = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<super::YesSessions>()
                .unwrap()
        });
        let notifications = root.read_with(cx, |root, _| root.notification.clone());
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        for message in ["First failure", "Next failure"] {
            app.update(cx, |app, cx| {
                app.sessions_generation += 1;
                app.loading_sessions = false;
                app.error = Some(message.into());
                cx.notify();
            });
            visual.debug_bounds("yes-sessions-root").unwrap();
            cx.run_until_parked();
            assert!(app.read_with(cx, |app, _| app.error.is_none()));
            assert_eq!(
                notifications.read_with(cx, |list, _| list.notifications().len()),
                1
            );
        }
        app.update(cx, |_, cx| cx.notify());
        visual.debug_bounds("yes-sessions-root").unwrap();
        cx.run_until_parked();
        assert_eq!(
            notifications.read_with(cx, |list, _| list.notifications().len()),
            1
        );
        cx.background_executor
            .advance_clock(std::time::Duration::from_secs(6));
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        assert!(notifications.read_with(cx, |list, _| list.notifications().is_empty()));
    }

    #[gpui_kit::test]
    fn released_text_selection_does_not_scroll_on_pointer_motion(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{AppContext as _, MouseButton, point, px, size};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(600.)), |window, cx| {
            let app = cx.new(|cx| super::YesSessions::new(window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        let app = window.root(cx).unwrap().read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<super::YesSessions>()
                .unwrap()
        });
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.loading_sessions = false;
            app.settings.sidebar_collapsed = true;
            app.settings_open = false;
            app.detail = Some(std::sync::Arc::new(yes_core::SessionDetail {
                session: session("selection-child", Some("parent")),
                messages: vec![SessionMessage::text(
                    MessageType::User,
                    "",
                    "Selectable message content for a scrolling regression.\n\n".repeat(80),
                )],
            }));
            app.conversation_state.update(cx, |state, cx| {
                state.reset(1, cx);
                state.scroll_to_item(0, cx);
            });
            cx.notify();
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        app.update(cx, |app, cx| {
            app.conversation_state.update(cx, |state, cx| {
                state.scroll_to_item(0, cx);
            });
        });
        visual.run_until_parked();
        let viewport = visual.debug_bounds("conversation-viewport").unwrap();
        let bubble = visual.debug_bounds("conversation-bubble-0").unwrap();
        let start = point(bubble.left() + px(24.), bubble.top() + px(36.));
        let edge = point(start.x, viewport.bottom() - px(2.));
        visual.simulate_click(start, Default::default());
        visual.simulate_mouse_move(edge, None, Default::default());
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(64));
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("conversation-bubble-0").unwrap().top(),
            bubble.top(),
            "a released click must not start edge autoscroll"
        );

        visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(edge, Some(MouseButton::Left), Default::default());
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(64));
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("conversation-bubble-0").unwrap().top() < bubble.top(),
            "dragging a text selection must still scroll at the edge"
        );
        visual.simulate_mouse_up(edge, MouseButton::Left, Default::default());
        visual.run_until_parked();
        let stopped = visual.debug_bounds("conversation-bubble-0").unwrap().top();
        visual.simulate_mouse_move(edge, None, Default::default());
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(64));
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("conversation-bubble-0").unwrap().top(),
            stopped
        );
    }

    #[gpui_kit::test]
    fn unread_button_keeps_count_through_layout_and_clears_on_click(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{px, size};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(600.)), super::YesSessions::new);
        let app = window.root(cx).unwrap();
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.loading_sessions = false;
            app.settings.sidebar_collapsed = true;
            app.settings_open = false;
            app.detail = Some(std::sync::Arc::new(yes_core::SessionDetail {
                session: session("unread-test", None),
                messages: (0..30)
                    .map(|i| {
                        SessionMessage::text(
                            MessageType::User,
                            "2026-09-07T00:00:00Z",
                            format!("Message {i}\n{}", "History content\n".repeat(10)),
                        )
                    })
                    .collect(),
            }));
            app.conversation_state.update(cx, |state, cx| {
                state.reset(30, cx);
                state.scroll_to_end(cx);
            });
            cx.notify();
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        visual.debug_bounds("session-detail-title").unwrap();
        assert!(visual.debug_bounds("conversation-end").is_some());
        // Scroll state alone must update the parent overlay, without unread messages
        // or an explicit notification on the app entity.
        app.update(cx, |app, cx| {
            app.conversation_state.update(cx, |state, cx| {
                state.scroll_to_item(0, cx);
            });
        });
        cx.run_until_parked();
        assert!(visual.debug_bounds("conversation-jump-latest").is_some());
        assert!(visual.debug_bounds("conversation-end").is_none());
        app.update(cx, |app, cx| {
            app.conversation_state
                .update(cx, |state, cx| state.scroll_to_end(cx));
        });
        cx.run_until_parked();
        assert!(visual.debug_bounds("conversation-jump-latest").is_none());
        app.update(cx, |app, cx| {
            app.conversation_state.update(cx, |state, cx| {
                state.scroll_to_item(0, cx);
            });
            app.unread_message_count = 3;
            app.conversation_state.update(cx, |state, cx| {
                state.remeasure_items(0..30, cx);
            });
            cx.notify();
        });
        cx.run_until_parked();
        visual.debug_bounds("session-detail-title").unwrap();
        app.read_with(cx, |app, cx| {
            assert!(
                app.conversation_state.read(cx).is_scrolled_up(),
                "expected history position; following={}, count={}",
                app.conversation_state.read(cx).is_following_tail(),
                app.unread_message_count
            );
        });
        let button = visual.debug_bounds("conversation-jump-latest").unwrap();
        assert!(button.size.width > px(80.));
        assert_eq!(app.read_with(cx, |app, _| app.unread_message_count), 3);
        visual.simulate_click(button.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.unread_message_count), 0);
        assert!(visual.debug_bounds("conversation-end").is_some());
    }

    #[gpui_kit::test]
    fn live_updates_preserve_manual_scroll_and_follow_only_at_tail(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::message_scroller::MessageScrollerState;
        let state = cx.new(|cx| MessageScrollerState::new(10, cx));
        state.update(cx, |state, cx| {
            state.scroll_to_item(2, cx);
            assert!(!state.is_following_tail());
            update_conversation_scroll(state, 10, 12, cx);
            assert_eq!(state.item_count(), 12);
            assert!(!state.is_following_tail());
            update_conversation_scroll(state, 12, 12, cx);
            assert!(!state.is_following_tail());
            update_conversation_scroll(state, 12, 8, cx);
            assert!(!state.is_following_tail());
            state.scroll_to_end(cx);
            update_conversation_scroll(state, 8, 9, cx);
            assert!(state.is_following_tail());
        });
    }

    #[test]
    fn unread_counts_messages_only_while_reading_history() {
        assert_eq!(unread_after_refresh(0, 10, 12, true), 2);
        assert_eq!(unread_after_refresh(2, 12, 15, true), 5);
        // Streaming an existing message and a compacted history add no messages.
        assert_eq!(unread_after_refresh(5, 15, 15, true), 5);
        assert_eq!(unread_after_refresh(5, 15, 10, true), 5);
        assert_eq!(unread_after_refresh(5, 15, 16, false), 0);
    }

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
    fn detail_directory_keeps_last_three_levels() {
        for (path, expected) in [
            (
                "/Users/krabswang/Workspaces/gc-homepage",
                "…/krabswang/Workspaces/gc-homepage",
            ),
            ("/a/b/c", "/a/b/c"),
            ("a/b/c/d", "…/b/c/d"),
            ("/项目/工作区/会话/", "/项目/工作区/会话/"),
            ("/", "/"),
        ] {
            assert_eq!(
                super::detail_directory_label(std::path::Path::new(path)),
                expected
            );
        }
    }

    #[gpui_kit::test]
    fn detail_title_wraps_as_available_width_shrinks(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{AppContext as _, px, size};
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1200.), px(700.)), |window, cx| {
            let app = cx.new(|cx| super::YesSessions::new(window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        let root = window.root(cx).unwrap();
        let notifications = root.read_with(cx, |root, _| root.notification.clone());
        let app = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<super::YesSessions>()
                .unwrap()
        });
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.loading_sessions = false;
            app.settings.sidebar_collapsed = true;
            app.settings_open = false;
            let mut item = session("title-wrap", Some("parent"));
            item.agent_type = Some("Plan".into());
            item.directory = Some(PathBuf::from("/tmp/a-long-workspace-directory-for-header-layout"));
            item.first_message = "检查文件预览和差异对比面板的布局，确保缩小会话区域后标题能够自动换行并完整显示。 Review the workspace preview layout and preserve the full session title.".into();
            app.detail = Some(std::sync::Arc::new(yes_core::SessionDetail { session: item, messages: vec![] }));
            cx.notify();
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        let wide = visual.debug_bounds("session-detail-title").unwrap();
        visual.simulate_resize(size(px(440.), px(700.)));
        let narrow = visual.debug_bounds("session-detail-title").unwrap();
        assert!(narrow.size.height > wide.size.height);
        assert!(narrow.right() <= px(440.));
        let back = visual.debug_bounds("back-to-parent").unwrap();
        assert_eq!(back.left(), px(440.) - narrow.right());
        assert_eq!(back.top(), narrow.top());
        assert_eq!(back.size.width, px(24.));
        assert!(back.right() < narrow.left());
        let toolbar = visual.debug_bounds("session-metadata-toolbar").unwrap();
        assert_eq!(toolbar.size.height, px(24.));
        assert!(toolbar.right() <= px(440.));
        let badge = visual.debug_bounds("session-agent-badge").unwrap();
        let directory = visual.debug_bounds("copy-session-directory").unwrap();
        let date = visual.debug_bounds("session-updated-date").unwrap();
        assert!(toolbar.top() >= narrow.bottom());
        assert_eq!(badge.top(), toolbar.top());
        assert!(directory.left() >= badge.right());
        assert!(date.left() >= directory.right());
        assert!(date.right() <= toolbar.right());
        cx.update(|cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("unchanged".into()))
        });
        visual.simulate_click(date.center(), Default::default());
        assert_eq!(
            cx.update(|cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            "unchanged"
        );
        assert!(app.read_with(cx, |app, _| app.copied_metadata.is_none()));
        assert!(notifications.read_with(cx, |list, _| list.notifications().is_empty()));
        for (id, expected) in [
            ("copy-session-id", "title-wrap".to_owned()),
            (
                "copy-session-directory",
                "/tmp/a-long-workspace-directory-for-header-layout".to_owned(),
            ),
        ] {
            let button = visual.debug_bounds(id).unwrap();
            assert!(button.right() <= toolbar.right());
            if id == "copy-session-id" {
                assert_eq!(button.top(), narrow.top());
                assert!(((narrow.left() - button.right()) - px(4.)).abs() <= px(1.));
                assert!(button.left() >= back.right());
            } else {
                assert_eq!(button.top(), toolbar.top());
            }
            visual.simulate_click(button.center(), Default::default());
            assert_eq!(
                cx.update(|cx| cx.read_from_clipboard().unwrap().text().unwrap()),
                expected
            );
            assert_eq!(
                app.read_with(cx, |app, _| app.copied_metadata.map(|(id, _)| id)),
                Some(id)
            );
            assert_eq!(visual.debug_bounds(id).unwrap().size, button.size);
            assert_eq!(
                notifications.read_with(cx, |list, _| list.notifications().len()),
                1
            );
        }
    }

    #[gpui_kit::test]
    fn group_toolbar_clicks_do_not_toggle_the_first_group(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{px, size};
        let provider_root = std::env::temp_dir().join(format!(
            "yes-sessions-group-toolbar-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(provider_root.join("projects")).unwrap();
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1100.), px(700.)), super::YesSessions::new);
        let app = window.root(cx).unwrap();
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.loading_sessions = false;
            app.settings.sidebar_collapsed = false;
            app.selected_app = AppType::Claude;
            // Availability must not depend on CLI data installed on the test host.
            let mut registry = yes_core::providers::ProviderRegistry::default();
            registry.register(std::sync::Arc::new(
                yes_core::providers::ClaudeProvider::with_root(provider_root.clone()),
            ));
            app.registry = std::sync::Arc::new(registry);
            cx.notify();
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        for mode in [
            super::SessionViewMode::Directory,
            super::SessionViewMode::Date,
        ] {
            for count in [1, 2] {
                app.update(cx, |app, cx| {
                    app.session_view_mode = mode;
                    app.sessions = std::sync::Arc::new(
                        (0..count)
                            .map(|index| {
                                let mut item = session(&format!("item-{index}"), None);
                                item.directory =
                                    Some(PathBuf::from(format!("/workspace/gamecenter-pc-official-account-with-long-name-{index}")));
                                item.updated_at = 1_700_000_000_000 + index * 86_400_000;
                                item
                            })
                            .collect(),
                    );
                    app.collapsed_groups.clear();
                    cx.notify();
                });
                let collapse = visual.debug_bounds("collapse-all-groups").unwrap();
                let header = visual.debug_bounds("session-group-0").unwrap();
                assert_eq!(collapse.center().y, header.center().y);
                let sidebar = visual.debug_bounds("sessions-sidebar").unwrap();
                assert!(header.right() <= sidebar.right());
                assert!(collapse.right() <= sidebar.right() - px(16.));
                assert!((header.right() - collapse.right() - px(16.)).abs() <= px(1.));
                visual.simulate_click(collapse.center(), Default::default());
                app.read_with(cx, |app, _| {
                    assert_eq!(app.session_rows().len(), count as usize);
                    assert_eq!(app.collapsed_groups.len(), count as usize);
                });
                let expand = visual.debug_bounds("expand-all-groups").unwrap();
                visual.simulate_click(expand.center(), Default::default());
                app.read_with(cx, |app, _| {
                    assert!(app.collapsed_groups.is_empty());
                    assert_eq!(app.session_rows().len(), count as usize * 2);
                });
            }
        }
        fs::remove_dir_all(provider_root).unwrap();
    }

    #[test]
    fn directory_labels_remove_shared_workspace_and_keep_nearest_parents() {
        let paths = [
            "/Users/me/Workspaces/client/app",
            "/Users/me/Workspaces/server/app",
            "/Users/me/Workspaces/project",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let root = super::directory_group_root(&paths).unwrap();
        assert_eq!(root, PathBuf::from("/Users/me/Workspaces"));
        assert_eq!(
            directory_group_labels("/Users/me/Workspaces/client/app", Some(&root)),
            ("app".into(), Some("client".into()))
        );
        assert_eq!(
            directory_group_labels("/Users/me/Workspaces/server/app", Some(&root)),
            ("app".into(), Some("server".into()))
        );
        assert_eq!(
            directory_group_labels("/Users/me/Workspaces/project", Some(&root)),
            ("project".into(), None)
        );
        assert_eq!(
            directory_group_labels("/Users/me/Workspaces/a/b/父目录/project", Some(&root)),
            ("project".into(), Some("b/父目录".into()))
        );
        assert_eq!(
            directory_group_labels("project", None),
            ("project".into(), None)
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
