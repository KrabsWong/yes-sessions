use crate::i18n::tr;
use gpui_kit::*;
use yes_core::Language;

actions!(
    yes_sessions,
    [
        Quit,
        CloseWindow,
        MinimizeWindow,
        FindInSession,
        FindInAgent
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-f", FindInSession, None),
        KeyBinding::new("cmd-shift-f", FindInAgent, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    // This single-window app keeps the live workspace when dismissed with Cmd-W.
    // Hiding preserves window-bound inputs, previews and scroll handles for Dock reopen.
    cx.on_action(|_: &CloseWindow, cx| cx.hide());
    cx.on_action(|_: &MinimizeWindow, cx| {
        if let Some(window) = cx.active_window() {
            cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, _| window.minimize_window());
            });
        }
    });
}

/// Dock activation reuses the existing window and its view entities, including
/// when macOS has miniaturized it. Only a closed window needs a new view.
pub fn restore_existing_window(cx: &mut App) -> bool {
    let Some(window) = cx.windows().first().copied() else {
        return false;
    };
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, _| window.activate_window());
    });
    true
}

pub fn update_menus(language: Language, cx: &mut App) {
    cx.set_menus(vec![
        Menu::new("Yes Sessions").items([MenuItem::action(tr(language, "menu.quit"), Quit)]),
        Menu::new(tr(language, "menu.file"))
            .items([MenuItem::action(tr(language, "menu.close"), CloseWindow)]),
        Menu::new(tr(language, "menu.window")).items([MenuItem::action(
            tr(language, "menu.minimize"),
            MinimizeWindow,
        )]),
    ]);
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        Context, InteractiveElement, IntoElement, Render, TestAppContext, Window, div, px, size,
    };

    struct TestWindow;
    impl Render for TestWindow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().debug_selector(|| "shortcut-test-root".into())
        }
    }

    #[gpui_kit::test]
    fn restoring_an_existing_window_reuses_its_view(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(300.), px(200.)), |_, _| TestWindow);
        let original = window.root(cx).unwrap().entity_id();
        assert!(cx.update(super::restore_existing_window));
        cx.run_until_parked();
        assert_eq!(window.root(cx).unwrap().entity_id(), original);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        assert!(!cx.update(super::restore_existing_window));
    }
}
