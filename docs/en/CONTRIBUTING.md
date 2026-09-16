# Development and contributions

[Documentation](README.md) · [中文](../zh/CONTRIBUTING.md) · English

Issues, documentation fixes, and code contributions are welcome. For substantial features or architectural changes, open an issue to discuss the use case and scope first.

## Run locally

You need macOS 13 or later, Apple Silicon, Rust stable, and Xcode Command Line Tools. The first build downloads dependencies.

```bash
git clone https://github.com/KrabsWong/yes-sessions.git
cd yes-sessions
cargo run -p yes-sessions
```

## Project structure

| Path | Responsibility |
| --- | --- |
| `crates/yes-core/` | Session models, providers, search, settings, terminal and Git services |
| `crates/yes-app/` | GPUI interface, conversation reader, file previews, and Mermaid host |
| `packaging/` | macOS metadata and Homebrew templates |
| `scripts/` | Build, packaging, and installation scripts |
| `docs/`, `docs/en/` | Chinese and English documentation |

The app uses Rust and GPUI Kit. Only Mermaid uses the system WKWebView; do not introduce general web pages or a separate browser runtime. Keep provider access in `yes-core` and listing large histories lightweight. UI strings live in `crates/yes-app/src/i18n.rs` and require both languages. Preserve macOS 13.0 compatibility and consider license and binary size when adding dependencies.

The repository’s [development rules](../../AGENTS.md) and [visual guidelines](../../DESIGN.md) describe additional constraints in Chinese.

## Validation

Before submitting code changes, run:

```bash
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
./scripts/package-macos.sh
codesign --verify --deep --strict "target/macos/Yes Sessions.app"
```

Outputs are `target/macos/Yes Sessions.app` and `release/Yes-Sessions-<version>-arm64.dmg`. Local builds can use ad-hoc signing without distribution credentials; this is not Apple notarization. For documentation-only changes, check accuracy, relative links, translation coverage, and commands against the implementation.

The icon source is `assets/logo.png`. ImageMagick and `./scripts/generate-icons.sh` are needed only when regenerating icons; routine packaging uses the generated assets in the repository.

## Submit a change

1. Create a `feature/` branch from the latest `main`; external contributors can fork the repository first.
2. Keep the change focused, add tests when needed, and update both documentation languages.
3. Review the diff for session history, credentials, personal paths, and build artifacts. Scan the entire outgoing commit history for secrets before pushing, not just the final files.
4. Include a concise commit subject and a body describing the reason and validation.
5. Open a PR targeting `main`, explaining the problem, behavior change, and validation. Maintainers confirm the release label and merge.

Choose exactly one of `major`, `minor`, `patch`, or `skip-release`; documentation-only changes normally use `skip-release`. See the [release process](RELEASE_PIPELINE.md).

## Report an issue

Include the app, macOS, and source tool versions, reproduction steps, and expected behavior. Use sanitized examples for parser problems; do not upload entire history directories, access tokens, or private source code.
