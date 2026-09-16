# Installation script

[Documentation](README.md) · [中文](../zh/INSTALL_SCRIPT.md) · English

Most users can install with [Homebrew or a DMG](../../README.en.md#installation). `scripts/install.sh` is an optional interactive method for Apple Silicon macOS and requires a package that passes system security assessment.

## Download and run

```bash
curl -fsSL -o install.sh https://raw.githubusercontent.com/KrabsWong/yes-sessions/main/scripts/install.sh
less install.sh
bash install.sh
```

The script asks whether to remove an existing app and whether to launch after installation. Run the downloaded script directly in a terminal so a pipe does not interfere with interactive input.

To select a published version, replace `X.Y.Z` with a version from Releases, without the `v` prefix:

```bash
bash install.sh --version X.Y.Z
# Or use an environment variable
YS_VERSION=X.Y.Z bash install.sh
```

Version precedence is `--version` / `-v`, then `YS_VERSION`, then the latest GitHub Release. Use `bash install.sh --help` for help.

## What it does

1. Checks macOS and arm64 architecture and determines the version.
2. Checks `/Applications/Yes Sessions.app`. If you agree to remove it, removal happens before downloading the new package.
3. Downloads the DMG from the main repository’s Release and runs `hdiutil verify`.
4. Mounts the image and runs `codesign --verify --deep --strict` and `spctl --assess`.
5. Copies the app to `/Applications` after verification, unmounts the image, and cleans up temporary files.
6. Offers to launch the app.

The script does not remove quarantine attributes or bypass Gatekeeper. An ad-hoc signed package may pass signature integrity verification but fail `spctl`. In that case, the script stops without installing the new app.

## Updates and failures

Quit the app before updating. Replacement is not atomic: if you agree to remove the old app and the download or verification fails, the old app has already been removed. Declining removal does not stop the subsequent installation. Use Homebrew or manual DMG installation for controlled replacement.

If latest-version lookup fails, check GitHub connectivity or API limits, or specify a version. If download fails, confirm an arm64 asset exists for that Release. Without write access to `/Applications`, prefer manual installation in Finder and follow the system’s permission prompts.

For assessment failures, check the release’s signing notes and the [first-launch guidance](FAQ.md). Passing signature integrity verification does not mean a package is notarized.
