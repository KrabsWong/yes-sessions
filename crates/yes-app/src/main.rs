#![recursion_limit = "512"]

use gpui_kit::component::{Root, Theme, ThemeMode};
use gpui_kit::*;
use yes_sessions::{app::YesSessions, app_assets::AppAssets};

fn main() {
    let application = gpui_kit::application().with_assets(AppAssets);
    application.on_reopen(|cx| {
        if !yes_sessions::commands::restore_existing_window(cx) {
            open_main_window(cx);
        }
        cx.activate(true);
    });
    application.run(|cx| {
        gpui_kit::init(cx);
        yes_sessions::commands::init(cx);
        yes_sessions::window_state::init(cx);
        open_main_window(cx);
    });
}

fn open_main_window(cx: &mut App) {
    let bounds = yes_sessions::window_state::bounds(cx);
    let restore_size = bounds.get_bounds().size;
    cx.open_window(
        WindowOptions {
            window_bounds: Some(bounds),
            window_min_size: Some(size(
                px(900.0).min(restore_size.width),
                px(600.0).min(restore_size.height),
            )),
            titlebar: Some(TitlebarOptions {
                title: Some("Yes Sessions".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.0), px(18.0))),
            }),
            ..Default::default()
        },
        |window, cx| {
            Theme::change(ThemeMode::from(window.appearance()), Some(window), cx);
            let app = cx.new(|cx| YesSessions::new(window, cx));
            cx.new(|cx| {
                yes_sessions::window_state::track(window, cx);
                Root::new(app, window, cx)
            })
        },
    )
    .expect("failed to open Yes Sessions window");
}
