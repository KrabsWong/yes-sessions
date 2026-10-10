# Frequently asked questions

[Documentation](README.md) · [中文](../zh/FAQ.md) · English

## Why are no sessions listed?

Select the correct tool and confirm it has saved sessions under the current macOS user. Check the [source locations](DATA_AND_PRIVACY.md). CodeBuddy CLI and CodeBuddy CN are separate sources; empty CodeBuddy CN sessions are omitted.

Storage format changes in the original tool may require an update. When reporting a problem, include the tool version, Yes Sessions version, and a sanitized description of the data structure.

## Why is content missing or usage zero?

Only information saved by the source tool can be displayed. Some sessions lack reasoning, token usage, or attachment bodies; missing statistics do not imply no actual usage. The usage dashboard only totals recognized data, so missing fields can make totals incomplete. Moved, deleted, or remote-only attachments may not be available for preview.

## Why does cross-session search return no or incomplete results?

Confirm the selected tool and review the keyword, time range, and working-directory filters. Message-role filters search only messages of that role; set the message type to All to search session information. Unreadable sessions are reported, and results are capped at 1,000. Narrow the time or directory scope and try again.

## Why does resuming fail?

Check that the corresponding CLI runs in your terminal and is signed in. Verify that the project directory still exists and your preferred terminal is available. OpenCode 2 sessions require `opencode2`. CodeBuddy CN supports history viewing only; continue in the IDE.

When using Terminal.app, macOS may ask for automation permission. If previously denied, check the system Privacy & Security settings.

## What if macOS blocks the app?

On first launch, if macOS says it cannot verify the app is free of malware and offers to move it to the Trash, close the alert and open **System Settings → Privacy & Security**. In the Security section, find the app-blocked message, choose **Open Anyway**, and confirm when prompted. First verify the package came from the project’s [Releases](https://github.com/KrabsWong/yes-sessions/releases) and read that version’s signing notes. Managed devices may prohibit this.

If the installation script fails at `spctl`, the package did not pass system assessment. The script does not bypass this check; see the [script reference](INSTALL_SCRIPT.md).

## What if Homebrew says the Tap is untrusted?

Run `brew trust krabswong/yes-sessions`, then retry `brew install --cask yes-sessions`. See the [Homebrew installation guide](HOMEBREW_CASK_GUIDE.md) for the full steps.

## Why does the file preview differ from code in a conversation?

The file and Git panels show the current workspace rather than historical snapshots. Later edits are reflected in previews. Refresh to reread the files; large files, long text, and unsupported formats have display limits.

## Why is the app still running after closing the window?

`⌘W` hides the app while retaining the session and reading state. Use `⌘Q` to quit completely. Reopen the hidden app from the Dock to continue reading.

## How do I update or uninstall?

Homebrew users can follow the [installation, update, and removal guide](HOMEBREW_CASK_GUIDE.md). For manual installations, quit the app and replace it in Applications with a newer download. To uninstall, move it to Trash. This does not delete history saved by the original tools.

## How do I report a problem?

Open a [GitHub issue](https://github.com/KrabsWong/yes-sessions/issues) with the app and macOS versions, source tool and version, steps to reproduce, and expected versus actual behavior. Sanitize screenshots and samples; you do not need to publish your complete conversation history.
