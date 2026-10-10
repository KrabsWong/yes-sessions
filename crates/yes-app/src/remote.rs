use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use yes_core::remote::{RemoteCodexConfig, RemoteCodexProvider};
use yes_core::ssh::{ConnectionEvent, SshConnection};

pub(super) struct RemoteSettings {
    name: Entity<InputState>,
    selected_server: Option<usize>,
    edit_error: Option<String>,
    editing: bool,
    active_name: String,
    active_server: Option<usize>,
    target: Entity<InputState>,
    root: Entity<InputState>,
    pub(super) status: Option<String>,
    connection: Option<SshConnection>,
    state: ConnectionState,
    prompt: Option<AuthPrompt>,
    pub(super) sessions_ready: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    Idle,
    Connecting,
    Connected,
    Reconnecting,
    Authenticating,
    Paused,
}

struct AuthPrompt {
    address_visible: bool,
    id: u64,
    text: String,
    confirm: bool,
    input: Entity<InputState>,
    _subscription: Subscription,
}

fn mask_address_token(text: &str, address: &str) -> String {
    if address.is_empty() {
        return text.to_owned();
    }
    let is_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    let mut result = String::new();
    let mut start = 0;
    for (index, matched) in text.match_indices(address) {
        let end = index + matched.len();
        if text[..index].chars().next_back().is_some_and(is_name)
            || text[end..].chars().next().is_some_and(is_name)
        {
            continue;
        }
        result.push_str(&text[start..index]);
        result.push_str("********");
        start = end;
    }
    result.push_str(&text[start..]);
    result
}

fn masked_ssh_target(target: &str) -> String {
    let (user, host) = match target.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, target),
    };
    let host = if host.contains(':') {
        "[***:***]".to_owned()
    } else {
        host.split('.').map(|_| "***").collect::<Vec<_>>().join(".")
    };
    match user {
        Some(user) => format!("{}***@{host}", user.chars().next().unwrap_or('*')),
        None => host,
    }
}

fn mask_remote_address(text: &str, target: &str) -> String {
    let mut text = text.to_owned();
    if !target.is_empty() {
        text = mask_address_token(&text, target);
        if let Some((_, host)) = target.rsplit_once('@') {
            text = mask_address_token(&text, host);
        }
    }
    // SSH config aliases may resolve to a hostname absent from the saved target.
    let boundary = |c: char| !c.is_ascii_alphanumeric() && !matches!(c, '.' | '-');
    text = text
        .split_inclusive(boundary)
        .map(|part| {
            let token = part.trim_end_matches(boundary);
            let host = token.trim_end_matches('.');
            let labels: Vec<_> = host.split('.').collect();
            let domain = labels.len() > 1
                && labels.iter().all(|label| !label.is_empty())
                && labels.last().is_some_and(|label| {
                    label.len() >= 2 && label.chars().all(|c| c.is_ascii_alphabetic())
                });
            if domain {
                format!("********{}", &part[token.len()..])
            } else {
                part.to_owned()
            }
        })
        .collect();
    // SSH may print a resolved IP even when the configured destination is an alias.
    text.split_inclusive(|c: char| !c.is_ascii_hexdigit() && !matches!(c, '.' | ':' | '[' | ']'))
        .map(|part| {
            let token = part.trim_end_matches(|c: char| {
                !c.is_ascii_hexdigit() && !matches!(c, '.' | ':' | '[' | ']')
            });
            let address = token.trim_matches(['[', ']']).trim_end_matches('.');
            if address.parse::<std::net::IpAddr>().is_ok()
                || token.parse::<std::net::SocketAddr>().is_ok()
            {
                format!("********{}", &part[token.len()..])
            } else {
                part.to_owned()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn ssh_alias_resolved_domains_are_redacted() {
        let text = "The authenticity of host 'private.example.com (192.0.2.10)' can't be established. SHA256:abcdef1234";
        let masked = mask_remote_address(text, "cloud-dev");
        assert!(!masked.contains("private.example.com"));
        assert!(!masked.contains("192.0.2.10"));
        assert!(masked.contains("SHA256:abcdef1234"));
        assert!(
            !mask_remote_address(
                "Could not resolve hostname private.example.com: unknown",
                "cloud-dev"
            )
            .contains("private.example.com")
        );
    }

    #[test]
    fn ssh_target_hint_masks_users_and_all_host_components() {
        assert_eq!(
            masked_ssh_target("ubuntu@192.0.2.10"),
            "u***@***.***.***.***"
        );
        assert_eq!(masked_ssh_target("alice@private.example"), "a***@***.***");
        assert_eq!(masked_ssh_target("my-private-host"), "***");
        assert_eq!(masked_ssh_target("root@[2001:db8::1]"), "r***@[***:***]");
    }

    #[test]
    fn masks_configured_and_resolved_addresses_without_hiding_fingerprints() {
        let text = "ubuntu@192.0.2.10: Permission denied. Connection to 192.0.2.10 port 22";
        let masked = mask_remote_address(text, "ubuntu@192.0.2.10");
        assert!(!masked.contains("192.0.2.10"));
        assert!(!masked.contains("ubuntu"));
        assert!(masked.contains("Permission denied"));
        let masked = mask_remote_address(
            "cloud-dev (192.0.2.20), [2001:db8::1]:22 SHA256:example-fingerprint",
            "cloud-dev",
        );
        assert!(!masked.contains("192.0.2.20"));
        assert!(!masked.contains("2001:db8::1"));
        assert!(masked.contains("SHA256:example-fingerprint"));
    }

    #[test]
    fn short_alias_does_not_change_other_words() {
        assert_eq!(
            mask_remote_address("a: password SHA256:example-fingerprint", "a"),
            "********: password SHA256:example-fingerprint"
        );
    }

    #[gpui_kit::test]
    fn server_profiles_can_be_saved_renamed_and_deleted(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(800.)), YesSessions::new);
        let path = std::env::temp_dir().join(format!("yes-profile-ui-{}.json", std::process::id()));
        window
            .update(cx, |app, window, cx| {
                app.settings_store = SettingsStore::new(path.clone());
                app.settings.remote_servers.clear();
                app.settings.remote_codex = None;
                app.edit_remote_server(None, window, cx);
                app.remote
                    .name
                    .update(cx, |input, cx| input.set_value("Work", window, cx));
                app.remote
                    .target
                    .update(cx, |input, cx| input.set_value("work-host", window, cx));
                assert!(app.save_remote_server(cx));
                app.edit_remote_server(None, window, cx);
                app.remote
                    .name
                    .update(cx, |input, cx| input.set_value("work", window, cx));
                app.remote
                    .target
                    .update(cx, |input, cx| input.set_value("other-host", window, cx));
                assert!(!app.save_remote_server(cx));
                app.remote
                    .name
                    .update(cx, |input, cx| input.set_value("Personal", window, cx));
                assert!(app.save_remote_server(cx));
                assert_eq!(app.settings.remote_servers.len(), 2);
                app.remote.active_server = Some(1);
                app.remote.active_name = "Personal".into();
                app.edit_remote_server(Some(0), window, cx);
                app.delete_remote_server(window, cx);
                assert_eq!(app.remote.active_server, Some(0));
                assert_eq!(app.remote.config(cx).ssh_alias, "other-host");
                app.remote
                    .name
                    .update(cx, |input, cx| input.set_value("Home", window, cx));
                assert!(app.save_remote_server(cx));
                assert_eq!(app.remote.active_name, "Home");
                assert_eq!(app.settings_store.load().remote_servers[0].name, "Home");
                app.settings.remote_codex = Some(app.remote.config(cx));
                app.delete_remote_server(window, cx);
                assert!(app.settings_store.load().remote_servers.is_empty());
                assert!(app.settings.remote_codex.is_none());
                assert!(app.remote.active_server.is_none());
            })
            .unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[gpui_kit::test]
    fn deleting_duplicate_destination_preserves_active_connection_identity(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1000.), px(800.)), YesSessions::new);
        let path = std::env::temp_dir().join(format!(
            "yes-duplicate-profile-ui-{}.json",
            std::process::id()
        ));
        window
            .update(cx, |app, window, cx| {
                app.settings_store = SettingsStore::new(path.clone());
                let config = RemoteCodexConfig {
                    ssh_alias: "shared-host".into(),
                    ..Default::default()
                };
                app.settings.remote_servers = ["A", "B"]
                    .into_iter()
                    .map(|name| yes_core::RemoteServer {
                        name: name.into(),
                        config: config.clone(),
                    })
                    .collect();
                app.settings.remote_codex = Some(config.clone());
                app.remote_provider =
                    Some(Arc::new(RemoteCodexProvider::new(config.clone()).unwrap()));
                app.remote.active_server = Some(1);
                app.remote.active_name = "B".into();
                app.edit_remote_server(Some(0), window, cx);
                app.delete_remote_server(window, cx);
                assert_eq!(app.remote.active_server, Some(0));
                assert_eq!(app.settings.remote_codex, Some(config));
                assert_eq!(
                    app.redact_remote_address("shared-host: denied"),
                    "********: denied"
                );
                assert!(app.remote_provider.is_some());
            })
            .unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[gpui_kit::test]
    fn authentication_prompt_never_exposes_destination(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(900.), px(700.)), YesSessions::new);
        window
            .update(cx, |app, _, _| {
                app.settings.remote_codex = Some(RemoteCodexConfig {
                    ssh_alias: "ubuntu@192.0.2.10".into(),
                    ..Default::default()
                });
                let prompt =
                    app.remote_auth_prompt_text("ubuntu@private.example's password:", false);
                assert!(!prompt.contains("private.example"));
                assert!(!prompt.contains("ubuntu"));
                let prompt =
                    app.remote_auth_prompt_text("Passphrase for /home/private/key:", false);
                assert!(!prompt.contains("/home/private"));
                let prompt = app.remote_auth_prompt_text("192.0.2.10 SHA256:fingerprint", true);
                assert!(!prompt.contains("192.0.2.10"));
                assert!(prompt.contains("SHA256:fingerprint"));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn reconnect_preserves_selection_and_authentication_is_transient(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(900.), px(700.)), YesSessions::new);
        window
            .update(cx, |app, window, cx| {
                app.sessions_generation += 1;
                app.remote.sessions_ready = true;
                app.selected_session_id = Some("selected-remote-session".into());
                let sessions = app.sessions.clone();
                app.handle_connection_event(
                    ConnectionEvent::Reconnecting { attempt: 1 },
                    window,
                    cx,
                );
                assert_eq!(app.remote.state, ConnectionState::Reconnecting);
                app.handle_connection_event(ConnectionEvent::Connected, window, cx);
                assert!(
                    app.loading_detail,
                    "resume an interrupted first detail read"
                );
                assert_eq!(
                    app.selected_session_id.as_deref(),
                    Some("selected-remote-session")
                );
                assert!(Arc::ptr_eq(&sessions, &app.sessions));
                app.handle_connection_event(
                    ConnectionEvent::Prompt {
                        id: 7,
                        text: "Password:".into(),
                        confirm: false,
                    },
                    window,
                    cx,
                );
                let input = app.remote.prompt.as_ref().unwrap().input.downgrade();
                app.remote
                    .prompt
                    .as_ref()
                    .unwrap()
                    .input
                    .update(cx, |input, cx| {
                        input.set_value("temporary-test-input", window, cx);
                    });
                app.answer_remote_prompt(false, window, cx);
                assert!(app.remote.prompt.is_none());
                assert_eq!(app.remote.state, ConnectionState::Paused);
                // GPUI may retain the entity until the next frame; its contents
                // and undo history must already be cleared when the prompt closes.
                if let Some(input) = input.upgrade() {
                    assert!(input.read(cx).value().is_empty());
                }
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn authentication_accepts_keyboard_input_and_escape_clears_it(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(760.), px(650.)), YesSessions::new);
        let input = window
            .update(cx, |app, window, cx| {
                app.handle_connection_event(
                    ConnectionEvent::Prompt {
                        id: 99,
                        text: "Password:".into(),
                        confirm: false,
                    },
                    window,
                    cx,
                );
                app.remote.prompt.as_ref().unwrap().input.clone()
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        visual.simulate_input("keyboard-test");
        assert_eq!(
            input.read_with(&visual, |input, _| input.value().to_string()),
            "keyboard-test"
        );
        visual.simulate_keystrokes("backspace");
        assert_eq!(
            input.read_with(&visual, |input, _| input.value().to_string()),
            "keyboard-tes"
        );
        visual.simulate_keystrokes("escape");
        assert!(input.read_with(&visual, |input, _| input.value().is_empty()));
    }

    #[gpui_kit::test]
    fn authentication_dialog_fits_both_languages_and_host_confirmation(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(760.), px(650.)), YesSessions::new);
        for language in [Language::Zh, Language::En] {
            for confirm in [false, true] {
                window
                    .update(cx, |app, window, cx| {
                        app.settings.language = language;
                        app.handle_connection_event(
                            ConnectionEvent::Prompt {
                                id: 1,
                                text: "remote.example.com\nSHA256:example-fingerprint".into(),
                                confirm,
                            },
                            window,
                            cx,
                        );
                    })
                    .unwrap();
                let mut visual = VisualTestContext::from_window(*window, cx);
                visual.run_until_parked();
                let bounds = visual.debug_bounds("remote-auth-dialog").unwrap();
                assert!(bounds.size.width <= px(500.));
                assert!(bounds.size.height < px(650.));
            }
        }
    }

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
            app.update(&mut visual, |app, cx| {
                app.remote.editing = true;
                cx.notify();
            });
            visual.run_until_parked();
            let bounds = visual.debug_bounds("remote-settings").unwrap();
            assert!(bounds.size.height < px(650.));
        }
    }
}

impl RemoteSettings {
    pub(super) fn new(
        settings: &yes_core::settings::AppSettings,
        window: &mut Window,
        cx: &mut Context<YesSessions>,
    ) -> Self {
        let selected_server = settings
            .remote_servers
            .iter()
            .position(|server| Some(&server.config) == settings.remote_codex.as_ref())
            .or_else(|| (!settings.remote_servers.is_empty()).then_some(0));
        let saved = selected_server.and_then(|i| settings.remote_servers.get(i));
        let config = saved.map(|s| s.config.clone()).unwrap_or_default();
        let name = cx.new(|cx| {
            let mut input =
                InputState::new(window, cx).placeholder(tr(settings.language, "remote.serverName"));
            input.set_value(
                saved.map(|s| s.name.clone()).unwrap_or_default(),
                window,
                cx,
            );
            input
        });
        let target = cx.new(|cx| {
            let mut input = InputState::new(window, cx)
                .placeholder("user@server / SSH Host")
                .masked(true);
            input.set_value(config.ssh_alias, window, cx);
            input
        });
        let root = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("~/.codex");
            input.set_value(config.root, window, cx);
            input
        });
        Self {
            name,
            selected_server,
            edit_error: None,
            editing: false,
            active_name: String::new(),
            active_server: None,
            target,
            root,
            status: None,
            connection: None,
            state: ConnectionState::Idle,
            prompt: None,
            sessions_ready: false,
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
    fn edit_remote_server(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let server = index.and_then(|i| self.settings.remote_servers.get(i));
        let name = server.map(|s| s.name.clone()).unwrap_or_default();
        let config = server.map(|s| s.config.clone()).unwrap_or_default();
        self.remote
            .name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.remote.target.update(cx, |input, cx| {
            input.set_value(config.ssh_alias, window, cx);
            input.set_masked(true, window, cx);
        });
        self.remote
            .root
            .update(cx, |input, cx| input.set_value(config.root, window, cx));
        self.remote.editing = true;
        self.remote.selected_server = index;
        self.remote.edit_error = None;
        cx.notify();
    }

    fn save_remote_server(&mut self, cx: &mut Context<Self>) -> bool {
        let name = self.remote.name.read(cx).value().trim().to_owned();
        let config = self.remote.config(cx);
        if name.is_empty()
            || self
                .settings
                .remote_servers
                .iter()
                .enumerate()
                .any(|(i, s)| {
                    Some(i) != self.remote.selected_server && s.name.eq_ignore_ascii_case(&name)
                })
        {
            self.remote.edit_error = Some(tr(self.settings.language, "remote.nameInvalid").into());
            cx.notify();
            return false;
        }
        if let Err(error) = config.validate() {
            self.remote.edit_error = Some(error.to_string());
            cx.notify();
            return false;
        }
        let server = yes_core::settings::RemoteServer { name, config };
        if let Some(index) = self.remote.selected_server {
            if self.remote.active_server == Some(index) {
                self.remote.active_name = server.name.clone();
            }
            self.settings.remote_servers[index] = server;
        } else {
            self.remote.selected_server = Some(self.settings.remote_servers.len());
            self.settings.remote_servers.push(server);
        }
        self.save_settings();
        self.remote.editing = false;
        self.remote.edit_error = None;
        cx.notify();
        true
    }

    fn delete_remote_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.remote.selected_server {
            let removed = self.settings.remote_servers.remove(index);
            if self.remote.active_server == Some(index) {
                self.select_local(cx);
            } else if let Some(active) = self.remote.active_server.as_mut() {
                if *active > index {
                    *active -= 1;
                }
            }
            if (self.remote_provider.is_none()
                && self.settings.remote_codex.as_ref() == Some(&removed.config))
                || self.settings.remote_servers.is_empty()
            {
                self.settings.remote_codex = None;
            }
            self.save_settings();
            let next = (!self.settings.remote_servers.is_empty()).then_some(0);
            self.edit_remote_server(next, window, cx);
            self.remote.editing = false;
        }
    }

    pub(super) fn redact_remote_address(&self, text: &str) -> String {
        let target = self
            .settings
            .remote_codex
            .as_ref()
            .map(|c| c.ssh_alias.as_str())
            .unwrap_or_default();
        mask_remote_address(text, target)
    }

    fn remote_display_text(&self, text: &str) -> String {
        self.redact_remote_address(text)
    }

    fn remote_auth_prompt_text(&self, prompt: &str, confirm: bool) -> String {
        if confirm {
            self.redact_remote_address(prompt)
        } else {
            tr(
                self.settings.language,
                if prompt.to_ascii_lowercase().contains("passphrase") {
                    "remote.enterPassphrase"
                } else {
                    "remote.enterPassword"
                },
            )
            .into()
        }
    }

    pub(super) fn render_remote_bar(&self) -> impl IntoElement {
        let language = self.settings.language;
        let busy = self.loading_sessions || self.refreshing_sessions || self.loading_detail;
        let failed = matches!(
            self.remote.state,
            ConnectionState::Paused | ConnectionState::Reconnecting
        ) || (!busy && self.remote.status.is_some());
        let status = if !self.remote_ready() {
            self.remote_connection_label()
        } else if busy {
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
                                !self.remote_ready()
                                    || self.loading_sessions
                                    || self.refreshing_sessions
                                    || self.loading_detail,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_sessions(cx))),
                    ),
            )
            .when_some(self.remote.status.clone(), |view, status| {
                view.child(self.remote_display_text(&status))
            })
            .child(
                div()
                    .flex()
                    .gap_2()
                    .when(self.remote.state == ConnectionState::Paused, |view| {
                        view.child(
                            Button::new("remote-retry")
                                .ghost()
                                .compact()
                                .label(tr(language, "remote.retry"))
                                .on_click(cx.listener(|this, _, _, cx| this.retry_remote(cx))),
                        )
                    })
                    .child(
                        Button::new("remote-disconnect")
                            .ghost()
                            .compact()
                            .label(tr(language, "remote.disconnect"))
                            .on_click(cx.listener(|this, _, _, cx| this.select_local(cx))),
                    ),
            )
    }

    pub(super) fn active_provider(&self) -> Option<Arc<dyn yes_core::SessionProvider>> {
        match &self.remote_provider {
            Some(provider) => Some(provider.clone()),
            None => self.registry.get(self.selected_app),
        }
    }

    pub(super) fn remote_connection_label(&self) -> &'static str {
        match self.remote.state {
            ConnectionState::Authenticating => "remote.authRequired",
            ConnectionState::Reconnecting => "remote.reconnecting",
            ConnectionState::Paused | ConnectionState::Idle => "remote.paused",
            _ => "remote.connecting",
        }
    }

    pub(super) fn remote_ready(&self) -> bool {
        self.remote_provider.is_none() || self.remote.state == ConnectionState::Connected
    }

    pub(super) fn stop_remote_connection(&mut self) {
        if let Some(connection) = self.remote.connection.take() {
            connection.disconnect();
        }
        self.remote.prompt = None;
        self.remote.active_server = None;
        self.remote.state = ConnectionState::Idle;
        self.remote.sessions_ready = false;
    }

    fn connect_remote_config(
        &mut self,
        config: RemoteCodexConfig,
        name: String,
        cx: &mut Context<Self>,
    ) {
        if self.remote.connection.is_some() && self.settings.remote_codex.as_ref() == Some(&config)
        {
            if self.remote.state == ConnectionState::Paused {
                self.retry_remote(cx);
            }
            self.remote.active_server = self
                .settings
                .remote_servers
                .iter()
                .position(|s| s.name == name);
            self.remote.active_name = name;
            self.settings_open = false;
            cx.notify();
            return;
        }
        let result = std::env::current_exe()
            .map_err(anyhow::Error::from)
            .and_then(|exe| SshConnection::start(config.clone(), exe))
            .and_then(|connection| {
                let provider = RemoteCodexProvider::with_connection(
                    config.clone(),
                    connection.control_path(),
                )?;
                Ok((connection, provider))
            });
        match result {
            Ok((connection, provider)) => {
                self.stop_remote_connection();
                if let Some(previous) = self.remote_provider.take() {
                    previous.cancel();
                }
                self.remote.active_server = self
                    .settings
                    .remote_servers
                    .iter()
                    .position(|s| s.name == name);
                self.remote.active_name = name;
                self.settings.remote_codex = Some(config);
                self.save_settings();
                self.remote.connection = Some(connection);
                self.remote.state = ConnectionState::Connecting;
                self.remote_provider = Some(Arc::new(provider));
                self.dashboard_open = false;
                self.dashboard_session = None;
                self.selected_app = AppType::Codex;
                self.collapsed_groups.clear();
                self.expanded_parents.clear();
                self.group_visible_counts.clear();
                self.settings_open = false;
                self.remote.status = None;
                self.clear_sessions(cx);
                self.loading_sessions = false;
            }
            Err(error) => self.remote.status = Some(error.to_string()),
        }
        cx.notify();
    }

    fn select_local(&mut self, cx: &mut Context<Self>) {
        self.stop_remote_connection();
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

    fn retry_remote(&mut self, cx: &mut Context<Self>) {
        if let Some(connection) = &self.remote.connection {
            self.remote.prompt = None;
            self.remote.status = None;
            self.remote.state = ConnectionState::Connecting;
            connection.retry();
        }
        cx.notify();
    }

    pub(super) fn start_remote_monitor(&self, window: &Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        let events = this
                            .remote
                            .connection
                            .as_ref()
                            .map(|connection| connection.poll())
                            .unwrap_or_default();
                        for event in events {
                            this.handle_connection_event(event, window, cx);
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

    fn handle_connection_event(
        &mut self,
        event: ConnectionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ConnectionEvent::Connecting => self.remote.state = ConnectionState::Connecting,
            ConnectionEvent::Connected => {
                self.remote.prompt = None;
                self.remote.state = ConnectionState::Connected;
                self.remote.status = None;
                if !self.remote.sessions_ready {
                    self.load_sessions(cx);
                } else if self.detail.is_none()
                    && let Some(id) = self.selected_session_id.clone()
                {
                    self.load_session(id, cx);
                }
            }
            ConnectionEvent::Reconnecting { .. } => {
                self.remote.prompt = None;
                self.remote.state = ConnectionState::Reconnecting;
                self.cancel_remote_reads();
            }
            ConnectionEvent::Prompt { id, text, confirm } => {
                let input = cx.new(|cx| InputState::new(window, cx).masked(true));
                let subscription =
                    cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            this.answer_remote_prompt(true, window, cx);
                        }
                    });
                if confirm {
                    self.root_focus.focus(window, cx);
                } else {
                    input.update(cx, |input, cx| input.focus(window, cx));
                }
                self.remote.prompt = Some(AuthPrompt {
                    address_visible: false,
                    id,
                    text,
                    confirm,
                    input,
                    _subscription: subscription,
                });
                self.remote.state = ConnectionState::Authenticating;
            }
            ConnectionEvent::Failed(error) => {
                self.remote.prompt = None;
                self.remote.status = Some(error);
                self.remote.state = ConnectionState::Paused;
                self.cancel_remote_reads();
            }
            ConnectionEvent::Cancelled => {
                self.remote.prompt = None;
                self.remote.state = ConnectionState::Paused;
                self.cancel_remote_reads();
            }
        }
        cx.notify();
    }

    fn cancel_remote_reads(&mut self) {
        if let Some(provider) = &self.remote_provider {
            provider.cancel();
        }
        self.sessions_generation += 1;
        self.detail_generation += 1;
        self.loading_sessions = false;
        self.refreshing_sessions = false;
        self.loading_detail = false;
        self.refreshing_detail = false;
    }

    pub(super) fn answer_remote_prompt(
        &mut self,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(prompt) = self.remote.prompt.take() {
            let answer = accept.then(|| {
                if prompt.confirm {
                    "yes".to_owned()
                } else {
                    prompt.input.read(cx).value().to_string()
                }
            });
            prompt
                .input
                .update(cx, |input, cx| input.set_value("", window, cx));
            if let Some(connection) = &self.remote.connection {
                connection.respond(prompt.id, answer);
            }
            self.remote.state = if accept {
                ConnectionState::Connecting
            } else {
                ConnectionState::Paused
            };
            if !accept {
                self.cancel_remote_reads();
            }
        }
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_remote_auth(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.settings.language;
        let prompt = self.remote.prompt.as_ref().unwrap();
        let target = self.settings.remote_codex.as_ref().map(|config| {
            if prompt.address_visible {
                config.ssh_alias.clone()
            } else {
                masked_ssh_target(&config.ssh_alias)
            }
        });
        let visibility_label = tr(
            language,
            if prompt.address_visible {
                "remote.hideAddress"
            } else {
                "remote.showAddress"
            },
        );
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(gpui_kit::black().opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .debug_selector(|| "remote-auth-dialog".into())
                    .v_flex()
                    .gap_4()
                    .p_6()
                    .w(px(500.))
                    .max_h(relative(0.85))
                    .bg(cx.theme().popover)
                    .text_color(cx.theme().foreground)
                    .rounded_lg()
                    .shadow_lg()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(tr(
                        language,
                        if prompt.confirm {
                            "remote.verifyHost"
                        } else {
                            "remote.authenticate"
                        },
                    )))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .flex_wrap()
                            .child(div().text_sm().child(self.remote.active_name.clone()))
                            .when_some(target, |view, target| {
                                view.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("(")
                                        .child(target)
                                        .child(
                                            Button::new("auth-address-visibility")
                                                .ghost()
                                                .compact()
                                                .size(px(24.))
                                                .icon(
                                                    Icon::new(if prompt.address_visible {
                                                        IconName::EyeOff
                                                    } else {
                                                        IconName::Eye
                                                    })
                                                    .text_color(cx.theme().muted_foreground),
                                                )
                                                .tooltip(visibility_label)
                                                .accessibility_label(visibility_label)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    if let Some(prompt) =
                                                        this.remote.prompt.as_mut()
                                                    {
                                                        prompt.address_visible =
                                                            !prompt.address_visible;
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                        .child(")"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("ssh-auth-prompt")
                            .max_h(px(200.))
                            .overflow_y_scroll()
                            .text_sm()
                            .child(self.remote_auth_prompt_text(&prompt.text, prompt.confirm)),
                    )
                    .when(!prompt.confirm, |view| {
                        view.child(Input::new(&prompt.input))
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr(
                                language,
                                if prompt.confirm {
                                    "remote.verifyHelp"
                                } else {
                                    "remote.noPasswordStorage"
                                },
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("ssh-auth-cancel")
                                    .outline()
                                    .label(tr(language, "remote.cancel"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.answer_remote_prompt(false, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("ssh-auth-submit")
                                    .primary()
                                    .label(tr(
                                        language,
                                        if prompt.confirm {
                                            "remote.trustHost"
                                        } else {
                                            "remote.continue"
                                        },
                                    ))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.answer_remote_prompt(true, window, cx)
                                    })),
                            ),
                    ),
            )
    }

    pub(super) fn remote_auth_open(&self) -> bool {
        self.remote.prompt.is_some()
    }

    pub(super) fn render_source_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.settings.language;
        let label = if self.remote_provider.is_some() {
            format!("SSH · {}", self.remote.active_name)
        } else {
            tr(language, "remote.local").to_owned()
        };
        let owner = cx.weak_entity();
        let servers = self.settings.remote_servers.clone();
        let selector = Button::new("source-selector")
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
                let mut menu = menu.item(
                    PopupMenuItem::new(tr(language, "remote.local")).on_click(move |_, _, cx| {
                        let _ = local_owner.update(cx, |this, cx| this.select_local(cx));
                    }),
                );
                for server in servers.iter().cloned() {
                    let owner = owner.clone();
                    menu = menu.item(PopupMenuItem::new(server.name.clone()).on_click(
                        move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.connect_remote_config(
                                    server.config.clone(),
                                    server.name.clone(),
                                    cx,
                                );
                            });
                        },
                    ));
                }
                menu.item(
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
            });
        div().flex().items_center().gap_1().child(selector)
    }

    fn render_remote_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.settings.language;
        div()
            .flex()
            .items_center()
            .gap_2()
            .p_2()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.remote.name)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.remote.target).mask_toggle()),
            )
            .child(div().w(px(105.)).child(Input::new(&self.remote.root)))
            .child(
                Button::new("remote-server-save")
                    .ghost()
                    .compact()
                    .icon(IconName::Check)
                    .tooltip(tr(language, "remote.saveServer"))
                    .accessibility_label(tr(language, "remote.saveServer"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.save_remote_server(cx);
                    })),
            )
            .child(
                Button::new("remote-server-cancel")
                    .ghost()
                    .compact()
                    .icon(IconName::Close)
                    .tooltip(tr(language, "remote.cancel"))
                    .accessibility_label(tr(language, "remote.cancel"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote.editing = false;
                        this.remote.edit_error = None;
                        cx.notify();
                    })),
            )
            .when(self.remote.selected_server.is_some(), |view| {
                view.child(
                    Button::new("remote-server-delete")
                        .ghost()
                        .compact()
                        .icon(IconName::Delete)
                        .tooltip(tr(language, "remote.deleteServer"))
                        .accessibility_label(tr(language, "remote.deleteServer"))
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.delete_remote_server(window, cx)
                            }),
                        ),
                )
            })
    }

    pub(super) fn render_remote_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.settings.language;
        let mut list = div()
            .id("remote-server-list")
            .v_flex()
            .gap_2()
            .max_h(px(220.))
            .overflow_y_scroll();
        for (index, server) in self.settings.remote_servers.iter().enumerate() {
            if self.remote.editing && self.remote.selected_server == Some(index) {
                list = list.child(self.render_remote_editor(cx));
                continue;
            }
            let config = server.config.clone();
            let name = server.name.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_md()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(server.name.clone()),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child("********"),
                    )
                    .child(
                        div()
                            .max_w(px(140.))
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(cx.theme().muted_foreground)
                            .child(server.config.root.clone()),
                    )
                    .child(
                        Button::new(("remote-row-connect", index))
                            .ghost()
                            .compact()
                            .label(tr(language, "remote.connect"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.connect_remote_config(config.clone(), name.clone(), cx)
                            })),
                    )
                    .child(
                        Button::new(("remote-row-edit", index))
                            .ghost()
                            .compact()
                            .icon(IconName::Settings)
                            .tooltip(tr(language, "remote.editServer"))
                            .accessibility_label(tr(language, "remote.editServer"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_remote_server(Some(index), window, cx)
                            })),
                    ),
            );
        }
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
            .child(list)
            .when(
                self.remote.editing && self.remote.selected_server.is_none(),
                |view| view.child(self.render_remote_editor(cx)),
            )
            .when_some(self.remote.edit_error.clone(), |view, error| {
                view.child(self.remote_display_text(&error))
            })
            .child(
                Button::new("remote-server-add")
                    .outline()
                    .icon(IconName::Plus)
                    .label(tr(language, "remote.addServer"))
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.edit_remote_server(None, window, cx)
                        }),
                    ),
            )
    }
}
