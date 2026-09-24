# Vendored block 0.1.6

Upstream: https://github.com/SSheldon/rust-block

The package manifest, README, and `src/` are from the published `block` 0.1.6
crate, whose registry checksum is
`0d8c1fef690941d3e7788d328517591fecc684c084084702d6ff1641e993699a`.
The `test_utils/` directory omitted from that crate is restored from upstream
revision `642ea4a4a5853a21b55b05c34832a5f1bb1af61c`.

## License

Both upstream manifests declare `license = "MIT"` and author Steven Sheldon.
Those declarations are preserved. Neither the published crate nor the upstream
root supplies a separate license text file; this copy does not invent an
upstream copyright notice. The declared license is the
[MIT license](https://spdx.org/licenses/MIT.html).

## Local changes

- Replace the uninhabited opaque `Class` enum with an inhabited `repr(C)`
  opaque struct and obtain the runtime symbol address with `ptr::addr_of!`.
  The public API, block layout, and libSystem linkage are unchanged; no newer
  macOS API is introduced and no warning is suppressed.
- Retain all upstream tests and add a captured-value copy/clone/drop regression.
- Give this vendored crate its own workspace for isolated tests.
- Use `cc` 1 in place of the obsolete `gcc` 0.3 test-helper build dependency;
  retain the upstream C helper code unchanged.
- Explicitly spell the existing C ABI on foreign declarations and function
  pointers in the library and Rust helper, and declare edition 2015 in both
  manifests. These preserve upstream semantics while passing `-D warnings`.

Run isolated tests without rebuilding the application:

```sh
MACOSX_DEPLOYMENT_TARGET=13.0 RUSTFLAGS='-D warnings' RUSTDOCFLAGS='-D warnings' \
  cargo test --manifest-path vendor/block/Cargo.toml --workspace --offline \
  --target-dir /tmp/yes-sessions-block-tests
```
