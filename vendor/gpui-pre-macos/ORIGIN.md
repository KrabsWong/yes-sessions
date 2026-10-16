# Origin and local patches

Source: crates.io `gpui-pre-macos` 0.3.9, published as part of GPUI Kit.
The original Apache-2.0 license is preserved in `LICENSE-APACHE`.

Local patches retained from the previous 0.3.3 integration:

- In `PlatformWindow::draw`, synchronize Metal presentation with AppKit's
  Core Animation transaction while visible native child views (WKWebView) are
  present. Restore the previous presentation mode after drawing, preserving
  resize/display-layer transactions and asynchronous rendering elsewhere.
  The renderer setter is required to select its transaction-aware command
  submission path; setting the CAMetalLayer property alone is insufficient.
- Replace deprecated Cocoa Foundation/AppKit calls with typed objc2 APIs,
  retaining raw pointers and geometry encoding adapters only where required
  by the legacy Objective-C callback boundary. Keep string, pasteboard,
  and geometry regression tests and the corresponding objc2 feature flags.

The migration preserves 0.3.9's display refresh handling, activation policies,
window restoration, tracking areas, drag image providers, and background
material changes. Its modern objc2 screen capture and file-panel implementations
are used directly instead of the older local replacements. Modules moved
upstream to `gpui-pre-apple` are no longer included in this macOS crate.

No new runtime or license is introduced by these local patches.
