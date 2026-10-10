use super::*;
use gpui_kit::component::input::{Input, InputState};
use yes_core::remote::{RemoteCodexConfig, RemoteCodexProvider};

pub(super) struct RemoteSettings {
    target: Entity<InputState>,
    root: Entity<InputState>,
    pub(super) status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[gpui_kit::test]
    fn remote_bar_fits_compact_window_in_both_languages(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(760.), px(650.)), YesSessions::new);
        let app = window.root(cx).unwrap();
        for language in [Language::Zh, Language::En] {
            app.update(cx, |app, cx| {
                app.sessions_generation += 1;
                app.loading_sessions = false;
                app.settings.language = language;
                app.remote_provider = Some(Arc::new(
                    RemoteCodexProvider::new(RemoteCodexConfig {
                        ssh_alias: "test.invalid".into(),
                        ..Default::default()
                    })
                    .unwrap(),
                ));
                cx.notify();
            });
            let mut visual = VisualTestContext::from_window(*window, cx);
            visual.run_until_parked();
            let bar = visual.debug_bounds("remote-status-bar").unwrap();
            assert_eq!(bar.size.width, px(760.));
            assert_eq!(bar.size.height, px(30.));
            assert_eq!(bar.bottom(), px(650.));
        }
    }

    #[gpui_kit::test]
    fn remote_source_blocks_local_actions_and_returns_to_local(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(800.)), YesSessions::new);
        let app = window.root(cx).unwrap();
        app.update(cx, |app, cx| {
            app.sessions_generation += 1;
            app.detail_generation += 1;
            app.loading_sessions = false;
            let generation = app.sessions_generation;
            let config = RemoteCodexConfig {
                ssh_alias: "test.invalid".into(),
                ..Default::default()
            };
            app.remote_provider = Some(Arc::new(RemoteCodexProvider::new(config).unwrap()));
            app.selected_app = AppType::Codex;
            app.select_app(AppType::OpenCode, cx);
            assert_eq!(app.selected_app, AppType::Codex);
            assert!(!app.has_workspace());
            assert!(app.active_provider().unwrap().is_available());
            app.parent_conversations.clear();
            app.select_local(cx);
            assert!(app.remote_provider.is_none());
            assert!(app.sessions_generation > generation);
            assert!(app.selected_session_id.is_none());
            assert!(app.parent_conversations.is_empty());
        });
    }

    #[gpui_kit::test]
    fn remote_settings_renders_in_both_languages(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(800.)), YesSessions::new);
        let app = window.root(cx).unwrap();
        for language in [Language::Zh, Language::En] {
            app.update(cx, |app, cx| {
                app.sessions_generation += 1;
                app.loading_sessions = false;
                app.settings.language = language;
                app.open_remote_settings(cx);
            });
            let mut visual = VisualTestContext::from_window(*window, cx);
            visual.run_until_parked();
            let bounds = visual.debug_bounds("remote-settings").unwrap();
            assert!(bounds.size.width <= px(576.));
            assert!(bounds.size.height < px(650.));
        }
    }
}

impl RemoteSettings {
    pub(super) fn new(
        saved: Option<&RemoteCodexConfig>,
        window: &mut Window,
        cx: &mut Context<YesSessions>,
    ) -> Self {
        let config = saved.cloned().unwrap_or_default();
        let target = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("user@server / SSH Host");
            input.set_value(config.ssh_alias, window, cx);
            input
        });
        let root = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("~/.codex");
            input.set_value(config.root, window, cx);
            input
        });
        Self {
            target,
            root,
            status: None,
        }
    }

    fn config(&self, cx: &App) -> RemoteCodexConfig {
        RemoteCodexConfig {
            ssh_alias: self.target.read(cx).value().trim().to_owned(),
            root: self.root.read(cx).value().trim().to_owned(),
        }
    }
}

impl YesSessions {
    pub(super) fn render_remote_bar(&self) -> impl IntoElement {
        let language = self.settings.language;
        let busy = self.loading_sessions || self.refreshing_sessions || self.loading_detail;
        let failed = !busy && self.remote.status.is_some();
        let status = if busy {
            "remote.reading"
        } else if failed {
            "remote.readFailed"
        } else {
            "remote.snapshotReady"
        };
        div()
            .id("remote-status-bar")
            .debug_selector(|| "remote-status-bar".into())
            .flex()
            .items_center()
            .gap_4()
            .h(px(30.))
            .flex_shrink_0()
            .px_4()
            .bg(rgb(if failed { 0x93451d } else { 0x126b63 }))
            .text_color(rgb(0xffffff))
            .text_xs()
            .child(
                div()
                    .flex_shrink_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("SSH · {}", tr(language, "remote.title"))),
            )
            .child(div().flex_1())
            .child(div().flex_shrink_0().child(tr(language, status)))
    }

    pub fn open_remote_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_tab = SettingsTab::Remote;
        self.settings_open = true;
        cx.notify();
    }

    pub(super) fn render_remote_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.settings.language;
        div()
            .v_flex()
            .gap_2()
            .px_3()
            .py_2()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(tr(language, "remote.readOnly"))
                    .child(
                        Button::new("remote-refresh")
                            .ghost()
                            .compact()
                            .label(tr(language, "dashboard.refresh"))
                            .disabled(
                                self.loading_sessions
                                    || self.refreshing_sessions
                                    || self.loading_detail,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_sessions(cx))),
                    ),
            )
            .when_some(self.remote.status.clone(), |view, status| {
                view.child(status)
            })
    }

    pub(super) fn active_provider(&self) -> Option<Arc<dyn yes_core::SessionProvider>> {
        match &self.remote_provider {
            Some(provider) => Some(provider.clone()),
            None => self.registry.get(self.selected_app),
        }
    }

    fn connect_remote(&mut self, cx: &mut Context<Self>) {
        let config = self.remote.config(cx);
        match RemoteCodexProvider::new(config.clone()) {
            Ok(provider) => {
                if let Some(previous) = self.remote_provider.take() {
                    previous.cancel();
                }
                self.settings.remote_codex = Some(config);
                self.save_settings();
                self.remote_provider = Some(Arc::new(provider));
                self.dashboard_open = false;
                self.dashboard_session = None;
                self.selected_app = AppType::Codex;
                self.collapsed_groups.clear();
                self.expanded_parents.clear();
                self.group_visible_counts.clear();
                self.settings_open = false;
                self.remote.status = None;
                self.load_sessions(cx);
            }
            Err(error) => self.remote.status = Some(error.to_string()),
        }
        cx.notify();
    }

    fn select_local(&mut self, cx: &mut Context<Self>) {
        if let Some(provider) = self.remote_provider.take() {
            provider.cancel();
            self.dashboard_session = None;
            self.collapsed_groups.clear();
            self.expanded_parents.clear();
            self.group_visible_counts.clear();
            self.load_sessions(cx);
        }
        cx.notify();
    }

    fn login_remote(&mut self, cx: &mut Context<Self>) {
        let result =
            self.remote.config(cx).login_command().and_then(|command| {
                yes_core::terminal::open_ssh_login(&command).map_err(Into::into)
            });
        self.remote.status = Some(match result {
            Ok(()) => tr(self.settings.language, "remote.loginPending").to_owned(),
            Err(error) => error.to_string(),
        });
        cx.notify();
    }

    pub(super) fn render_source_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.settings.language;
        let label = if self.remote_provider.is_some() {
            format!(
                "SSH · {}",
                self.settings
                    .remote_codex
                    .as_ref()
                    .map(|c| c.ssh_alias.as_str())
                    .unwrap_or_default()
            )
        } else {
            tr(language, "remote.local").to_owned()
        };
        let owner = cx.weak_entity();
        Button::new("source-selector")
            .debug_selector(|| "source-selector".into())
            .when(self.remote_provider.is_none(), |button| button.outline())
            .when(self.remote_provider.is_some(), |button| {
                button
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(rgb(0x126b63).into())
                            .foreground(rgb(0xffffff).into())
                            .hover(rgb(0x185c56).into()),
                    )
                    .bg(rgb(0x126b63))
            })
            .compact()
            .max_w(px(290.))
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let local_owner = owner.clone();
                let remote_owner = owner.clone();
                menu.item(PopupMenuItem::new(tr(language, "remote.local")).on_click(
                    move |_, _, cx| {
                        let _ = local_owner.update(cx, |this, cx| this.select_local(cx));
                    },
                ))
                .item(
                    PopupMenuItem::new(tr(language, "remote.configure")).on_click(
                        move |_, _, cx| {
                            let _ = remote_owner.update(cx, |this, cx| {
                                this.settings_tab = SettingsTab::Remote;
                                this.settings_open = true;
                                cx.notify();
                            });
                        },
                    ),
                )
            })
    }

    pub(super) fn render_remote_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.settings.language;
        div()
            .debug_selector(|| "remote-settings".into())
            .v_flex()
            .gap_3()
            .text_sm()
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(language, "remote.description")),
            )
            .child(tr(language, "remote.target"))
            .child(Input::new(&self.remote.target))
            .child(tr(language, "remote.root"))
            .child(Input::new(&self.remote.root))
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(language, "remote.authHelp")),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("remote-login")
                            .outline()
                            .label(tr(language, "remote.login"))
                            .on_click(cx.listener(|this, _, _, cx| this.login_remote(cx))),
                    )
                    .child(
                        Button::new("remote-connect")
                            .primary()
                            .label(tr(language, "remote.connect"))
                            .on_click(cx.listener(|this, _, _, cx| this.connect_remote(cx))),
                    ),
            )
            .when_some(self.remote.status.clone(), |view, status| {
                view.child(div().text_color(cx.theme().muted_foreground).child(status))
            })
    }
}
