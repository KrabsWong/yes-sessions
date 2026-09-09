use gpui::{
    AppContext, Context, Element, IntoElement, ListState, Render, ScrollDelta, ScrollWheelEvent,
    Styled, TestAppContext, VisualTestContext, Window, div, list, point, px, size,
};
use std::{cell::Cell, rc::Rc};

struct TestView {
    state: ListState,
    scale: Rc<Cell<f32>>,
    renders: Rc<Cell<usize>>,
}
impl Render for TestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let scale = self.scale.get();
        let renders = self.renders.clone();
        list(self.state.clone(), move |ix, _, _| {
            renders.set(renders.get() + 1);
            div()
                .h(px(if ix % 2 == 0 { 40. } else { 60. }) * scale)
                .w_full()
                .into_any_element()
        })
        .size_full()
    }
}

fn draw(cx: &mut VisualTestContext, view: &gpui::Entity<TestView>, width: f32) {
    cx.draw(point(px(0.), px(0.)), size(px(width), px(200.)), |_, _| {
        view.clone().into_any_element()
    });
}

fn fixture(cx: &mut VisualTestContext) -> (ListState, gpui::Entity<TestView>) {
    let state = ListState::new(1000, gpui::ListAlignment::Top, px(400.));
    let view = cx.update(|_, cx| {
        cx.new(|_| TestView {
            state: state.clone(),
            scale: Rc::new(Cell::new(1.)),
            renders: Rc::new(Cell::new(0)),
        })
    });
    draw(cx, &view, 100.);
    state.scroll_to_end();
    draw(cx, &view, 100.);
    assert_eq!(state.logical_scroll_top().item_ix, 996);
    (state, view)
}

#[gpui::test]
fn test_scroll_across_unmeasured_rows_keeps_nearby_anchor(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, view) = fixture(cx);
    state.scroll_by(px(-900.));
    draw(cx, &view, 100.);
    assert_eq!(
        state.logical_scroll_top().item_ix,
        978,
        "a 900px scroll must not skip unmeasured history"
    );
    state.scroll_to(gpui::ListOffset {
        item_ix: 0,
        offset_in_item: px(0.),
    });
    draw(cx, &view, 100.);
    state.scroll_by(px(900.));
    draw(cx, &view, 100.);
    assert_eq!(state.logical_scroll_top().item_ix, 18);
    let renders = cx.update(|_, cx| view.read(cx).renders.get());
    assert!(
        renders < 150,
        "scrolling should only measure its path and viewport: {renders}"
    );
    draw(cx, &view, 100.);
    let redraw_renders = cx.update(|_, cx| view.read(cx).renders.get()) - renders;
    assert!(
        redraw_renders < 30,
        "redrawing should only refresh the viewport and overdraw: {redraw_renders}"
    );
}

#[gpui::test]
fn test_coalesced_wheel_events_preserve_painted_anchor(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, view) = fixture(cx);
    for delta in [900., 300.] {
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        });
    }
    draw(cx, &view, 100.);
    assert_eq!(state.logical_scroll_top().item_ix, 972);
    for delta in [-700., -500.] {
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        });
    }
    draw(cx, &view, 100.);
    assert_eq!(state.logical_scroll_top().item_ix, 996);
}

#[gpui::test]
fn test_pending_scroll_uses_new_heights_after_width_change(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, view) = fixture(cx);
    state.scroll_by(px(-900.));
    cx.update(|_, cx| view.read(cx).scale.set(2.));
    draw(cx, &view, 200.);
    assert_eq!(state.logical_scroll_top().item_ix, 987);
    assert_eq!(state.logical_scroll_top().offset_in_item, px(20.));
}

#[gpui::test]
fn test_pending_scroll_accumulates_and_explicit_jump_replaces_it(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, view) = fixture(cx);
    state.scroll_by(px(-900.));
    state.scroll_by(px(-300.));
    draw(cx, &view, 100.);
    assert_eq!(state.logical_scroll_top().item_ix, 972);
    state.scroll_by(px(-9000.));
    state.scroll_to_end();
    draw(cx, &view, 100.);
    assert_eq!(state.logical_scroll_top().item_ix, 996);
}

#[gpui::test]
fn test_remeasure_then_scroll_does_not_revert_scroll_position(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();

    let state = ListState::new(20, gpui::ListAlignment::Top, px(10.));

    struct TestView(ListState);
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            list(self.0.clone(), |_, _, _| {
                div().h(px(100.)).w_full().into_any()
            })
            .w_full()
            .h_full()
        }
    }

    let view = {
        let state = state.clone();
        cx.update(|_, cx| cx.new(|_| TestView(state)))
    };

    state.scroll_to(gpui::ListOffset {
        item_ix: 5,
        offset_in_item: px(40.),
    });

    cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
        view.clone().into_any_element()
    });

    state.remeasure_items(5..6);

    cx.simulate_event(ScrollWheelEvent {
        position: point(px(50.), px(100.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-30.))),
        ..Default::default()
    });

    let offset = state.logical_scroll_top();
    assert_eq!(offset.item_ix, 5);
    assert_eq!(offset.offset_in_item, px(70.));

    cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
        view.into_any_element()
    });

    let offset = state.logical_scroll_top();
    assert_eq!(offset.item_ix, 5);
    assert_eq!(
        offset.offset_in_item,
        px(70.),
        "scrolling after a remeasure should not be reverted by the stale pending scroll"
    );
}

#[gpui::test]
fn test_scroll_after_remeasure_clamps_to_shrunk_item_height(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();

    let item_height = Rc::new(Cell::new(100usize));
    let state = ListState::new(20, gpui::ListAlignment::Top, px(10.));

    struct TestView {
        state: ListState,
        item_height: Rc<Cell<usize>>,
    }

    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let height = self.item_height.get();
            list(self.state.clone(), move |index, _, _| {
                let height = if index == 5 { height } else { 100 };
                div().h(px(height as f32)).w_full().into_any()
            })
            .w_full()
            .h_full()
        }
    }

    let view = {
        let state = state.clone();
        let item_height = item_height.clone();
        cx.update(|_, cx| cx.new(|_| TestView { state, item_height }))
    };

    state.scroll_to(gpui::ListOffset {
        item_ix: 5,
        offset_in_item: px(40.),
    });

    cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
        view.clone().into_any_element()
    });

    // Item 5 shrinks from 100px to 50px and is remeasured...
    item_height.set(50);
    state.remeasure_items(5..6);

    // ...and then the user scrolls down by 30px before the next frame,
    // landing at offset 70.
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(50.), px(100.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-30.))),
        ..Default::default()
    });

    cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
        view.into_any_element()
    });

    // The rebased pending scroll clamps the user's offset to the item's
    // new height instead of leaving it pointing past the end of the item.
    let offset = state.logical_scroll_top();
    assert_eq!(offset.item_ix, 5);
    assert_eq!(offset.offset_in_item, px(50.));
}

#[gpui::test]
fn test_bottom_alignment_keeps_tail_anchor_at_exact_end(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(10, gpui::ListAlignment::Bottom, px(400.));
    let view = cx.update(|_, cx| {
        cx.new(|_| TestView {
            state: state.clone(),
            scale: Rc::new(Cell::new(2.)),
            renders: Rc::new(Cell::new(0)),
        })
    });
    state.scroll_to(gpui::ListOffset {
        item_ix: 0,
        offset_in_item: px(0.),
    });
    draw(cx, &view, 100.);
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(10.), px(10.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-800.))),
        ..Default::default()
    });
    draw(cx, &view, 100.);
    state.splice(10..10, 2);
    draw(cx, &view, 100.);
    assert_eq!(
        state.logical_scroll_top().item_ix,
        12,
        "bottom-aligned exact end must keep following appended rows"
    );
}

#[gpui::test]
fn test_remeasure_during_pending_scroll_cannot_restore_stale_anchor(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, view) = fixture(cx);
    state.scroll_by(px(-900.));
    state.remeasure();
    draw(cx, &view, 100.);
    assert_eq!(state.logical_scroll_top().item_ix, 978);
}

#[gpui::test]
fn test_scroll_mask_handle_keeps_anchor_across_unknown_rows(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, view) = fixture(cx);
    for delta in [900., 300.] {
        let offset = state.scroll_px_offset_for_scrollbar();
        state.set_offset_from_scrollbar(point(px(0.), offset.y + px(delta)));
    }
    draw(cx, &view, 100.);
    assert_eq!(state.logical_scroll_top().item_ix, 972);
    for _ in 0..30 {
        let offset = state.scroll_px_offset_for_scrollbar();
        state.set_offset_from_scrollbar(point(px(0.), offset.y + px(80.)));
        draw(cx, &view, 100.);
    }
    assert_eq!(state.logical_scroll_top().item_ix, 924);
}

#[gpui::test]
fn test_scroll_mask_round_trip_resumes_tail_following(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, view) = fixture(cx);
    state.set_follow_mode(gpui::FollowMode::Tail);
    draw(cx, &view, 100.);
    let offset = state.scroll_px_offset_for_scrollbar();
    state.set_offset_from_scrollbar(point(px(0.), offset.y + px(900.)));
    draw(cx, &view, 100.);
    assert!(!state.is_following_tail());
    let offset = state.scroll_px_offset_for_scrollbar();
    state.set_offset_from_scrollbar(point(px(0.), offset.y - px(900.)));
    draw(cx, &view, 100.);
    assert!(state.is_following_tail());
    state.splice(1000..1000, 2);
    draw(cx, &view, 100.);
    assert!(state.is_following_tail());
    assert_eq!(state.logical_scroll_top().item_ix, 998);
}

#[gpui::test]
fn test_leading_overdraw_refreshes_asynchronously_grown_row(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(20, gpui::ListAlignment::Top, px(400.));
    let growing_height = Rc::new(Cell::new(20.));
    struct GrowthView {
        state: ListState,
        growing_height: Rc<Cell<f32>>,
    }
    impl Render for GrowthView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let growing_height = self.growing_height.get();
            list(self.state.clone(), move |ix, _, _| {
                div()
                    .h(px(if ix == 16 { growing_height } else { 100. }))
                    .w_full()
                    .into_any_element()
            })
            .size_full()
        }
    }
    let view = cx.update(|_, cx| {
        cx.new(|_| GrowthView {
            state: state.clone(),
            growing_height: growing_height.clone(),
        })
    });
    let draw = |cx: &mut VisualTestContext| {
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
    };
    state.scroll_to_end();
    draw(cx);
    assert_eq!(state.logical_scroll_top().item_ix, 18);
    // Mimic Markdown's asynchronous preparation: the view redraws with completed
    // content, but no explicit list remeasurement is sent by the content entity.
    growing_height.set(1600.);
    draw(cx);
    assert_eq!(state.logical_scroll_top().item_ix, 18);
    let offset = state.scroll_px_offset_for_scrollbar();
    state.set_offset_from_scrollbar(point(px(0.), offset.y + px(120.)));
    draw(cx);
    let anchor = state.logical_scroll_top();
    assert_eq!(anchor.item_ix, 16);
    assert_eq!(
        anchor.offset_in_item,
        px(1580.),
        "the old visible row must move by exactly the 120px scroll, not the asynchronously added height"
    );
    // Row 18 began at y=0; the visible remainder of row 16 plus row 17 is 120px.
    assert_eq!(px(1600.) - anchor.offset_in_item + px(100.), px(120.));
}
