#![recursion_limit = "512"]

use gpui_kit::component::{Root, Theme, ThemeMode};
use gpui_kit::*;
use yes_sessions::{app::YesSessions, app_assets::AppAssets};

fn main() {
    gpui_kit::application().with_assets(AppAssets).run(|cx| {
        gpui_kit::init(cx);
        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(120.0), px(80.0)),
                        size(px(1200.0), px(800.0)),
                    ))),
                    window_min_size: Some(size(px(900.0), px(600.0))),
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
                    cx.new(|cx| Root::new(app, window, cx))
                },
            )
            .expect("failed to open Yes Sessions window");
        })
        .detach();
    });
}
