# Install with Homebrew

[Home](../../README.en.md) · [简体中文](../zh/HOMEBREW_CASK_GUIDE.md) · [Release process](RELEASE_PIPELINE.md)

Yes Sessions is distributed through the project's [Homebrew Tap](https://github.com/KrabsWong/homebrew-yes-sessions). It requires **macOS 13 Ventura or later and Apple Silicon**. Install Homebrew before using the commands below, or download the [DMG installer](https://github.com/KrabsWong/yes-sessions/releases/latest) directly.

## Install

```bash
brew tap krabswong/yes-sessions
brew install --cask yes-sessions
```

After installation, open **Yes Sessions** from Applications.

If macOS blocks the first launch, check that version's Release notes: some installers use ad-hoc signing without Apple notarization. After verifying that you trust the source, choose **Open Anyway** in **System Settings → Privacy & Security**. Organization-managed devices may restrict this option.

## Update

Quit Yes Sessions, then run:

```bash
brew update
brew upgrade --cask yes-sessions
```

Reopen the app when the update finishes. To inspect the current Cask and installed version:

```bash
brew info --cask yes-sessions
brew list --cask --versions yes-sessions
```

## Uninstall

```bash
brew uninstall --cask yes-sessions
brew untap krabswong/yes-sessions
```

A regular uninstall keeps the app's settings. To also remove Yes Sessions settings and saved window state, use this command instead before removing the Tap:

```bash
brew uninstall --cask --zap yes-sessions
```

The current Cask's `zap` list contains:

- `~/Library/Application Support/yes-sessions`
- `~/Library/Preferences/com.yessessions.app.plist`
- `~/Library/Saved Application State/com.yessessions.app.savedState`

These operations do not remove session directories owned by Claude Code, Codex, or other source tools.

## Troubleshooting installation

- **Cask not found:** Confirm you have run `brew tap krabswong/yes-sessions`, then run `brew update`.
- **Download failed:** Check whether you can access the [Tap's Release page](https://github.com/KrabsWong/homebrew-yes-sessions/releases/latest). You can also download the DMG from the main repository and install it manually.
- **Checksum mismatch:** Do not bypass verification. Record the version, command, and error, then [report the issue](https://github.com/KrabsWong/yes-sessions/issues).
- **Unsupported architecture:** The installer currently supports Apple Silicon only. An Intel version is not available.

## Maintainer reference

The installation source is the project's own Tap and does not depend on submission to the official `homebrew-cask` repository. The [release workflow](../../.github/workflows/release.yml) updates the Cask automatically using the final published DMG's SHA256. Users do not need to edit versions or checksums.

Maintain the Cask through the main repository's [template](../../packaging/yes-sessions.rb.in), and maintain the Tap README through its [template](../../packaging/homebrew-README.md). See the [release process](RELEASE_PIPELINE.md) for credential setup, publication order, and recovery steps.
