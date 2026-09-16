# Yes Sessions

**Browse, search, and resume your AI coding sessions in one macOS app.**

[中文](README.md) · English

Yes Sessions brings local history from CodeBuddy CLI, CodeBuddy CN, Claude Code, OpenCode, and Codex CLI into a native interface. Find an earlier question, inspect tool activity, or return to the terminal to continue working without digging through log files.

|Dark|Light
|:--|:--
<img width="500" alt="image" src="https://github.com/user-attachments/assets/1cffc250-0b2f-4614-a86f-f8d8a1edd13c" /> | <img width="500" alt="image" src="https://github.com/user-attachments/assets/fdb31790-ebe5-4d10-90a1-22e413660d7e" />

[Download](https://github.com/KrabsWong/yes-sessions/releases/latest) · [User guide](docs/en/USER_GUIDE.md) · [FAQ](docs/en/FAQ.md) · [Report an issue](https://github.com/KrabsWong/yes-sessions/issues)

## What you can do

- **Recover context**: browse sessions by project and date, search a conversation or the selected tool’s history, and jump to matching messages.
- **Follow the work**: read Markdown, code, reasoning, tool calls, attachments, and subagent sessions. Expand attached context to inspect environment information and rules added by an IDE.
- **Inspect code and changes**: browse project files, preview images, and compare staged, unstaged, and untracked Git changes in a side panel.
- **Continue working**: resume supported CLI sessions in Ghostty, Kitty, or Terminal.app.
- **Read long conversations comfortably**: navigate between questions, keep your reading position as messages arrive, and choose your language, theme, and reading layout.

## Supported tools

| Tool | Browse and search local history | Resume in a terminal |
| --- | --- | --- |
| CodeBuddy CLI | Yes | Yes |
| CodeBuddy CN (IDE) | Yes | No |
| Claude Code | Yes | Yes |
| OpenCode | Yes, including OpenCode 1 and OpenCode 2 beta data | Yes, with the corresponding CLI installed |
| Codex CLI | Yes | Yes |

Available content depends on what the source tool saved. Yes Sessions cannot reconstruct missing messages and does not provide model services. Resuming a session requires the corresponding CLI to be installed and configured.

## Installation

Requires an **Apple Silicon Mac running macOS 13 Ventura or later**. Intel, Windows, and Linux packages are not currently available.

### Homebrew

```bash
brew tap krabswong/yes-sessions
brew install --cask yes-sessions
```

To update:

```bash
brew update
brew upgrade --cask yes-sessions
```

### Download manually

Download `Yes-Sessions-<version>-arm64.dmg` from [GitHub Releases](https://github.com/KrabsWong/yes-sessions/releases/latest), open it, and drag `Yes Sessions.app` into Applications.

Check the release notes for signing and notarization status. If macOS blocks an unnotarized release, verify the download source and use System Settings → Privacy & Security → Open Anyway. Managed devices may restrict this option.

See the [installation script reference](docs/en/INSTALL_SCRIPT.md) for an alternative method.

## Getting started

1. Create local session history in one of the supported tools, then open Yes Sessions.
2. Select a tool, locate a session by project and date, and open its details.
3. Use `⌘F` to search the current conversation or `⇧⌘F` to search the selected tool’s history.
4. Use the resume action to continue a supported session, or open the file and changes panel from the right side of the title bar to inspect project code.

Open settings with `⌘,` to change the interface language, theme, reading layout, and preferred terminal. See the [user guide](docs/en/USER_GUIDE.md).

## Data and privacy

Browsing and searching read local history files or databases. You do not need to give Yes Sessions a model API key. Settings and window state are stored locally. See [data and privacy](docs/en/DATA_AND_PRIVACY.md) for storage locations, CodeBuddy CN coverage, and external actions.

The file and Git panels are read-only previews of the **current workspace**, not snapshots from the time of a conversation. After resuming, network activity, file changes, and charges are governed by the corresponding CLI.

## Documentation and contributions

- [Documentation index](docs/en/README.md): installation, usage, troubleshooting, and maintenance.
- [Development and contributions](docs/en/CONTRIBUTING.md): local builds, validation, issue reports, and code contributions.
- [Release process](docs/en/RELEASE_PIPELINE.md): versioning, signing, and distribution for maintainers.

Built with Rust and GPUI Kit for a native macOS interface. Only Mermaid diagrams use the system WebKit renderer; no browser engine is bundled.

## License

The project declares the MIT license in [Cargo.toml](Cargo.toml). Third-party components retain their own licenses; see the [third-party notices](crates/yes-app/third-party/).
