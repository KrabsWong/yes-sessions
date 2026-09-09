use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use yes_core::{MessageType, SessionMessage};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SearchScope {
    #[default]
    All,
    User,
    Assistant,
}

#[derive(Clone, Debug)]
struct SearchHit {
    message: usize,
    excerpt: String,
}

#[derive(Default)]
pub(super) struct SessionSearch {
    pub(super) input: Option<Entity<InputState>>,
    source: Option<Arc<SessionDetail>>,
    hits: Vec<SearchHit>,
    active: usize,
    scope: SearchScope,
    scroll: UniformListScrollHandle,
    pending: bool,
    task: Option<Task<()>>,
}

fn match_range(text: &str, query: &str) -> Option<Range<usize>> {
    if query.is_empty() {
        return None;
    }
    let offset = text.to_lowercase().find(query)?;
    let mut folded = 0;
    let mut start = None;
    for (index, ch) in text.char_indices() {
        let next = folded + ch.to_lowercase().map(char::len_utf8).sum::<usize>();
        if start.is_none() && next > offset {
            start = Some(index);
        }
        if next >= offset + query.len() {
            return Some(start?..index + ch.len_utf8());
        }
        folded = next;
    }
    None
}

// Return a small original-text excerpt, including when lowercasing changes UTF-8 length.
fn excerpt(text: &str, query: &str) -> Option<String> {
    let start = match_range(text, query)?.start;
    let prefix = text[..start]
        .char_indices()
        .rev()
        .nth(40)
        .map_or(0, |(i, _)| i);
    let body = text[prefix..]
        .chars()
        .take(200)
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .collect::<String>();
    Some(format!(
        "{}{}{}",
        if prefix > 0 { "…" } else { "" },
        body,
        if text[prefix..].chars().take(201).count() > 200 {
            "…"
        } else {
            ""
        }
    ))
}

fn search_messages(messages: &[SessionMessage], query: &str, scope: SearchScope) -> Vec<SearchHit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    messages
        .iter()
        .enumerate()
        .filter(|(_, item)| match scope {
            SearchScope::All => true,
            SearchScope::User => item.message_type == MessageType::User,
            SearchScope::Assistant => item.message_type == MessageType::Assistant,
        })
        .filter_map(|(message, item)| {
            let found = [
                item.content.as_deref(),
                item.reasoning_content.as_deref(),
                item.tool_name.as_deref(),
                item.tool_output
                    .as_ref()
                    .and_then(|out| out.output.as_deref()),
                item.tool_output
                    .as_ref()
                    .and_then(|out| out.preview.as_deref()),
            ]
            .into_iter()
            .flatten()
            .find_map(|text| excerpt(text, &query))
            .or_else(|| {
                item.attachments.iter().find_map(|attachment| {
                    excerpt(&attachment.name, &query).or_else(|| match &attachment.source {
                        yes_core::AttachmentSource::LocalPath(path) => {
                            excerpt(&path.to_string_lossy(), &query)
                        }
                        _ => None,
                    })
                })
            })
            .or_else(|| {
                item.tool_input.as_ref().and_then(|input| {
                    excerpt(
                        &serde_json::to_string_pretty(input).unwrap_or_default(),
                        &query,
                    )
                })
            });
            found.map(|excerpt| SearchHit { message, excerpt })
        })
        .collect()
}

impl YesSessions {
    pub(super) fn open_session_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.detail.is_none() || self.settings_open {
            return;
        }
        if self.session_search.input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(tr(self.settings.language, "search.placeholder"))
            });
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.schedule_session_search(cx),
                    InputEvent::PressEnter { .. } => this.accept_session_search(window, cx),
                    _ => {}
                },
            )
            .detach();
            self.session_search.input = Some(input);
        }
        self.session_search
            .input
            .as_ref()
            .unwrap()
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    pub(super) fn schedule_session_search(&mut self, cx: &mut Context<Self>) {
        self.session_search.task = None;
        self.session_search.hits.clear();
        self.session_search.active = 0;
        self.session_search.scroll = UniformListScrollHandle::new();
        self.session_search.pending = false;
        self.session_search.source = self.detail.clone();
        let Some(input) = &self.session_search.input else {
            return;
        };
        let query = input.read(cx).value().to_string();
        let Some(detail) = self.detail.clone().filter(|_| !query.trim().is_empty()) else {
            cx.notify();
            return;
        };
        self.session_search.pending = true;
        let scope = self.session_search.scope;
        self.session_search.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(180))
                .await;
            let hits = cx
                .background_executor()
                .spawn(async move { search_messages(&detail.messages, &query, scope) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.session_search.hits = hits;
                this.session_search.pending = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn set_session_search_scope(
        &mut self,
        scope: SearchScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session_search.scope != scope {
            self.session_search.scope = scope;
            self.schedule_session_search(cx);
        }
        if let Some(input) = &self.session_search.input {
            input.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    pub(super) fn step_session_search(&mut self, previous: bool, cx: &mut Context<Self>) {
        let count = self.session_search.hits.len();
        if count == 0 {
            return;
        }
        self.session_search.active =
            (self.session_search.active + if previous { count - 1 } else { 1 }) % count;
        self.session_search
            .scroll
            .scroll_to_item(self.session_search.active, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn jump_session_search(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self.session_search.hits.get(self.session_search.active) else {
            return;
        };
        let Some(detail) = &self.detail else {
            return;
        };
        let message = hit.message;
        let landing = (message, Instant::now());
        self.search_landing = Some(landing);
        self.search_landing_scroll_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| {
                if this.search_landing == Some(landing) {
                    this.search_landing = None;
                    cx.notify();
                }
            });
        })
        .detach();
        self.expanded_messages.insert(message);
        // Tool result panels are controlled by their corresponding tool-use message.
        if let Some(call_id) = &detail.messages[message].call_id {
            for (index, item) in detail.messages.iter().enumerate() {
                if item.call_id.as_ref() == Some(call_id) {
                    self.expanded_messages.insert(index);
                }
            }
        }
        if let Some(key) = crate::conversation::tool_activity_key_for_message(
            &detail.messages,
            message,
            self.selected_app,
            self.settings.show_thinking_content,
        ) {
            self.expanded_messages.insert(key);
        }
        let turn = turn_index_for_message(&detail.messages, message, self.selected_app);
        self.conversation_state.update(cx, |state, cx| {
            state.scroll_to_item(turn, cx);
        });
        cx.notify();
    }

    fn accept_session_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_search.hits.is_empty() {
            return;
        }
        self.jump_session_search(cx);
        self.session_search = SessionSearch::default();
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_session_search(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let changed = match (&self.session_search.source, &self.detail) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.schedule_session_search(cx);
        }
        let input = self.session_search.input.as_ref().unwrap();
        let lang = self.settings.language;
        let count = self.session_search.hits.len();
        let status = if self.session_search.pending {
            tr(lang, "search.searching").to_owned()
        } else if input.read(cx).value().trim().is_empty() {
            tr(
                lang,
                match self.session_search.scope {
                    SearchScope::All => "search.scope",
                    SearchScope::User => "search.scopeUser",
                    SearchScope::Assistant => "search.scopeAssistant",
                },
            )
            .to_owned()
        } else if count == 0 {
            tr(lang, "search.empty").to_owned()
        } else {
            format!("{} {}", count, tr(lang, "search.matches"))
        };
        let results = uniform_list(
            "session-search-results",
            count,
            cx.processor(|this, range: Range<usize>, _, cx| {
                let query = this
                    .session_search
                    .input
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .value()
                    .trim()
                    .to_lowercase();
                range
                    .map(|index| {
                        let hit = &this.session_search.hits[index];
                        let selected = this.session_search.active == index;
                        let dark = cx.theme().mode == ThemeMode::Dark;
                        let highlights = match_range(&hit.excerpt, &query).map(|range| {
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
                        let message = &this.detail.as_ref().unwrap().messages[hit.message];
                        let role = match message.message_type {
                            yes_core::MessageType::User => {
                                tr(this.settings.language, "search.user")
                            }
                            yes_core::MessageType::Assistant => this.selected_app.display_name(),
                            _ => message
                                .tool_name
                                .as_deref()
                                .unwrap_or(tr(this.settings.language, "search.tool")),
                        };
                        div()
                            .id(("search-result", index))
                            .debug_selector(move || format!("search-result-{index}").into())
                            .w_full()
                            .h(px(96.))
                            .px_4()
                            .py_3()
                            .v_flex()
                            .gap_2()
                            .overflow_hidden()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .when(selected, |view| {
                                view.bg(if dark { rgb(0x39331e) } else { rgb(0xfff7d6) })
                            })
                            .hover(|style| style.bg(cx.theme().muted))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(role.to_owned())
                                    .child(crate::conversation::display_datetime(
                                        &message.timestamp,
                                    )),
                            )
                            .child(div().text_size(px(13.)).line_clamp(2).child(
                                StyledText::new(hit.excerpt.clone()).with_highlights(highlights),
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.session_search.active = index;
                                this.accept_session_search(window, cx);
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.session_search.scroll)
        .size_full();
        let results = div()
            .relative()
            .min_h_0()
            .h(px((count.min(5) * 96) as f32))
            .w_full()
            .child(results)
            .child(Scrollbar::vertical(&self.session_search.scroll).mode(ScrollbarMode::Always));
        div()
            .id("search-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .bg(black().opacity(0.35))
            .flex()
            .items_start()
            .justify_center()
            .pt(px(88.))
            .on_click(cx.listener(|this, _, window, cx| {
                this.session_search = SessionSearch::default();
                this.root_focus.focus(window, cx);
                cx.notify();
            }))
            .child(
                div()
                    .id("session-search-dialog")
                    .debug_selector(|| "session-search".into())
                    .w(px(640.))
                    .max_w(relative(0.9))
                    .max_h(relative(0.85))
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
                            .flex()
                            .items_start()
                            .gap_3()
                            .px_4()
                            .pt_4()
                            .pb_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(tr(lang, "search.title")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(cx.theme().muted_foreground)
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(
                                                self.detail
                                                    .as_ref()
                                                    .map(|detail| {
                                                        if detail.session.first_message.is_empty() {
                                                            detail.session.file_name.clone()
                                                        } else {
                                                            detail.session.first_message.clone()
                                                        }
                                                    })
                                                    .unwrap_or_default(),
                                            ),
                                    ),
                            )
                            .child(
                                Button::new("search-close")
                                    .ghost()
                                    .compact()
                                    .icon(IconName::Close)
                                    .tooltip(tr(lang, "search.close"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.session_search = SessionSearch::default();
                                        this.root_focus.focus(window, cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_4()
                            .pb_3()
                            .child(
                                Icon::new(IconName::Search)
                                    .size(px(18.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(div().flex_1().min_w_0().child(Input::new(input))),
                    )
                    .child(
                        div().flex_none().flex().gap_1().px_4().pb_3().children(
                            [
                                (SearchScope::All, "search.filterAll"),
                                (SearchScope::User, "search.filterUser"),
                                (SearchScope::Assistant, "search.filterAssistant"),
                            ]
                            .into_iter()
                            .map(|(scope, key)| {
                                Button::new(key)
                                    .debug_selector(move || key.into())
                                    .ghost()
                                    .compact()
                                    .label(tr(lang, key))
                                    .selected(self.session_search.scope == scope)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_session_search_scope(scope, window, cx);
                                    }))
                            }),
                        ),
                    )
                    .child(
                        div()
                            .px_4()
                            .pb_3()
                            .when(count == 0, |view| view.pt_2())
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(status),
                    )
                    .when(count > 0, |view| view.child(results))
                    .child(
                        div()
                            .px_4()
                            .py_2()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(tr(lang, "search.hint")),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn attachment_names_and_paths_remain_searchable_without_indexing_payloads() {
        let mut message = SessionMessage::text(MessageType::User, "", "");
        message.attachments = vec![
            yes_core::SessionAttachment {
                name: "diagram.png".into(),
                embedded_fallback: None,
                mime_type: Some("image/png".into()),
                source: yes_core::AttachmentSource::LocalPath("/tmp/project/diagram.png".into()),
            },
            yes_core::SessionAttachment {
                name: "report.pdf".into(),
                embedded_fallback: None,
                mime_type: Some("application/pdf".into()),
                source: yes_core::AttachmentSource::DataUrl(
                    "data:application/pdf;base64,secretpayload".into(),
                ),
            },
        ];
        let messages = [message];
        for query in ["diagram", "project", "report"] {
            let hits = search_messages(&messages, query, SearchScope::User);
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].message, 0);
        }
        assert!(search_messages(&messages, "secretpayload", SearchScope::All).is_empty());
    }

    #[test]
    fn filters_message_sources_and_preserves_jump_indices() {
        let mut reasoning = SessionMessage::text(MessageType::Assistant, "", "");
        reasoning.reasoning_content = Some("needle in reasoning".into());
        let messages = vec![
            SessionMessage::text(MessageType::System, "", "needle"),
            SessionMessage::text(MessageType::User, "", "needle"),
            SessionMessage::text(MessageType::ToolUse, "", "needle"),
            SessionMessage::text(MessageType::Assistant, "", "needle"),
            SessionMessage::text(MessageType::ToolResult, "", "needle"),
            reasoning,
        ];
        let indices = |scope| {
            search_messages(&messages, "needle", scope)
                .into_iter()
                .map(|hit| hit.message)
                .collect::<Vec<_>>()
        };
        assert_eq!(indices(SearchScope::All), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(indices(SearchScope::User), vec![1]);
        assert_eq!(indices(SearchScope::Assistant), vec![3, 5]);
    }

    #[test]
    fn finds_literal_unicode_text_and_tool_input_once_per_message() {
        let mut tool = SessionMessage::text(yes_core::MessageType::ToolUse, "", "");
        tool.tool_input = Some(
            serde_json::json!({"path": "src/Test.rs"})
                .as_object()
                .unwrap()
                .clone(),
        );
        let messages = vec![
            SessionMessage::text(yes_core::MessageType::User, "", "中文 TEST test"),
            tool,
        ];
        assert_eq!(
            search_messages(&messages, "test", SearchScope::All).len(),
            2
        );
        assert_eq!(
            search_messages(&messages, "中文", SearchScope::All)[0].message,
            0
        );
        assert!(search_messages(&messages, "  ", SearchScope::All).is_empty());
        assert!(search_messages(&messages, "[.*]", SearchScope::All).is_empty());
        assert!(excerpt("İ Unicode 搜索", "搜索").unwrap().contains("搜索"));
    }
    #[test]
    fn bounds_long_message_excerpt() {
        let hits = search_messages(
            &[SessionMessage::text(
                yes_core::MessageType::User,
                "",
                format!("{}keyword{}", "长".repeat(100_000), "x".repeat(100_000)),
            )],
            "keyword",
            SearchScope::All,
        );
        assert_eq!(hits.len(), 1);
        assert!(hits[0].excerpt.contains("keyword"));
        assert!(hits[0].excerpt.chars().count() <= 202);
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use core::prelude::v1::test;

    #[gpui_kit::test]
    fn search_scopes_results_navigates_and_fits_narrow_detail(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(crate::commands::init);
        let window = cx.open_window(size(px(900.), px(700.)), YesSessions::new);
        let app = window.root(cx).unwrap();
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.loading_sessions = false;
            let session = Session {
                id: "search-fixture".into(),
                app_type: AppType::CodeBuddy,
                file_name: "fixture".into(),
                file_path: PathBuf::from("/tmp/search-fixture"),
                created_at: 0,
                updated_at: 0,
                message_count: 3,
                first_message: "Search fixture".into(),
                last_message: String::new(),
                directory: None,
                uuid: None,
                kind: yes_core::model::SessionKind::Main,
                parent_session_id: None,
                agent_type: None,
            };
            app.detail = Some(Arc::new(SessionDetail {
                session,
                messages: vec![
                    SessionMessage::text(yes_core::MessageType::User, "", "First needle"),
                    SessionMessage::text(yes_core::MessageType::Assistant, "", "Answer"),
                    SessionMessage::text(yes_core::MessageType::User, "", "Second NEEDLE"),
                ],
            }));
            app.conversation_state
                .update(cx, |state, cx| state.reset(3, cx));
        });
        window
            .update(cx, |app, window, cx| {
                app.open_session_search(window, cx);
                app.session_search
                    .input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("needle", window, cx));
                let hits = search_messages(
                    &app.detail.as_ref().unwrap().messages,
                    "needle",
                    SearchScope::All,
                );
                app.session_search.hits = hits;
                app.step_session_search(false, cx);
                assert_eq!(app.session_search.active, 1);
                assert!(!app.expanded_messages.contains(&2));
                app.step_session_search(false, cx);
                assert_eq!(app.session_search.active, 0);
                app.step_session_search(true, cx);
                assert_eq!(app.session_search.active, 1);
                // Keep the fixture synchronous so this test does not depend on timer scheduling.
                app.session_search.source = app.detail.clone();
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("session-search").unwrap();
        assert!(bounds.size.width > px(200.));
        assert!(bounds.right() <= px(900.));
        for (key, scope, count) in [
            ("search.filterAssistant", SearchScope::Assistant, 0),
            ("search.filterUser", SearchScope::User, 2),
            ("search.filterAll", SearchScope::All, 2),
        ] {
            let filter = visual.debug_bounds(key).unwrap();
            assert!(filter.right() <= bounds.right());
            visual.simulate_click(filter.center(), Default::default());
            visual.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(200));
            visual.run_until_parked();
            app.read_with(cx, |app, cx| {
                assert_eq!(app.session_search.scope, scope);
                assert_eq!(app.session_search.hits.len(), count);
                assert_eq!(app.session_search.active, 0);
                assert!(!app.session_search.pending);
                assert_eq!(
                    app.session_search.input.as_ref().unwrap().read(cx).value(),
                    "needle"
                );
            });
        }
        app.update(cx, |app, cx| app.step_session_search(true, cx));
        visual.simulate_keystrokes("down");
        assert_eq!(app.read_with(cx, |app, _| app.session_search.active), 0);
        visual.simulate_keystrokes("enter");
        assert!(app.read_with(cx, |app, _| app.session_search.input.is_none()));
        assert!(app.read_with(cx, |app, _| app.expanded_messages.contains(&0)));
        visual.simulate_keystrokes("cmd-f");
        visual.simulate_keystrokes("escape");
        assert!(app.read_with(cx, |app, _| app.session_search.input.is_none()));
        visual.simulate_keystrokes("cmd-f");
        assert!(app.read_with(cx, |app, _| app.session_search.input.is_some()));
        app.update(cx, |app, cx| {
            app.session_search.source = app.detail.clone();
            app.session_search.hits = search_messages(
                &app.detail.as_ref().unwrap().messages,
                "needle",
                SearchScope::All,
            );
            cx.notify();
        });
        visual.run_until_parked();
        let row = visual.debug_bounds("search-result-1").unwrap();
        visual.simulate_click(row.center(), Default::default());
        assert!(app.read_with(cx, |app, _| app.session_search.input.is_none()));
        assert!(app.read_with(cx, |app, _| app.expanded_messages.contains(&2)));
        assert_eq!(
            app.read_with(cx, |app, _| app.search_landing.map(|(index, _)| index)),
            Some(2)
        );
        assert!(visual.debug_bounds("search-landing-2").is_some());
        visual.simulate_keystrokes("cmd-f");
        app.update(cx, |app, cx| {
            let detail = Arc::make_mut(app.detail.as_mut().unwrap());
            detail.messages[0].content = Some("Long prelude paragraph.\n\n".repeat(80));
            app.session_search.source = app.detail.clone();
            app.session_search.hits = search_messages(
                &app.detail.as_ref().unwrap().messages,
                "Answer",
                SearchScope::All,
            );
            app.session_search.active = 0;
            app.conversation_state
                .update(cx, |state, cx| state.remeasure(cx));
            cx.notify();
        });
        visual.run_until_parked();
        let result = visual.debug_bounds("search-result-0").unwrap();
        visual.simulate_click(result.center(), Default::default());
        let landing = visual.debug_bounds("search-landing-1").unwrap();
        assert!(
            landing.top() >= px(0.) && landing.top() < px(650.),
            "{landing:?}"
        );
        visual.simulate_keystrokes("cmd-f");
        app.update(cx, |app, cx| {
            let detail = Arc::make_mut(app.detail.as_mut().unwrap());
            detail.messages = (0..30)
                .map(|_| SessionMessage::text(yes_core::MessageType::User, "", "needle"))
                .collect();
            app.session_search.hits = search_messages(&detail.messages, "needle", SearchScope::All);
            app.session_search.source = app.detail.clone();
            app.session_search.active = 0;
            cx.notify();
        });
        visual.run_until_parked();
        app.update(cx, |app, cx| app.step_session_search(true, cx));
        visual.run_until_parked();
        app.read_with(cx, |app, _| {
            use gpui_kit::component::scroll::ScrollbarHandle;
            let scroll = &app.session_search.scroll;
            assert!(scroll.content_size().height > scroll.viewport_bounds().size.height);
            assert!(scroll.offset().y < px(0.));
        });
        let last = visual.debug_bounds("search-result-29").unwrap();
        assert!(last.bottom() <= visual.debug_bounds("session-search").unwrap().bottom());
        app.update(cx, |app, cx| {
            app.reset_detail(cx);
            assert!(app.session_search.input.is_none());
            assert!(app.session_search.hits.is_empty());
        });
    }
}
