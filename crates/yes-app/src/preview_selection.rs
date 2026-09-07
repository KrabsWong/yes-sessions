//! Selection for virtualized code rows. Coordinates refer to the cached document,
//! so copying does not depend on which rows currently have rendered elements.
use gpui_kit::*;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Position(pub usize, pub usize);

pub(crate) struct CodeSelection {
    pub focus: FocusHandle,
    pub documents: [Vec<Option<String>>; 3],
    pub originals: [Vec<Option<String>>; 3],
    side: usize,
    anchor: Position,
    cursor: Position,
    dragging: bool,
}
impl CodeSelection {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle(),
            documents: Default::default(),
            originals: Default::default(),
            side: 0,
            anchor: Position(0, 0),
            cursor: Position(0, 0),
            dragging: false,
        }
    }
    pub fn reset(&mut self, documents: [Vec<Option<String>>; 3]) {
        self.documents = documents;
        self.originals = Default::default();
        self.clear(0);
    }
    pub fn clear(&mut self, side: usize) {
        self.side = side;
        self.anchor = Position(0, 0);
        self.cursor = self.anchor;
        self.dragging = false;
    }
    fn range(&self, side: usize, row: usize, len: usize) -> Range<usize> {
        let (start, end) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        if self.side != side || row < start.0 || row > end.0 {
            return 0..0;
        }
        (if row == start.0 { start.1.min(len) } else { 0 })..(if row == end.0 {
            end.1.min(len)
        } else {
            len
        })
    }
    pub fn copy(&self) -> String {
        let (start, end) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        if start == end {
            return String::new();
        }
        self.documents[self.side]
            .iter()
            .enumerate()
            .filter(|(i, _)| *i >= start.0 && *i <= end.0)
            .filter_map(|(i, text)| {
                text.as_ref().map(|text| {
                    let range = self.range(self.side, i, text.len());
                    if let Some(Some(original)) = self.originals[self.side].get(i) {
                        let offset = |column: usize| {
                            let mut display = 0;
                            for (index, ch) in original.char_indices() {
                                if display >= column {
                                    return index;
                                }
                                display += if ch == '\t' { 4 } else { ch.len_utf8() };
                            }
                            original.len()
                        };
                        original[offset(range.start)..offset(range.end)].to_owned()
                    } else {
                        text[range].to_owned()
                    }
                })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn select_all(&mut self) {
        self.anchor = Position(0, 0);
        if let Some((index, last)) = self.documents[self.side]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, t)| t.as_ref().map(|t| (i, t)))
        {
            self.cursor = Position(index, last.len());
        }
    }
}

pub(crate) struct CodeText {
    text: StyledText,
    selection: Entity<CodeSelection>,
    side: usize,
    row: usize,
    len: usize,
    color: Hsla,
}
impl CodeText {
    pub fn new(
        text: StyledText,
        len: usize,
        selection: Entity<CodeSelection>,
        side: usize,
        row: usize,
        color: Hsla,
    ) -> Self {
        Self {
            text,
            len,
            selection,
            side,
            row,
            color,
        }
    }
}
impl IntoElement for CodeText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for CodeText {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.text.request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.text
            .prepaint(id, inspector, bounds, &mut (), window, cx);
        // Include the rest of the line so trailing whitespace and empty lines can be selected.
        window.insert_hitbox(
            Bounds::new(
                bounds.origin,
                size(
                    window.viewport_size().width - bounds.left(),
                    bounds.size.height,
                ),
            ),
            HitboxBehavior::Normal,
        )
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.text.layout().clone();
        let range = self.selection.read(cx).range(self.side, self.row, self.len);
        if range.start < range.end {
            if let (Some(a), Some(b)) = (
                layout.position_for_index(range.start),
                layout.position_for_index(range.end),
            ) {
                window.paint_quad(fill(
                    Bounds::new(a, size(b.x - a.x, layout.line_height())),
                    self.color,
                ));
            }
        }
        window.set_cursor_style(CursorStyle::IBeam, hitbox);
        let (side, row) = (self.side, self.row);
        let selection = self.selection.clone();
        let hit = hitbox.clone();
        let down_layout = layout.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase.bubble() && event.button == MouseButton::Left && hit.is_hovered(window) {
                let byte = down_layout
                    .index_for_position(event.position)
                    .unwrap_or_else(|i| i);
                selection.update(cx, |state, cx| {
                    let point = Position(
                        row,
                        byte.min(
                            state.documents[side]
                                .get(row)
                                .and_then(Option::as_ref)
                                .map_or(0, String::len),
                        ),
                    );
                    if !event.modifiers.shift || state.side != side {
                        state.anchor = point;
                    }
                    state.side = side;
                    state.cursor = point;
                    state.dragging = true;
                    window.focus(&state.focus, cx);
                    cx.notify();
                });
                window.refresh();
                cx.stop_propagation();
            }
        });
        let selection = self.selection.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase.bubble()
                && event.pressed_button == Some(MouseButton::Left)
                && event.position.y >= bounds.top()
                && event.position.y < bounds.bottom()
            {
                let active = selection.read(cx);
                if active.dragging && active.side == side {
                    let byte = layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|i| i);
                    selection.update(cx, |state, cx| {
                        state.cursor = Position(
                            row,
                            byte.min(
                                state.documents[side]
                                    .get(row)
                                    .and_then(Option::as_ref)
                                    .map_or(0, String::len),
                            ),
                        );
                        cx.notify();
                    });
                    window.refresh();
                }
            }
        });
        let selection = self.selection.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase.bubble() && event.button == MouseButton::Left {
                selection.update(cx, |state, _| state.dragging = false);
            }
        });
        self.text
            .paint(id, inspector, bounds, &mut (), &mut (), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{CodeSelection, Position};
    #[gpui_kit::test]
    fn reverse_unicode_selection_preserves_tabs_and_skips_alignment_rows(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            let mut state = CodeSelection::new(cx);
            state.reset([
                vec![Some("    你好".into()), None, Some("next".into())],
                vec![],
                vec![],
            ]);
            state.originals[0] = vec![Some("\t你好".into()), None, None];
            state.anchor = Position(2, 4);
            state.cursor = Position(0, 0);
            assert_eq!(state.copy(), "\t你好\nnext");
            state.anchor = Position(0, 7);
            state.cursor = Position(0, 4);
            assert_eq!(state.copy(), "你");
            state.clear(1);
            assert!(state.copy().is_empty());
        });
    }
}
