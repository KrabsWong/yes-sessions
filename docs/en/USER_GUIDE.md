# User guide

[Documentation](README.md) · [中文](../zh/USER_GUIDE.md) · English

## Browse sessions

Select a data source, then open a session from the list organized by project and date. Expand entries with subagent sessions to read their history. Content comes from the source tool, so message, usage, and tool-result coverage varies.

Conversations display Markdown, code blocks, tool calls and results, reasoning, and attachments. CodeBuddy CN questions are separated from IDE-supplied context. Expand Attached context to read environment information and rules, or copy the original message.

Use the navigation marks on the right to jump to questions and hover for a summary. Incoming messages preserve your position when reading older content; select the new-message indicator to jump to the latest content. Mermaid diagrams support zoom, pan, and reset. If rendering fails, view the source and retry.

## Search

- `⌘F`: search the current conversation.
- `⇧⌘F`: search sessions from the selected tool.
- Select a result to jump to its message; press `Escape` to close search.

Searching across sessions may read message bodies, so a large history can take longer to search than to list.

## Resume a session

Use the resume action on a supported CLI session to open your preferred terminal and invoke the original tool. Install the corresponding CLI and complete its login or configuration first. OpenCode 2 sessions use `opencode2`; other OpenCode sessions use `opencode`.

Supported terminals are Ghostty, Kitty, and Terminal.app. Automatic selection tries them in that order. An unavailable third-party preference falls back to an available option. If the project directory still exists, the command runs from that directory.

CodeBuddy CN history cannot be resumed through a CLI. Return to the IDE to continue working.

## Files and Git changes

Open the panel with the collapse/expand icon on the right of the title bar. Drag its boundary to resize it.

- **Files**: expand directories, search filenames, and preview text or images. Explicit file paths in tool calls can also open a preview.
- **Changes**: inspect the current branch and staged, unstaged, and untracked files. Text supports unified and side-by-side views; image revisions can be compared side by side.
- **Refresh**: when Git changes are detected, refresh to load them. The current preview is not replaced automatically.

Use the preview’s back button or `Escape` to return to the list. Press `Escape` in the list to close the panel.

Previews show current files and Git state, **not historical conversation snapshots**, and do not edit files. Files are limited to 4 MiB; text to 100,000 lines and 32 KiB per line. File search returns at most 500 matches and has traversal limits. Unsupported formats and exceeded limits show a notice.

## Settings and windows

Use `⌘,` to configure Chinese or English, light/dark/system appearance, reading layout, reasoning visibility, and preferred terminal. The documentation’s default language does not change the app’s language setting.

| Shortcut | Action |
| --- | --- |
| `⌘,` | Open settings |
| `⌘F` | Search this conversation |
| `⇧⌘F` | Search the selected tool’s sessions |
| `⌘W` | Hide the app and keep the reading state |
| `⌘M` | Minimize the window |
| `⌘Q` | Quit |

A normal exit saves window position, size, and maximized/full-screen state. Reopening a hidden app from the Dock returns to the current reading position.
