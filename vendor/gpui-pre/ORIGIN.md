# Origin and local patch

Source: crates.io `gpui-pre` 0.3.3, corresponding to Zed revision
`5b055fa789a8b8d38ac951a6e0cde272f66b4495`. The original Apache-2.0 license is
preserved in `LICENSE-APACHE`. Examples and their large assets are omitted;
example targets are removed from the normalized package manifest.

The local change is confined to `src/elements/list.rs`. It preserves logical
row anchors while scrolling across unmeasured variable-height rows. Pixel
movement is resolved during layout using only the rows on its path, rather
than seeking through unknown rows whose height is represented as zero.

Both direct wheel events and `ScrollableMask`'s scrollbar-handle wheel route
use this behavior. Actual scrollbar thumb drags retain their absolute-position
behavior. Previously measured rows retain their existing remeasurement anchor
semantics, and bottom-aligned lists continue following appended content. The
bounded leading overdraw refreshes actual row heights on redraw, so asynchronous
Markdown completion cannot leave stale short heights in the upward scroll path.

`tests/list_scroll.rs` covers lazy measurement, both scroll directions, coalesced
wheel events, repeated handle offsets, resizing, remeasurement, explicit jumps,
bottom alignment, and asynchronous row growth inside leading overdraw. Run from the repository root:

```sh
cargo test -p gpui-pre --test list_scroll
```

Upstream library unit tests reference font fixtures outside the published crate;
this regression suite uses public-API integration tests and needs no extra assets.
