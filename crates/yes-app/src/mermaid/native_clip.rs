//! Mirror GPUI's scroll mask into AppKit, which paints child views separately.

use gpui_kit::{Bounds, Pixels, px};
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use wry::WebViewExtMacOS;

pub(super) struct NativeClip {
    view: Retained<NSView>,
}

impl NativeClip {
    pub(super) fn new(webview: &wry::WebView) -> Self {
        let native = webview.webview();
        // Both views are retained and accessed only on GPUI's main thread.
        let parent = unsafe { native.superview() }.expect("Mermaid WebView has a parent");
        let mtm = MainThreadMarker::new().expect("GPUI renders on the main thread");
        let view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
        view.setClipsToBounds(true);
        view.setHidden(true);
        parent.addSubview(&view);
        view.addSubview(&native);
        native.setAutoresizingMask(NSAutoresizingMaskOptions::ViewNotSizable);
        Self { view }
    }

    pub(super) fn hide(&self) {
        self.view.setHidden(true);
    }

    pub(super) fn update(
        &self,
        webview: &wry::WebView,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
    ) {
        let visible = bounds.intersect(&mask);
        if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
            self.hide();
            return;
        }
        // The window owns the parent; retaining it keeps it alive during layout.
        let Some(parent) = (unsafe { self.view.superview() }) else {
            return;
        };
        self.view.setFrame(native_frame(
            visible,
            parent.bounds().size.height,
            parent.isFlipped(),
        ));

        // Keep the full diagram viewport and translate it behind the clip. Resizing
        // the WebView to the intersection would reflow/recenter the diagram.
        let local = Bounds {
            origin: bounds.origin - visible.origin,
            size: bounds.size,
        };
        webview.webview().setFrame(native_frame(
            local,
            f64::from(visible.size.height),
            self.view.isFlipped(),
        ));
        let _ = webview.set_visible(true);
        self.view.setHidden(false);
    }
}

impl Drop for NativeClip {
    fn drop(&mut self) {
        self.view.removeFromSuperview();
    }
}

fn native_frame(bounds: Bounds<Pixels>, parent_height: f64, flipped: bool) -> NSRect {
    let x = f64::from(bounds.origin.x);
    let y = f64::from(bounds.origin.y);
    let width = f64::from(bounds.size.width);
    let height = f64::from(bounds.size.height);
    NSRect::new(
        NSPoint::new(
            x,
            if flipped {
                y
            } else {
                parent_height - y - height
            },
        ),
        NSSize::new(width, height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, size};

    #[test]
    fn scrolling_above_header_clips_without_resizing_diagram() {
        let bounds = Bounds::new(point(px(20.), px(50.)), size(px(600.), px(500.)));
        let mask = Bounds::new(point(px(0.), px(200.)), size(px(800.), px(600.)));
        let visible = bounds.intersect(&mask);
        assert_eq!(visible.origin.y, px(200.));
        assert_eq!(visible.size.height, px(350.));
        let local = Bounds::new(bounds.origin - visible.origin, bounds.size);
        let frame = native_frame(local, 350., false);
        assert_eq!(frame.origin.y, 0.);
        assert_eq!(frame.size.height, 500.);
        assert_eq!(native_frame(local, 350., true).origin.y, -150.);
    }

    #[test]
    fn clipping_bottom_and_sides_preserves_content_offset() {
        let bounds = Bounds::new(point(px(-20.), px(300.)), size(px(600.), px(500.)));
        let mask = Bounds::new(point(px(0.), px(200.)), size(px(400.), px(400.)));
        let visible = bounds.intersect(&mask);
        let local = Bounds::new(bounds.origin - visible.origin, bounds.size);
        let frame = native_frame(local, f64::from(visible.size.height), false);
        assert_eq!(visible.size, size(px(400.), px(300.)));
        assert_eq!(frame.origin, NSPoint::new(-20., -200.));
        assert_eq!(frame.size, NSSize::new(600., 500.));
    }

    #[test]
    fn fully_scrolled_out_diagram_has_no_visible_area() {
        let bounds = Bounds::new(point(px(0.), px(-500.)), size(px(600.), px(500.)));
        let mask = Bounds::new(point(px(0.), px(200.)), size(px(800.), px(600.)));
        assert!(bounds.intersect(&mask).size.height <= px(0.));
    }
}
