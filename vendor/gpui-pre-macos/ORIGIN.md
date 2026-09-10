# Origin and local patch

Source: crates.io `gpui-pre-macos` 0.3.3, corresponding to Zed revision
`5b055fa789a8b8d38ac951a6e0cde272f66b4495`. The original Apache-2.0 license is
preserved in `LICENSE-APACHE`.

The local change is confined to `PlatformWindow::draw` in `src/window.rs`.
When the GPUI host has visible native child views, Metal presentation uses the
same Core Animation transaction as AppKit. This keeps embedded WKWebView frames
aligned with their GPUI containers during scrolling. The previous presentation
mode is restored after drawing, preserving resize/display-layer transactions
and the normal asynchronous path in windows without visible native children.

Merely setting `CAMetalLayer.presentsWithTransaction` from the application is
insufficient: GPUI's renderer also needs to select its transaction-aware command
submission path (commit, wait until scheduled, then present the drawable).

This is the existing dependency and version, with no new runtime or license.
