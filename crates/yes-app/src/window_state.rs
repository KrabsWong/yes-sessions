//! Main-window geometry is kept separately from user preferences.
use std::{fs, path::Path};

use gpui_kit::*;
use serde_json::{Value, json};

struct WindowState(WindowBounds);
impl Global for WindowState {}

pub fn init(cx: &mut App) {
    let path = yes_core::SettingsStore::default_path().with_file_name("window-state.json");
    let saved = fs::read_to_string(&path)
        .ok()
        .and_then(|text| decode(&text));
    let displays = display_bounds(cx);
    let bounds = fit_to_displays(saved.unwrap_or(default_bounds()), &displays);
    cx.set_global(WindowState(bounds));
    cx.on_app_quit(move |cx| {
        // Capture the final state as well as resize notifications, including zoom/fullscreen.
        if let Some(handle) = cx.windows().first().copied() {
            let _ = handle.update(cx, |_, window, cx| capture(window, cx));
        }
        if let Err(error) = save(&path, cx.global::<WindowState>().0) {
            eprintln!("Unable to save window state: {error}");
        }
        async {}
    })
    .detach();
}

pub fn bounds(cx: &App) -> WindowBounds {
    let displays = display_bounds(cx);
    fit_to_displays(cx.global::<WindowState>().0, &displays)
}

fn display_bounds(cx: &App) -> Vec<Bounds<Pixels>> {
    let mut displays = cx.displays();
    if let Some(primary) = cx.primary_display() {
        displays.sort_by_key(|display| display.id() != primary.id());
    }
    displays
        .iter()
        .map(|display| display.visible_bounds())
        .collect()
}

pub fn track<T: 'static>(window: &mut Window, cx: &mut Context<T>) {
    cx.observe_window_bounds(window, |_, window, cx| capture(window, cx))
        .detach();
}

fn capture(window: &Window, cx: &mut App) {
    let previous = cx.global::<WindowState>().0;
    let current = window.window_bounds();
    cx.global_mut::<WindowState>().0 = match current {
        WindowBounds::Windowed(_) if window.is_maximized() => {
            WindowBounds::Maximized(previous.get_bounds())
        }
        _ => current,
    };
}

fn default_bounds() -> WindowBounds {
    WindowBounds::Windowed(Bounds::new(
        point(px(120.), px(80.)),
        size(px(1200.), px(800.)),
    ))
}

fn fit_to_displays(saved: WindowBounds, displays: &[Bounds<Pixels>]) -> WindowBounds {
    let bounds = saved.get_bounds();
    let Some(display) = displays
        .iter()
        .max_by(|a, b| overlap(bounds, **a).total_cmp(&overlap(bounds, **b)))
    else {
        return saved;
    };
    // A disconnected display sends the window back to the primary display.
    let display = if overlap(bounds, *display) > 0. {
        *display
    } else {
        displays[0]
    };
    let width = bounds.size.width.max(px(900.)).min(display.size.width);
    let height = bounds.size.height.max(px(600.)).min(display.size.height);
    let fitted = Bounds::new(
        point(
            bounds
                .origin
                .x
                .max(display.left())
                .min(display.right() - width),
            bounds
                .origin
                .y
                .max(display.top())
                .min(display.bottom() - height),
        ),
        size(width, height),
    );
    match saved {
        WindowBounds::Windowed(_) => WindowBounds::Windowed(fitted),
        WindowBounds::Maximized(_) => WindowBounds::Maximized(fitted),
        WindowBounds::Fullscreen(_) => WindowBounds::Fullscreen(fitted),
    }
}

fn overlap(a: Bounds<Pixels>, b: Bounds<Pixels>) -> f32 {
    let width = f32::from((a.right().min(b.right()) - a.left().max(b.left())).max(px(0.)));
    let height = f32::from((a.bottom().min(b.bottom()) - a.top().max(b.top())).max(px(0.)));
    width * height
}

fn encode(state: WindowBounds) -> String {
    let bounds = state.get_bounds();
    json!({
        "mode": match state {
            WindowBounds::Windowed(_) => "windowed",
            WindowBounds::Maximized(_) => "maximized",
            WindowBounds::Fullscreen(_) => "fullscreen",
        },
        "x": f32::from(bounds.origin.x), "y": f32::from(bounds.origin.y),
        "width": f32::from(bounds.size.width), "height": f32::from(bounds.size.height),
    })
    .to_string()
}

fn decode(text: &str) -> Option<WindowBounds> {
    let value: Value = serde_json::from_str(text).ok()?;
    let number = |key: &str| {
        let number = value.get(key)?.as_f64()? as f32;
        number.is_finite().then_some(number)
    };
    let (x, y, width, height) = (
        number("x")?,
        number("y")?,
        number("width")?,
        number("height")?,
    );
    if width <= 0. || height <= 0. {
        return None;
    }
    let bounds = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
    match value.get("mode")?.as_str()? {
        "windowed" => Some(WindowBounds::Windowed(bounds)),
        "maximized" => Some(WindowBounds::Maximized(bounds)),
        "fullscreen" => Some(WindowBounds::Fullscreen(bounds)),
        _ => None,
    }
}

fn save(path: &Path, state: WindowBounds) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, encode(state))?;
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, fit_to_displays, save};
    use gpui_kit::{Bounds, WindowBounds, point, px, size};

    #[test]
    fn geometry_modes_round_trip_without_overwriting_settings() {
        let directory =
            std::env::temp_dir().join(format!("yes-window-state-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let settings = directory.join("settings.json");
        std::fs::write(&settings, "unchanged").unwrap();
        let rect = Bounds::new(point(px(-1100.), px(100.)), size(px(1000.), px(700.)));
        for state in [
            WindowBounds::Windowed(rect),
            WindowBounds::Maximized(rect),
            WindowBounds::Fullscreen(rect),
        ] {
            assert_eq!(decode(&encode(state)), Some(state));
            let path = directory.join("window-state.json");
            save(&path, state).unwrap();
            assert_eq!(decode(&std::fs::read_to_string(path).unwrap()), Some(state));
        }
        assert_eq!(std::fs::read_to_string(settings).unwrap(), "unchanged");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn disconnected_or_smaller_displays_keep_the_whole_window_visible() {
        let main = Bounds::new(point(px(0.), px(25.)), size(px(1440.), px(850.)));
        let secondary = Bounds::new(point(px(-1920.), px(25.)), size(px(1920.), px(1055.)));
        let saved = WindowBounds::Maximized(Bounds::new(
            point(px(-1800.), px(80.)),
            size(px(1600.), px(950.)),
        ));
        assert_eq!(fit_to_displays(saved, &[main, secondary]), saved);
        let restored = fit_to_displays(saved, &[main]);
        assert!(matches!(restored, WindowBounds::Maximized(_)));
        assert_eq!(restored.get_bounds(), main);
        let tiny = Bounds::new(point(px(0.), px(25.)), size(px(800.), px(550.)));
        assert_eq!(fit_to_displays(saved, &[tiny]).get_bounds(), tiny);
    }

    #[test]
    fn malformed_geometry_is_ignored() {
        for text in [
            "not json",
            "{}",
            r#"{"mode":"windowed","x":0,"y":0,"width":-1,"height":800}"#,
            r#"{"mode":"windowed","x":1e100,"y":0,"width":1000,"height":800}"#,
        ] {
            assert!(decode(text).is_none());
        }
    }
}
