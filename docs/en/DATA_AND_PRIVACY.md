# Data and privacy

[Documentation](README.md) · [中文](../zh/DATA_AND_PRIVACY.md) · English

## Session sources

Yes Sessions reads local data under the current macOS user’s home directory. No file import or model API key is required.

| Source | Default location |
| --- | --- |
| CodeBuddy CLI | `~/.codebuddy/projects/<project>/*.jsonl` |
| CodeBuddy CN | `~/Library/Application Support/CodeBuddyExtension/Data/<account>/CodeBuddyIDE/<user>/history/<workspace>/<session>/` |
| Claude Code | `~/.claude/projects/<project>/*.jsonl` |
| OpenCode | `~/.local/share/opencode/opencode.db` |
| Codex CLI | `~/.codex/sessions/**/*.jsonl` and `~/.codex/session_index.jsonl` |

Session lists use lightweight reads. Opening details loads complete messages; cross-session searches may also read message bodies. Supported OpenCode v1/v2 data is parsed with migrated sessions deduplicated.

### CodeBuddy CN coverage

The list reads the IDE’s saved indexes. Details load indexed messages, including saved reasoning, tool calls and results, images, and request-level usage. Workspace paths are recovered from CodeBuddy CN’s `User/workspaceStorage` data. Empty sessions without displayable content are omitted.

IDE-supplied environment information and rules are collapsed by default; the original message remains available to copy. This changes presentation without rewriting source records. Messages never saved locally, deleted history, and missing attachments cannot be recovered.

## What the app stores

The app’s configuration directory is `~/Library/Application Support/yes-sessions/`, including:

- `settings.json`: language, theme, reading layout, terminal preferences, and other settings.
- `window-state.json`: window position, dimensions, and related state.

Opening an embedded attachment may create a file in the system temporary directory for an external app to open. Save as writes to the location you choose.

## Local access and external actions

Session browsing, searching, and project previews run locally. Previews do not modify source sessions or project files. The app has no cloud account service for syncing conversations.

Opening a remote attachment or link delegates to an external app and may access its network address. Resuming starts the original CLI; subsequent model requests, charges, and workspace changes follow that tool’s configuration and permissions. Installation and updates download releases from GitHub or Homebrew.

## Before sharing an issue

Sessions may contain source code, file paths, terminal output, and credentials. Remove API keys, personal information, and private code before sharing screenshots or examples. Prefer a minimal sanitized excerpt over an entire history directory.

Uninstalling the app does not clean up the source tools’ history. Homebrew’s `--zap` removes app settings and other paths listed by the Cask; read the [uninstall instructions](HOMEBREW_CASK_GUIDE.md) first.
