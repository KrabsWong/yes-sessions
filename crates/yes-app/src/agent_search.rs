use super::*;
use crate::date_time_range::{DateTimeRangeEvent, DateTimeRangePicker};
use gpui_kit::component::{
    Sizable,
    button::ButtonCustomVariant,
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    popover::Popover,
    scroll::{Scrollbar, ScrollbarMode},
};
use yes_core::search::{
    SearchFilter, SearchHit, SearchScope, match_range, search_messages, search_session,
    search_session_metadata,
};

const RESULT_LIMIT: usize = 1000;

#[derive(Clone)]
struct AgentSearchHit {
    session: Arc<Session>,
    hit: Option<SearchHit>,
    excerpt: String,
}

#[derive(Default)]
pub(super) struct AgentSearch {
    pub(super) input: Option<Entity<InputState>>,
    time_range: Option<Entity<DateTimeRangePicker>>,
    directory_query: Option<Entity<InputState>>,
    directories: Vec<PathBuf>,
    selected_directories: HashSet<PathBuf>,
    include_subdirectories: bool,
    directories_open: bool,
    time_open: bool,
    source_open: bool,
    expanded_sessions: HashSet<String>,
    directory_scroll: UniformListScrollHandle,
    scope: SearchScope,
    hits: Vec<AgentSearchHit>,
    active: usize,
    scroll: UniformListScrollHandle,
    pending: bool,
    submitted: bool,
    draft_query: String,
    scanned: usize,
    total: usize,
    failures: usize,
    limited: bool,
    error: Option<&'static str>,
    task: Option<Task<()>>,
}

pub(super) struct AgentSearchLanding {
    session_id: String,
    query: String,
    scope: SearchScope,
    hit: SearchHit,
}

fn session_title(session: &Session) -> String {
    if session.first_message.is_empty() {
        session.file_name.clone()
    } else {
        session.first_message.clone()
    }
}

impl AgentSearch {
    pub(super) fn filter_open(&self) -> bool {
        self.time_open || self.directories_open || self.source_open
    }

    fn visible_hits(&self) -> Vec<usize> {
        self.hits
            .iter()
            .enumerate()
            .filter_map(|(index, hit)| {
                (index == 0
                    || self.hits[index - 1].session.id != hit.session.id
                    || self.expanded_sessions.contains(&hit.session.id))
                .then_some(index)
            })
            .collect()
    }
}

impl YesSessions {
    pub(super) fn open_agent_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        self.session_search = SessionSearch::default();
        if self.agent_search.input.is_none() {
            let lang = self.settings.language;
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder(tr(lang, "agentSearch.placeholder"))
            });
            let time_range = cx.new(|cx| DateTimeRangePicker::new(lang, window, cx));
            let directory_query = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(tr(lang, "agentSearch.directoryPlaceholder"))
            });
            cx.subscribe_in(
                &input,
                window,
                |this, source, event: &InputEvent, window, cx| {
                    if this.agent_search.input.as_ref() != Some(source) {
                        return;
                    }
                    match event {
                        InputEvent::Change => {
                            let query = source.read(cx).value().to_string();
                            if query != this.agent_search.draft_query {
                                this.invalidate_agent_search(cx);
                            }
                        }
                        InputEvent::PressEnter { .. } => {
                            if this.agent_search.submitted {
                                this.accept_agent_search(window, cx);
                            } else {
                                this.run_agent_search(cx);
                            }
                        }
                        _ => {}
                    }
                },
            )
            .detach();
            cx.subscribe(&time_range, |this, source, _: &DateTimeRangeEvent, cx| {
                if this.agent_search.time_range.as_ref() == Some(&source) {
                    if !source.read(cx).custom_open {
                        this.agent_search.time_open = false;
                    }
                    this.invalidate_agent_search(cx);
                }
            })
            .detach();
            cx.subscribe(&directory_query, |this, source, _: &InputEvent, cx| {
                if this.agent_search.directory_query.as_ref() != Some(&source) {
                    return;
                }
                this.agent_search.directory_scroll = UniformListScrollHandle::new();
                cx.notify();
            })
            .detach();
            self.agent_search = AgentSearch {
                input: Some(input),
                time_range: Some(time_range),
                directory_query: Some(directory_query),
                ..Default::default()
            };
            self.sync_agent_search_directories();
        }
        self.agent_search
            .input
            .as_ref()
            .unwrap()
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    pub(super) fn close_agent_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_search = AgentSearch::default();
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn agent_search_query_focused(&self, window: &Window, cx: &App) -> bool {
        self.agent_search
            .input
            .as_ref()
            .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    pub(super) fn sync_agent_search_directories(&mut self) {
        if self.agent_search.input.is_none() {
            return;
        }
        let mut directories = self
            .sessions
            .iter()
            .filter_map(|s| s.directory.clone())
            .flat_map(|path| {
                path.ancestors()
                    .filter(|p| p.is_absolute())
                    .map(|p| p.to_path_buf())
                    .collect::<Vec<_>>()
            })
            .chain(self.agent_search.selected_directories.iter().cloned())
            .collect::<Vec<_>>();
        let working_directories = self
            .sessions
            .iter()
            .filter_map(|session| session.directory.as_ref())
            .collect::<HashSet<_>>();
        directories.sort_by(|a, b| {
            working_directories
                .contains(b)
                .cmp(&working_directories.contains(a))
                .then_with(|| b.components().count().cmp(&a.components().count()))
                .then_with(|| a.cmp(b))
        });
        directories.dedup();
        self.agent_search.directories = directories;
    }

    fn invalidate_agent_search(&mut self, cx: &mut Context<Self>) {
        let search = &mut self.agent_search;
        // Cancel the previous scan and discard results belonging to the old conditions.
        search.task = None;
        search.hits.clear();
        search.expanded_sessions.clear();
        search.active = 0;
        search.scroll = UniformListScrollHandle::new();
        search.pending = false;
        search.submitted = false;
        search.scanned = 0;
        search.total = 0;
        search.failures = 0;
        search.limited = false;
        search.error = None;
        search.draft_query = search
            .input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        if search
            .input
            .as_ref()
            .is_some_and(|input| input.read(cx).value().trim().is_empty())
        {
            search.scope = SearchScope::All;
        }
        cx.notify();
    }

    fn run_agent_search(&mut self, cx: &mut Context<Self>) {
        if self.loading_sessions {
            return;
        }
        self.invalidate_agent_search(cx);
        self.sync_agent_search_directories();
        let search = &mut self.agent_search;
        let Some(input) = &search.input else { return };
        let query = input.read(cx).value().trim().to_owned();
        search.submitted = true;
        let filter = search.time_range.as_ref().unwrap().read(cx).bounds(cx).map(
            |(updated_from, updated_until)| SearchFilter {
                updated_from,
                updated_until,
                directories: search.selected_directories.iter().cloned().collect(),
                include_subdirectories: search.include_subdirectories,
            },
        );
        let filter = match filter {
            Ok(filter) => filter,
            Err(error) => {
                search.error = Some(error);
                cx.notify();
                return;
            }
        };
        let mut sessions = self
            .sessions
            .iter()
            .filter(|session| filter.matches(session))
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        search.total = sessions.len();
        let Some(provider) = self.registry.get(self.selected_app) else {
            return;
        };
        let scope = search.scope;
        search.pending = true;
        search.task = Some(cx.spawn(async move |this, cx| {
            for session in sessions {
                let provider = provider.clone();
                let query = query.clone();
                let empty_query = query.is_empty();
                let (session, result, metadata) = cx
                    .background_executor()
                    .spawn(async move {
                        let metadata = search_session_metadata(&session, &query);
                        let result = if query.is_empty() {
                            Ok(Vec::new())
                        } else {
                            search_session(provider.as_ref(), &session, &query, scope)
                        };
                        (session, result, metadata)
                    })
                    .await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        let search = &mut this.agent_search;
                        search.scanned += 1;
                        let session = Arc::new(session);
                        match result {
                            Ok(hits) if !hits.is_empty() => {
                                let available = RESULT_LIMIT - search.hits.len();
                                search.limited |= hits.len() > available;
                                search
                                    .hits
                                    .extend(hits.into_iter().take(available).map(|hit| {
                                        AgentSearchHit {
                                            excerpt: hit.excerpt.clone(),
                                            session: session.clone(),
                                            hit: Some(hit),
                                        }
                                    }));
                            }
                            result => {
                                if result.is_err() {
                                    search.failures += 1;
                                }
                                if empty_query || (scope == SearchScope::All && metadata.is_some())
                                {
                                    search.hits.push(AgentSearchHit {
                                        excerpt: metadata
                                            .unwrap_or_else(|| session_title(&session)),
                                        session,
                                        hit: None,
                                    });
                                }
                            }
                        }
                        let stop = search.hits.len() >= RESULT_LIMIT;
                        search.limited |= stop && search.scanned < search.total;
                        cx.notify();
                        !stop
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.agent_search.pending = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn step_agent_search(&mut self, previous: bool, cx: &mut Context<Self>) {
        let search = &mut self.agent_search;
        if search.hits.is_empty() {
            return;
        }
        let visible = search.visible_hits();
        let position = visible
            .iter()
            .position(|index| *index == search.active)
            .unwrap_or(0);
        let position = (position + if previous { visible.len() - 1 } else { 1 }) % visible.len();
        search.active = visible[position];
        search
            .scroll
            .scroll_to_item(position, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn accept_agent_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(result) = self
            .agent_search
            .hits
            .get(self.agent_search.active)
            .cloned()
        else {
            return;
        };
        let landing = result.hit.map(|hit| AgentSearchLanding {
            session_id: result.session.id.clone(),
            query: self
                .agent_search
                .input
                .as_ref()
                .unwrap()
                .read(cx)
                .value()
                .to_string(),
            scope: self.agent_search.scope,
            hit,
        });
        self.close_agent_search(window, cx);
        // Use the normal loader with a fresh conversation, also for subagent results.
        self.parent_conversations.clear();
        self.collapsed_groups
            .remove(&self.session_group_key(&result.session));
        self.load_session(result.session.id.clone(), cx);
        self.agent_search_landing = landing;
    }

    pub(super) fn finish_agent_search_landing(&mut self, cx: &mut Context<Self>) {
        let Some(landing) = self.agent_search_landing.take() else {
            return;
        };
        let Some(detail) = self.detail.clone() else {
            return;
        };
        if detail.session.id != landing.session_id {
            return;
        }
        let generation = self.detail_generation;
        let source = detail.clone();
        // Revalidate fresh content in the background; large conversations must not block UI.
        let task = cx.background_executor().spawn(async move {
            let hits = search_messages(&detail.messages, &landing.query, landing.scope);
            hits.iter()
                .find(|hit| {
                    hit.message == landing.hit.message && hit.excerpt == landing.hit.excerpt
                })
                .or_else(|| hits.iter().find(|hit| hit.excerpt == landing.hit.excerpt))
                .or_else(|| hits.first())
                .map(|hit| hit.message)
        });
        cx.spawn(async move |this, cx| {
            let message = task.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.detail_generation {
                    return;
                }
                if this
                    .detail
                    .as_ref()
                    .is_some_and(|detail| Arc::ptr_eq(detail, &source))
                {
                    if let Some(message) = message {
                        this.jump_to_search_message(message, cx);
                        return;
                    }
                }
                this.error = Some(tr(this.settings.language, "agentSearch.changed").into());
                cx.notify();
            });
        })
        .detach();
    }

    fn render_agent_directories(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.settings.language;
        let query = self
            .agent_search
            .directory_query
            .as_ref()
            .unwrap()
            .read(cx)
            .value()
            .to_lowercase();
        let directories = self
            .agent_search
            .directories
            .iter()
            .filter(|p| p.to_string_lossy().to_lowercase().contains(&query))
            .cloned()
            .collect::<Vec<_>>();
        let count = directories.len();
        div()
            .flex_none()
            .v_flex()
            .gap_2()
            .child(
                Button::new("agent-search-all-directories")
                    .small()
                    .ghost()
                    .text_color(cx.theme().foreground)
                    .accessibility_label(tr(lang, "agentSearch.allDirectories"))
                    .child(div().w_full().child(tr(lang, "agentSearch.allDirectories")))
                    .selected(self.agent_search.selected_directories.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.agent_search.selected_directories.is_empty() {
                            this.agent_search.selected_directories.clear();
                            this.invalidate_agent_search(cx);
                        }
                    })),
            )
            .child(Input::new(
                self.agent_search.directory_query.as_ref().unwrap(),
            ))
            .child(
                Checkbox::new("agent-search-subdirectories")
                    .debug_selector(|| "agent-search-subdirectories".into())
                    .label(tr(lang, "agentSearch.subdirectories"))
                    .checked(self.agent_search.include_subdirectories)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.agent_search.include_subdirectories = *checked;
                        this.invalidate_agent_search(cx);
                    })),
            )
            .child(
                div()
                    .relative()
                    .h(px((count.max(1).min(4) * 48) as f32))
                    .child(
                        uniform_list(
                            "agent-search-directories",
                            count,
                            cx.processor(move |this, range: Range<usize>, _, cx| {
                                range
                                    .map(|index| {
                                        let path = directories[index].clone();
                                        let checked =
                                            this.agent_search.selected_directories.contains(&path);
                                        let text_path = path.clone();
                                        let tooltip_path = path.to_string_lossy().into_owned();
                                        div()
                                            .h(px(48.))
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .pr_3()
                                            .overflow_hidden()
                                            .child(
                                                Checkbox::new(("agent-search-directory", index))
                                                    .debug_selector(move || {
                                                        format!("agent-search-directory-{index}")
                                                            .into()
                                                    })
                                                    .checked(checked)
                                                    .accessibility_label(
                                                        path.to_string_lossy().into_owned(),
                                                    )
                                                    .tooltip(path.to_string_lossy().into_owned())
                                                    .on_click(cx.listener(
                                                        move |this, checked: &bool, _, cx| {
                                                            if *checked {
                                                                this.agent_search
                                                                    .selected_directories
                                                                    .insert(path.clone());
                                                            } else {
                                                                this.agent_search
                                                                    .selected_directories
                                                                    .remove(&path);
                                                            }
                                                            this.invalidate_agent_search(cx);
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .id(("directory-label", index))
                                                    .flex_1()
                                                    .min_w_0()
                                                    .cursor_pointer()
                                                    .tooltip(move |window, cx| {
                                                        Tooltip::new(tooltip_path.clone())
                                                            .build(window, cx)
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        if !this
                                                            .agent_search
                                                            .selected_directories
                                                            .remove(&text_path)
                                                        {
                                                            this.agent_search
                                                                .selected_directories
                                                                .insert(text_path.clone());
                                                        }
                                                        this.invalidate_agent_search(cx);
                                                    }))
                                                    .v_flex()
                                                    .child(
                                                        div().text_sm().truncate().child(
                                                            directories[index]
                                                                .file_name()
                                                                .unwrap_or(
                                                                    directories[index].as_os_str(),
                                                                )
                                                                .to_string_lossy()
                                                                .into_owned(),
                                                        ),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(11.))
                                                            .text_color(cx.theme().muted_foreground)
                                                            .truncate()
                                                            .child(
                                                                directories[index]
                                                                    .parent()
                                                                    .map(|p| {
                                                                        p.to_string_lossy()
                                                                            .into_owned()
                                                                    })
                                                                    .unwrap_or_default(),
                                                            ),
                                                    ),
                                            )
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.agent_search.directory_scroll)
                        .size_full(),
                    )
                    .child(
                        Scrollbar::vertical(&self.agent_search.directory_scroll)
                            .mode(ScrollbarMode::Always),
                    ),
            )
            .into_any_element()
    }

    fn render_agent_filters(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.settings.language;
        let picker = self.agent_search.time_range.as_ref().unwrap().clone();
        let time_label = if picker.read(cx).has_value(cx) {
            picker.read(cx).summary(cx)
        } else {
            tr(lang, "agentSearch.time").to_owned()
        };
        let owner = cx.entity().downgrade();
        let directory_owner = owner.clone();
        let source_owner = owner.clone();
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .pb_3()
            .child(
                Popover::new("search-time-popover")
                    .open(self.agent_search.time_open)
                    .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                        this.agent_search.time_open = *open;
                        if *open {
                            this.agent_search.directories_open = false;
                            this.agent_search.source_open = false;
                        }
                        cx.notify();
                    }))
                    .trigger(
                        Button::new("agent-search-time")
                            .small()
                            .icon(IconName::Calendar)
                            .debug_selector(|| "agent-search-time".into())
                            .label(time_label)
                            .tooltip(picker.read(cx).description(cx))
                            .selected(picker.read(cx).has_value(cx)),
                    )
                    .content(move |_, _, _| picker.clone()),
            )
            .child(
                Popover::new("search-directory-popover")
                    .open(self.agent_search.directories_open)
                    .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                        this.agent_search.directories_open = *open;
                        if *open {
                            this.agent_search.time_open = false;
                            this.agent_search.source_open = false;
                        }
                        cx.notify();
                    }))
                    .trigger(
                        Button::new("agent-search-folders")
                            .small()
                            .icon(IconName::Folder)
                            .debug_selector(|| "agent-search-folders".into())
                            .label(if self.agent_search.selected_directories.is_empty() {
                                tr(lang, "agentSearch.directories").to_owned()
                            } else {
                                format!(
                                    "{} · {}",
                                    tr(lang, "agentSearch.directories"),
                                    self.agent_search.selected_directories.len()
                                )
                            })
                            .selected(!self.agent_search.selected_directories.is_empty()),
                    )
                    .content(move |_, _, cx| {
                        directory_owner
                            .update(cx, |this, cx| {
                                div()
                                    .w(px(480.))
                                    .max_w(relative(0.9))
                                    .child(this.render_agent_directories(cx))
                                    .into_any_element()
                            })
                            .unwrap_or_else(|_| div().into_any_element())
                    }),
            )
            .child(
                Popover::new("search-source-popover")
                    .open(self.agent_search.source_open)
                    .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                        this.agent_search.source_open = *open;
                        if *open {
                            this.agent_search.time_open = false;
                            this.agent_search.directories_open = false;
                        }
                        cx.notify();
                    }))
                    .trigger(
                        Button::new("agent-search-source")
                            .small()
                            .icon(IconName::User)
                            .debug_selector(|| "agent-search-source".into())
                            .label(tr(
                                lang,
                                match self.agent_search.scope {
                                    SearchScope::All => "agentSearch.source",
                                    SearchScope::User => "search.filterUser",
                                    SearchScope::Assistant => "search.filterAssistant",
                                },
                            ))
                            .selected(self.agent_search.scope != SearchScope::All),
                    )
                    .content(move |_, _, cx| {
                        source_owner
                            .update(cx, |this, cx| {
                                div()
                                    .w(px(200.))
                                    .v_flex()
                                    .gap_1()
                                    .children(
                                        [
                                            (SearchScope::All, "search.filterAll"),
                                            (SearchScope::User, "search.filterUser"),
                                            (SearchScope::Assistant, "search.filterAssistant"),
                                        ]
                                        .into_iter()
                                        .map(
                                            |(scope, key)| {
                                                Button::new(key)
                                                    .ghost()
                                                    .small()
                                                    .debug_selector(move || {
                                                        format!("agent-{key}").into()
                                                    })
                                                    .when(
                                                        scope == SearchScope::All
                                                            || !this
                                                                .agent_search
                                                                .draft_query
                                                                .trim()
                                                                .is_empty(),
                                                        |button| {
                                                            button.text_color(cx.theme().foreground)
                                                        },
                                                    )
                                                    .accessibility_label(tr(lang, key))
                                                    .child(div().w_full().child(tr(lang, key)))
                                                    .selected(this.agent_search.scope == scope)
                                                    .disabled(
                                                        scope != SearchScope::All
                                                            && this
                                                                .agent_search
                                                                .input
                                                                .as_ref()
                                                                .unwrap()
                                                                .read(cx)
                                                                .value()
                                                                .trim()
                                                                .is_empty(),
                                                    )
                                                    .tooltip(tr(lang, "agentSearch.roleHint"))
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            if this.agent_search.scope != scope {
                                                                this.agent_search.scope = scope;
                                                                this.invalidate_agent_search(cx);
                                                            }
                                                            this.agent_search.source_open = false;
                                                            cx.notify();
                                                            this.agent_search
                                                                .input
                                                                .as_ref()
                                                                .unwrap()
                                                                .read(cx)
                                                                .focus_handle(cx)
                                                                .focus(window, cx);
                                                        },
                                                    ))
                                            },
                                        ),
                                    )
                                    .into_any_element()
                            })
                            .unwrap_or_else(|_| div().into_any_element())
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_agent_search(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.settings.language;
        self.agent_search
            .time_range
            .as_ref()
            .unwrap()
            .update(cx, |picker, cx| picker.set_language(lang, cx));
        let search = &self.agent_search;
        let count = search.hits.len();
        let has_filters = !search.selected_directories.is_empty()
            || search.time_range.as_ref().unwrap().read(cx).has_value(cx);
        let status = if let Some(error) = search.error {
            tr(lang, error).to_owned()
        } else if !search.submitted {
            tr(lang, "agentSearch.ready").to_owned()
        } else if search.pending {
            format!(
                "{} {}/{} · {} {}",
                tr(lang, "search.searching"),
                search.scanned,
                search.total,
                count,
                tr(lang, "agentSearch.results")
            )
        } else if count == 0 {
            tr(lang, "agentSearch.empty").to_owned()
        } else {
            let sessions = search
                .hits
                .iter()
                .map(|hit| &hit.session.id)
                .collect::<HashSet<_>>()
                .len();
            let messages = search.hits.iter().filter(|hit| hit.hit.is_some()).count();
            if search
                .input
                .as_ref()
                .unwrap()
                .read(cx)
                .value()
                .trim()
                .is_empty()
            {
                format!("{} {}", sessions, tr(lang, "agentSearch.sessionCount"))
            } else {
                format!(
                    "{} {} · {} {}",
                    sessions,
                    tr(lang, "agentSearch.sessionCount"),
                    messages,
                    tr(lang, "search.matches")
                )
            }
        };
        let visible = self.agent_search.visible_hits();
        let results = uniform_list(
            "agent-search-results",
            visible.len(),
            cx.processor(move |this, range: Range<usize>, _, cx| {
                let query = this
                    .agent_search
                    .input
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .value()
                    .trim()
                    .to_lowercase();
                range
                    .map(|row| {
                        let index = visible[row];
                        let result = &this.agent_search.hits[index];
                        let header = index == 0
                            || this.agent_search.hits[index - 1].session.id != result.session.id;
                        let group_count = this
                            .agent_search
                            .hits
                            .iter()
                            .filter(|hit| hit.session.id == result.session.id)
                            .count();
                        let group_id = result.session.id.clone();
                        let expanded = this.agent_search.expanded_sessions.contains(&group_id);
                        let source = tr(
                            this.settings.language,
                            match result.hit.as_ref().map(|hit| hit.message_type) {
                                Some(yes_core::MessageType::User) => "search.filterUser",
                                Some(yes_core::MessageType::Assistant) => "search.filterAssistant",
                                Some(_) => "agentSearch.toolContent",
                                None => "agentSearch.sessionInfo",
                            },
                        );
                        let selected = this.agent_search.active == index;
                        let timestamp = Local
                            .timestamp_millis_opt(result.session.updated_at)
                            .single()
                            .map(|date| date.format("%Y-%m-%d %H:%M:%S").to_string())
                            .unwrap_or_default();
                        let directory = result
                            .session
                            .directory
                            .as_ref()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_else(|| {
                                tr(this.settings.language, "agentSearch.noDirectory").to_owned()
                            });
                        let highlights = match_range(&result.excerpt, &query).map(|range| {
                            (
                                range,
                                HighlightStyle {
                                    color: Some(rgb(0x292000).into()),
                                    background_color: Some(rgb(0xffdf61).into()),
                                    font_weight: Some(FontWeight::SEMIBOLD),
                                    ..Default::default()
                                },
                            )
                        });
                        div()
                            .id(("agent-search-result", index))
                            .debug_selector(move || format!("agent-search-result-{index}").into())
                            .w_full()
                            .h(px(104.))
                            .px_4()
                            .py_2()
                            .v_flex()
                            .gap_1()
                            .overflow_hidden()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .when(selected, |view| view.bg(cx.theme().accent))
                            .hover(|style| style.bg(cx.theme().muted))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .truncate()
                                            .child(if header {
                                                session_title(&result.session)
                                            } else {
                                                source.to_owned()
                                            }),
                                    )
                                    .when(header && group_count > 1, |view| {
                                        view.child(
                                            Button::new(("session-matches", index))
                                                .ghost()
                                                .small()
                                                .debug_selector(move || {
                                                    format!("agent-search-expand-{index}").into()
                                                })
                                                .label(format!(
                                                    "{} · {}",
                                                    group_count,
                                                    tr(
                                                        this.settings.language,
                                                        if expanded {
                                                            "agentSearch.lessMatches"
                                                        } else {
                                                            "agentSearch.moreMatches"
                                                        }
                                                    )
                                                ))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        if !this
                                                            .agent_search
                                                            .expanded_sessions
                                                            .remove(&group_id)
                                                        {
                                                            this.agent_search
                                                                .expanded_sessions
                                                                .insert(group_id.clone());
                                                        }
                                                        this.agent_search.active = index;
                                                        this.agent_search
                                                            .input
                                                            .as_ref()
                                                            .unwrap()
                                                            .read(cx)
                                                            .focus_handle(cx)
                                                            .focus(window, cx);
                                                        cx.notify();
                                                    },
                                                )),
                                        )
                                    }),
                            )
                            .child(div().text_size(px(13.)).line_clamp(2).child(
                                StyledText::new(result.excerpt.clone()).with_highlights(highlights),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .gap_3()
                                    .text_size(px(10.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(directory),
                                    )
                                    .child(div().flex_none().child(source))
                                    .child(div().flex_none().child(timestamp)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.agent_search.active = index;
                                this.accept_agent_search(window, cx);
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.agent_search.scroll)
        .size_full();
        div()
            .id("agent-search-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .bg(black().opacity(0.35))
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|this, _, window, cx| this.close_agent_search(window, cx)))
            .child(
                div()
                    .id("agent-search-dialog")
                    .debug_selector(|| "agent-search".into())
                    .w(px(720.))
                    .max_w(relative(0.94))
                    .h(px(620.))
                    .max_h(relative(0.9))
                    .v_flex()
                    .overflow_hidden()
                    .bg(cx.theme().popover)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_lg()
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex_none()
                            .px_4()
                            .pt_4()
                            .pb_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(self.agent_search.input.as_ref().unwrap())),
                            )
                            .child(
                                Button::new("agent-search-reset")
                                    .small()
                                    .ghost()
                                    .label(tr(lang, "agentSearch.clearSearch"))
                                    .debug_selector(|| "agent-search-reset".into())
                                    .disabled(
                                        !has_filters
                                            && self.agent_search.scope == SearchScope::All
                                            && self
                                                .agent_search
                                                .input
                                                .as_ref()
                                                .unwrap()
                                                .read(cx)
                                                .value()
                                                .is_empty()
                                            && !self.agent_search.submitted,
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.agent_search
                                            .input
                                            .as_ref()
                                            .unwrap()
                                            .update(cx, |input, cx| {
                                                input.set_value("", window, cx)
                                            });
                                        this.agent_search
                                            .time_range
                                            .as_ref()
                                            .unwrap()
                                            .update(cx, |picker, cx| picker.clear(window, cx));
                                        this.agent_search.selected_directories.clear();
                                        this.agent_search.include_subdirectories = false;
                                        this.agent_search.scope = SearchScope::All;
                                        this.invalidate_agent_search(cx);
                                        this.agent_search
                                            .input
                                            .as_ref()
                                            .unwrap()
                                            .read(cx)
                                            .focus_handle(cx)
                                            .focus(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("agent-search-submit")
                                    .debug_selector(|| "agent-search-submit".into())
                                    .custom(
                                        ButtonCustomVariant::new(cx)
                                            .color(cx.theme().secondary)
                                            .foreground(cx.theme().foreground)
                                            .hover(cx.theme().secondary)
                                            .active(cx.theme().accent),
                                    )
                                    .label(tr(lang, "agentSearch.submit"))
                                    .disabled(self.loading_sessions || self.agent_search.pending)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.run_agent_search(cx)),
                                    ),
                            )
                            .child(
                                Button::new("agent-search-close")
                                    .ghost()
                                    .compact()
                                    .icon(IconName::Close)
                                    .tooltip(tr(lang, "search.close"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_agent_search(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .px_4()
                            .pb_3()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} {}",
                                tr(lang, "agentSearch.scopeHint"),
                                self.selected_app.display_name()
                            )),
                    )
                    .child(self.render_agent_filters(cx))
                    .child(
                        div()
                            .flex_none()
                            .px_4()
                            .pb_2()
                            .text_size(px(11.))
                            .text_color(if self.agent_search.error.is_some() {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            })
                            .pt_3()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .debug_selector(|| "agent-search-status".into())
                            .child(status),
                    )
                    .when(self.agent_search.limited, |view| {
                        view.child(
                            div()
                                .flex_none()
                                .px_4()
                                .pb_2()
                                .text_size(px(11.))
                                .child(tr(lang, "agentSearch.limit")),
                        )
                    })
                    .when(self.agent_search.failures > 0, |view| {
                        view.child(div().flex_none().px_4().pb_2().text_size(px(11.)).child(
                            format!(
                                "{} {}",
                                self.agent_search.failures,
                                tr(lang, "agentSearch.failures")
                            ),
                        ))
                    })
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(results)
                            .child(
                                Scrollbar::vertical(&self.agent_search.scroll)
                                    .mode(ScrollbarMode::Always),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_4()
                            .py_2()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(tr(lang, "agentSearch.hint")),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use std::sync::Mutex;
    use yes_core::{MessageType, SessionMessage, SessionProvider};

    struct FixtureProvider {
        details: Vec<SessionDetail>,
        reads: Arc<Mutex<Vec<String>>>,
    }
    impl SessionProvider for FixtureProvider {
        fn app_type(&self) -> AppType {
            AppType::Codex
        }
        fn is_available(&self) -> bool {
            true
        }
        fn sessions(&self) -> anyhow::Result<Vec<Session>> {
            Ok(self.details.iter().map(|d| d.session.clone()).collect())
        }
        fn session_detail(&self, id: &str) -> anyhow::Result<Option<SessionDetail>> {
            self.reads.lock().unwrap().push(id.to_owned());
            if id == "broken" {
                anyhow::bail!("Unreadable fixture");
            }
            Ok(self.details.iter().find(|d| d.session.id == id).cloned())
        }
    }
    fn fixture(id: &str, directory: &str, updated_at: i64) -> SessionDetail {
        SessionDetail {
            subtree_usage: None,
            session: Session {
                id: id.into(),
                app_type: AppType::Codex,
                file_name: id.into(),
                file_path: format!("/tmp/{id}").into(),
                created_at: 0,
                updated_at,
                message_count: 3,
                first_message: format!("Session {id}"),
                last_message: String::new(),
                directory: Some(directory.into()),
                uuid: None,
                kind: yes_core::model::SessionKind::Main,
                parent_session_id: None,
                agent_type: None,
            },
            messages: vec![
                SessionMessage::text(MessageType::User, "", "A different message"),
                SessionMessage::text(MessageType::Assistant, "", "A needle answer"),
                SessionMessage::text(MessageType::User, "", "A needle question"),
            ],
        }
    }
    fn setup(
        cx: &mut TestAppContext,
        details: Vec<SessionDetail>,
    ) -> (
        WindowHandle<YesSessions>,
        Entity<YesSessions>,
        Arc<Mutex<Vec<String>>>,
    ) {
        cx.update(gpui_kit::init);
        cx.update(crate::commands::init);
        let window = cx.open_window(size(px(900.), px(700.)), YesSessions::new);
        let app = window.root(cx).unwrap();
        let reads = Arc::new(Mutex::new(Vec::new()));
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.detail_generation += 1;
            app.loading_sessions = false;
            app.loading_detail = false;
            app.selected_app = AppType::Codex;
            app.settings.sidebar_collapsed = false;
            app.settings.language = Language::En;
            let mut registry = ProviderRegistry::default();
            let provider = FixtureProvider {
                details,
                reads: reads.clone(),
            };
            app.sessions = Arc::new(provider.sessions().unwrap());
            app.registry = {
                registry.register(Arc::new(provider));
                Arc::new(registry)
            };
            app.detail = None;
            cx.notify();
        });
        (window, app, reads)
    }

    fn submit(app: &Entity<YesSessions>, cx: &mut TestAppContext) {
        cx.run_until_parked();
        app.update(cx, |app, cx| app.run_agent_search(cx));
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn grouped_matches_navigation_and_popovers_preserve_results(cx: &mut TestAppContext) {
        let (window, app, reads) = setup(
            cx,
            vec![fixture("one", "/work/a", 2), fixture("two", "/work/b", 1)],
        );
        window
            .update(cx, |app, window, cx| {
                app.open_agent_search(window, cx);
                app.agent_search
                    .input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("needle", window, cx));
            })
            .unwrap();
        submit(&app, cx);
        assert_eq!(reads.lock().unwrap().len(), 2);
        assert_eq!(
            app.read_with(cx, |app, _| app.agent_search.visible_hits()),
            vec![0, 2]
        );
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("down");
        visual.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.agent_search.active), 2);
        let expand = visual.debug_bounds("agent-search-expand-0").unwrap();
        visual.simulate_click(expand.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(
            app.read_with(cx, |app, _| app.agent_search.visible_hits()),
            vec![0, 1, 2]
        );
        visual.simulate_keystrokes("down");
        visual.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.agent_search.active), 1);
        let collapse = visual.debug_bounds("agent-search-expand-0").unwrap();
        visual.simulate_click(collapse.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.agent_search.active), 0);
        assert_eq!(
            app.read_with(cx, |app, _| app.agent_search.visible_hits()),
            vec![0, 2]
        );
        let source = visual.debug_bounds("agent-search-source").unwrap();
        visual.simulate_click(source.center(), Default::default());
        visual.run_until_parked();
        let all = visual.debug_bounds("agent-search.filterAll").unwrap();
        visual.simulate_click(all.center(), Default::default());
        visual.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.agent_search.submitted));
        assert_eq!(app.read_with(cx, |app, _| app.agent_search.hits.len()), 4);
        let source = visual.debug_bounds("agent-search-source").unwrap();
        visual.simulate_click(source.center(), Default::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        app.read_with(cx, |app, _| {
            assert!(
                app.agent_search.input.is_some(),
                "Escape should close the filter first"
            );
            assert!(!app.agent_search.filter_open());
        });
        assert_eq!(
            reads.lock().unwrap().len(),
            2,
            "view changes must not reload sessions"
        );
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.agent_search.input.is_none()));
    }

    #[gpui_kit::test]
    fn searches_filters_and_opens_message_across_sessions(cx: &mut TestAppContext) {
        let date = Local
            .with_ymd_and_hms(2026, 9, 9, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let (window, app, reads) = setup(
            cx,
            vec![
                fixture("first", "/work/a", date),
                fixture("second", "/work/b", date + 1),
                fixture("old", "/work/a", date - 1),
            ],
        );
        window
            .update(cx, |app, window, cx| app.open_agent_search(window, cx))
            .unwrap();
        submit(&app, cx);
        assert!(
            reads.lock().unwrap().is_empty(),
            "blank searches only read metadata"
        );
        assert_eq!(app.read_with(cx, |app, _| app.agent_search.hits.len()), 3);
        window
            .update(cx, |app, window, cx| {
                app.agent_search
                    .time_range
                    .as_ref()
                    .unwrap()
                    .update(cx, |picker, cx| {
                        let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 9).unwrap();
                        picker.set_range(
                            day.and_hms_opt(0, 0, 0),
                            day.and_hms_opt(23, 59, 59),
                            window,
                            cx,
                        );
                    });
                app.agent_search.selected_directories =
                    [PathBuf::from("/work/a"), PathBuf::from("/work/b")].into();
                app.agent_search.scope = SearchScope::Assistant;
                app.agent_search
                    .input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("needle", window, cx));
            })
            .unwrap();
        cx.run_until_parked();
        submit(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.agent_search.hits.len(), 2);
            assert_eq!(app.agent_search.hits[0].session.id, "second");
            assert_eq!(app.agent_search.hits[1].hit.as_ref().unwrap().message, 1);
            assert!(!app.agent_search.pending);
        });
        assert_eq!(*reads.lock().unwrap(), vec!["second", "first"]);
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("agent-search").unwrap();
        assert!(bounds.right() <= px(900.) && bounds.bottom() <= px(700.));
        visual.simulate_keystrokes("down");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        app.read_with(cx, |app, _| {
            assert!(app.agent_search.input.is_none());
            assert_eq!(app.detail.as_ref().unwrap().session.id, "first");
            assert_eq!(app.search_landing.map(|(index, _)| index), Some(1));
            assert!(app.expanded_messages.contains(&1));
        });
        visual.simulate_keystrokes("cmd-shift-f");
        assert!(app.read_with(cx, |app, _| app.agent_search.input.is_some()));
        visual.simulate_keystrokes("escape");
        assert!(app.read_with(cx, |app, _| app.agent_search.input.is_none()));
    }

    #[gpui_kit::test]
    fn cancels_old_queries_and_reports_errors(cx: &mut TestAppContext) {
        let (window, app, reads) = setup(
            cx,
            vec![
                fixture("valid", "/work/a", 2),
                fixture("broken", "/work/b", 1),
            ],
        );
        window
            .update(cx, |app, window, cx| {
                app.open_agent_search(window, cx);
                app.agent_search
                    .input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("needle", window, cx));
            })
            .unwrap();
        cx.run_until_parked();
        // Draft edits do not start reading sessions.
        window
            .update(cx, |app, window, cx| {
                app.agent_search
                    .input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| {
                        input.set_value("different", window, cx);
                        cx.emit(InputEvent::Change);
                    });
            })
            .unwrap();
        cx.run_until_parked();
        submit(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.agent_search.hits.len(), 1);
            assert!(app.agent_search.hits[0].excerpt.contains("different"));
            assert_eq!(app.agent_search.failures, 1);
        });
        assert_eq!(reads.lock().unwrap().len(), 2);
        window
            .update(cx, |app, window, cx| {
                app.agent_search
                    .time_range
                    .as_ref()
                    .unwrap()
                    .update(cx, |picker, cx| {
                        let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 9).unwrap();
                        picker.set_range(
                            day.and_hms_opt(16, 0, 0),
                            day.and_hms_opt(15, 0, 0),
                            window,
                            cx,
                        );
                    });
            })
            .unwrap();
        cx.run_until_parked();
        submit(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(app.agent_search.error.is_some());
            assert!(app.agent_search.hits.is_empty());
            assert!(!app.agent_search.pending);
        });
        // A new provider invalidates the entire dialog, including its in-flight request.
        app.update(cx, |app, cx| app.select_app(AppType::Claude, cx));
        assert!(app.read_with(cx, |app, _| app.agent_search.input.is_none()));
    }

    #[gpui_kit::test]
    fn directory_picker_fits_small_window_in_both_languages(cx: &mut TestAppContext) {
        let (window, app, _) = setup(
            cx,
            vec![fixture(
                "中文搜索示例",
                "/work/a/very-long-working-directory-name",
                0,
            )],
        );
        window
            .update(cx, |app, window, cx| app.open_agent_search(window, cx))
            .unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("time-range-date-0").is_none());
        for language in [Language::Zh, Language::En] {
            app.update(cx, |app, cx| {
                app.settings.language = language;
                app.agent_search.directories_open = true;
                app.agent_search.time_open = false;
                cx.notify();
            });
            visual.simulate_resize(size(px(640.), px(600.)));
            visual.run_until_parked();
            let dialog = visual.debug_bounds("agent-search").unwrap();
            let status = visual.debug_bounds("agent-search-status").unwrap();
            let directory = visual.debug_bounds("agent-search-directory-0").unwrap();
            assert!(dialog.right() <= px(640.) && dialog.bottom() <= px(600.));
            assert!(status.bottom() <= dialog.bottom());
            assert!(directory.right() <= dialog.right());

            app.update(cx, |app, cx| {
                app.agent_search.directories_open = false;
                cx.notify();
            });
            visual.run_until_parked();
            let time_tab = visual.debug_bounds("agent-search-time").unwrap();
            visual.simulate_click(time_tab.center(), Default::default());
            visual.run_until_parked();
            let time_submit = visual.debug_bounds("agent-search-submit").unwrap();
            assert!(time_submit.bottom() <= dialog.bottom());
            if let Some(custom) = visual.debug_bounds("time-custom") {
                visual.simulate_click(custom.center(), Default::default());
                visual.run_until_parked();
            }
            assert_eq!(
                visual.debug_bounds("agent-search").unwrap(),
                dialog,
                "popover must not move results"
            );
            let clear = visual.debug_bounds("time-range-clear").unwrap();
            assert!(time_submit.bottom() < clear.top());
            let reset = visual.debug_bounds("agent-search-reset").unwrap();
            assert_eq!(reset.center().y, time_submit.center().y);
            assert!(reset.right() <= time_submit.left());
            let calendar = visual.debug_bounds("time-range-calendar").unwrap();
            assert!(calendar.right() <= dialog.right());
            assert!(calendar.bottom() <= px(600.));
        }
    }
    #[gpui_kit::test]
    fn searches_after_initial_loading_and_bounds_large_result_sets(cx: &mut TestAppContext) {
        let mut large = fixture("large", "/work/a", 2);
        large.messages = (0..RESULT_LIMIT + 10)
            .map(|_| SessionMessage::text(MessageType::User, "", "needle"))
            .collect();
        let (window, app, reads) = setup(cx, vec![large, fixture("later", "/work/b", 1)]);
        window
            .update(cx, |app, window, cx| {
                app.load_sessions(cx);
                app.open_agent_search(window, cx);
                assert!(app.loading_sessions);
            })
            .unwrap();
        cx.run_until_parked();
        submit(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.agent_search.hits.len(), 2);
            assert!(
                app.agent_search
                    .directories
                    .contains(&PathBuf::from("/work/b"))
            );
        });
        reads.lock().unwrap().clear();
        window
            .update(cx, |app, window, cx| {
                app.agent_search
                    .input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| {
                        input.set_value("needle", window, cx);
                        cx.emit(InputEvent::Change);
                    });
            })
            .unwrap();
        cx.run_until_parked();
        submit(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.agent_search.hits.len(), RESULT_LIMIT);
            assert!(app.agent_search.limited);
            assert!(!app.agent_search.pending);
        });
        assert_eq!(*reads.lock().unwrap(), vec!["large"]);
    }
    #[gpui_kit::test]
    fn picker_second_precision_filters_the_matching_second(cx: &mut TestAppContext) {
        let time = Local
            .with_ymd_and_hms(2026, 9, 9, 12, 34, 56)
            .single()
            .unwrap();
        let stamp = time.timestamp_millis();
        let (window, app, reads) = setup(
            cx,
            vec![
                fixture("before", "/work/a", stamp - 1),
                fixture("within", "/work/a", stamp + 999),
                fixture("after", "/work/a", stamp + 1000),
            ],
        );
        window
            .update(cx, |app, window, cx| {
                app.open_agent_search(window, cx);
                app.agent_search
                    .time_range
                    .as_ref()
                    .unwrap()
                    .update(cx, |picker, cx| {
                        picker.set_range(
                            Some(time.naive_local()),
                            Some(time.naive_local()),
                            window,
                            cx,
                        );
                    });
            })
            .unwrap();
        cx.run_until_parked();
        submit(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.agent_search.hits.len(), 1);
            assert_eq!(app.agent_search.hits[0].session.id, "within");
        });
        assert!(reads.lock().unwrap().is_empty());
    }
    #[gpui_kit::test]
    fn draft_filters_do_not_read_sessions_and_roles_count_messages(cx: &mut TestAppContext) {
        let mut detail = fixture("one", "/work/a", 0);
        detail.messages.push(SessionMessage::text(
            MessageType::User,
            "",
            "another needle",
        ));
        let (window, app, reads) = setup(cx, vec![detail]);
        window
            .update(cx, |app, window, cx| {
                app.open_agent_search(window, cx);
                app.agent_search
                    .input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| {
                        input.set_value("needle", window, cx);
                        cx.emit(InputEvent::Change);
                    });
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        let source = visual.debug_bounds("agent-search-source").unwrap();
        visual.simulate_click(source.center(), Default::default());
        visual.run_until_parked();
        let user = visual.debug_bounds("agent-search.filterUser").unwrap();
        visual.simulate_click(user.center(), Default::default());
        visual.run_until_parked();
        assert!(reads.lock().unwrap().is_empty());
        assert!(app.read_with(cx, |app, _| app.agent_search.hits.is_empty()));
        let button = visual.debug_bounds("agent-search-submit").unwrap();
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.agent_search.hits.len()), 2);
        assert_eq!(reads.lock().unwrap().len(), 1);
        let source = visual.debug_bounds("agent-search-source").unwrap();
        visual.simulate_click(source.center(), Default::default());
        visual.run_until_parked();
        let ai = visual.debug_bounds("agent-search.filterAssistant").unwrap();
        visual.simulate_click(ai.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(
            reads.lock().unwrap().len(),
            1,
            "changing scope must not read again"
        );
        assert!(
            app.read_with(cx, |app, _| app.agent_search.hits.is_empty()),
            "old scope results must not remain actionable"
        );
        window
            .update(cx, |app, window, cx| {
                assert!(
                    app.agent_search_query_focused(window, cx),
                    "query must be focused after scope click"
                );
            })
            .unwrap();
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert_eq!(
            app.read_with(cx, |app, _| (
                app.agent_search.hits.len(),
                app.agent_search.submitted,
                app.agent_search.error
            )),
            (1, true, None)
        );
        assert_eq!(reads.lock().unwrap().len(), 2);
        window
            .update(cx, |app, window, cx| {
                app.agent_search
                    .input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| {
                        input.set_value("", window, cx);
                        cx.emit(InputEvent::Change);
                    });
            })
            .unwrap();
        visual.run_until_parked();
        assert_eq!(
            app.read_with(cx, |app, _| app.agent_search.scope),
            SearchScope::All
        );
        submit(&app, cx);
        assert_eq!(
            reads.lock().unwrap().len(),
            2,
            "empty keyword uses cached session metadata"
        );
        app.read_with(cx, |app, _| {
            assert_eq!(app.agent_search.hits.len(), 1);
            assert!(app.agent_search.hits[0].hit.is_none());
        });
        visual.run_until_parked();
        let clear = visual.debug_bounds("agent-search-reset").unwrap();
        visual.simulate_click(clear.center(), Default::default());
        visual.run_until_parked();
        app.read_with(cx, |app, cx| {
            assert!(app.agent_search.hits.is_empty());
            assert!(!app.agent_search.submitted);
            assert!(
                app.agent_search
                    .input
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .value()
                    .is_empty()
            );
        });
        assert_eq!(reads.lock().unwrap().len(), 2, "clearing must not scan");
    }
}
