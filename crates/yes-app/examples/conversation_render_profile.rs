//! Native layout/scroll probe using the installed settings and real provider data.
//! Run with a CodeBuddy session ID. It prints timings, never transcript contents.
use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{component::Root, *};
use yes_sessions::{app::YesSessions, app_assets::AppAssets};

fn settle(cx: &mut VisualTestAppContext, window: AnyWindowHandle) -> anyhow::Result<()> {
    cx.run_until_parked();
    cx.update_window(window, |_, window, _| window.refresh())?;
    cx.advance_clock(Duration::from_millis(34));
    cx.run_until_parked();
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let id = std::env::args()
        .nth(1)
        .expect("pass a CodeBuddy session ID");
    let platform = gpui_kit::platform::current_platform(false);
    let mut cx = VisualTestAppContext::with_asset_source(platform, Arc::new(AppAssets));
    cx.update(gpui_kit::init);
    let holder = Rc::new(RefCell::new(None));
    let capture = holder.clone();
    let started = Instant::now();
    let window = cx.open_offscreen_window(size(px(1200.), px(800.)), move |window, cx| {
        let app = cx.new(|cx| YesSessions::new(window, cx));
        *capture.borrow_mut() = Some(app.clone());
        cx.new(|cx| Root::new(app, window, cx))
    })?;
    let window = AnyWindowHandle::from(window);
    settle(&mut cx, window)?;
    println!("startup including default session: {:?}", started.elapsed());
    let app = holder.borrow().as_ref().unwrap().clone();
    let started = Instant::now();
    cx.update(|cx| app.update(cx, |app, cx| app.open_sub_agent(&id, cx)));
    settle(&mut cx, window)?;
    println!(
        "navigate to requested session (no-op if already selected): {:?}",
        started.elapsed()
    );
    let mut frames = Vec::new();
    for _ in 0..80 {
        let started = Instant::now();
        cx.simulate_event(
            window,
            ScrollWheelEvent {
                position: point(px(850.), px(420.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-400.))),
                touch_phase: TouchPhase::Moved,
                ..Default::default()
            },
        );
        cx.run_until_parked();
        frames.push(started.elapsed());
    }
    frames.sort();
    println!(
        "scroll p50={:?}, p95={:?}, max={:?}",
        frames[40], frames[76], frames[79]
    );
    if let Some(path) = std::env::var_os("YES_PROFILE_SCREENSHOT") {
        cx.capture_screenshot(window)?.save(path)?;
    }
    Ok(())
}
