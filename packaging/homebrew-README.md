# Yes Sessions Homebrew Tap

[Yes Sessions](https://github.com/KrabsWong/yes-sessions) is a native Rust/GPUI app for browsing and resuming local AI CLI sessions.

## Install

```bash
brew tap krabswong/yes-sessions
brew trust krabswong/yes-sessions
brew install --cask yes-sessions
```

If Homebrew says the tap is untrusted, run `brew trust krabswong/yes-sessions` and retry the installation.

## Upgrade

```bash
brew update
brew upgrade --cask yes-sessions
```

## Requirements

- macOS 13 Ventura or later
- Apple Silicon

Releases may use ad-hoc signing without Apple notarization; check the source release notes for signing status. If macOS says it cannot verify the app is free of malware and offers to move it to the Trash on first launch, close the alert and open System Settings → Privacy & Security. In the Security section, find the app-blocked message, choose **Open Anyway**, and confirm when prompted. First verify the app came from the [official Releases page](https://github.com/KrabsWong/yes-sessions/releases). Managed devices may restrict this option.

## Uninstall

```bash
brew uninstall --cask yes-sessions
brew untap krabswong/yes-sessions
```

[Latest release](https://github.com/KrabsWong/homebrew-yes-sessions/releases/latest)
