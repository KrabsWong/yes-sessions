//! Bound layout work for oversized pasted text instead of laying out one enormous Markdown node.
use gpui_kit::component::{
    ActiveTheme as _,
    input::{Editor, EditorState},
};
use gpui_kit::{
    App, AppContext, Entity, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, px,
};
use yes_core::Language;

use crate::i18n::tr;

pub(crate) fn needs_virtual_text(text: &str) -> bool {
    text.len() > 32 * 1024 || text.bytes().filter(|byte| *byte == b'\n').take(400).count() == 400
}

struct LargeMessageState {
    source: SharedString,
    editor: Entity<EditorState>,
}

#[derive(IntoElement)]
pub(crate) struct LargeMessage {
    pub id: usize,
    pub source: SharedString,
    pub language: Language,
}

impl RenderOnce for LargeMessage {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let source = self.source.clone();
        let state = window.use_keyed_state(("large-message", self.id), cx, |window, cx| {
            let editor = cx.new(|cx| {
                let mut editor = EditorState::new(window, cx).line_number(true);
                editor.set_value(source.clone(), window, cx);
                editor
            });
            LargeMessageState { source, editor }
        });
        let editor = state.read(cx).editor.clone();
        if state.read(cx).source != self.source {
            editor.update(cx, |editor, cx| {
                editor.set_value(self.source.clone(), window, cx)
            });
            state.update(cx, |state, _| state.source = self.source);
        }
        div()
            .debug_selector(|| "large-message".into())
            .w_full()
            .min_w_0()
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .text_size(px(12.))
                    .px_2()
                    .py_1()
                    .bg(cx.theme().muted)
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(self.language, "message.largeText")),
            )
            .child(
                Editor::new(&editor)
                    .readonly(true)
                    .h(px(360.))
                    .w_full()
                    .text_size(px(13.)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        Context, Render, ScrollDelta, ScrollWheelEvent, TouchPhase, VisualTestContext, point, size,
    };
    #[test]
    fn limits_large_lines_and_many_short_lines() {
        assert!(!needs_virtual_text("ordinary **message**"));
        assert!(needs_virtual_text(&"x".repeat(32 * 1024 + 1)));
        assert!(needs_virtual_text(&"x\n".repeat(400)));
    }

    struct TestView {
        source: SharedString,
    }
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            LargeMessage {
                id: 0,
                source: self.source.clone(),
                language: Language::En,
            }
        }
    }
    #[gpui_kit::test]
    fn large_paste_has_bounded_layout(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let source: SharedString = std::env::var("YES_LARGE_MESSAGE_FIXTURE")
            .ok()
            .map(|path| std::fs::read_to_string(path).unwrap())
            .unwrap_or_else(|| {
                format!(
                    "{}{}\nEND",
                    "let value = some_function(argument);\n".repeat(8221),
                    "x".repeat(40_000)
                )
            })
            .into();
        let window = cx.open_window(size(px(600.), px(500.)), |_, _| TestView {
            source: source.clone(),
        });
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("large-message").unwrap();
        assert!(bounds.size.height <= px(500.));
        for _ in 0..20 {
            visual.simulate_event(ScrollWheelEvent {
                position: bounds.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-1800.))),
                modifiers: Default::default(),
                touch_phase: TouchPhase::Moved,
            });
            visual.run_until_parked();
        }
        window
            .update(cx, |_, window, cx| {
                let editor = cx.new(|cx| {
                    let mut editor = EditorState::new(window, cx);
                    editor.set_value(source.clone(), window, cx);
                    editor
                });
                editor.update(cx, |editor, cx| {
                    editor.select_all(window, cx);
                    assert_eq!(editor.selected_text().to_string(), source.as_ref());
                    editor.set_value("replacement", window, cx);
                    assert_eq!(editor.value().as_ref(), "replacement");
                });
            })
            .unwrap();
    }
}
