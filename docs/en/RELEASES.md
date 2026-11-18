# Releases

## 11.8.1

- Add static HTML session export. The main conversation and loadable subagent pages are saved together with in-page role filters, following the current language, reading layout, reasoning visibility, and tool-folding settings. Remote images are omitted, and sub-sessions that cannot be loaded are listed in the report.
- Move Rust CI and Release to the supported `macos-15` arm64 runner so jobs no longer fail before setup because of the retired `macos-14` configuration.
- Upgrade the artifact upload action to its Node.js 24 release, removing the Node.js 20 deprecation warning.
- Document session export in the bilingual README, user guide, and documentation index.

GitHub Releases continues to generate the detailed release notes automatically. This page records the main user-facing changes.
