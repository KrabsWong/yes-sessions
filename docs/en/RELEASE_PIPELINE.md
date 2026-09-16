# Release process

[Home](../../README.en.md) · [简体中文](../zh/RELEASE_PIPELINE.md) · [Homebrew installation](HOMEBREW_CASK_GUIDE.md)

This guide is for maintainers and covers the path from merging a pull request to distributing a macOS installer. Users can follow the [Homebrew installation guide](HOMEBREW_CASK_GUIDE.md) or download the [latest release](https://github.com/KrabsWong/yes-sessions/releases/latest).

## Choosing a release

Regular changes enter `main` through feature branches and pull requests. Before merging, assign exactly one release label to the PR:

| Label | Purpose |
| --- | --- |
| `major` | Major version changes, such as incompatible changes |
| `minor` | New features |
| `patch` | Fixes and small improvements |
| `skip-release` | Documentation, configuration, or other changes that need no release |

Do not rely on the workflow defaulting to `patch` when no label is present. PR labels select the version increment; Git tags in the form `vMAJOR.MINOR.PATCH` trigger package publication. They are separate mechanisms.

## Automated release sequence

1. After a PR is merged, the [version workflow](../../.github/workflows/auto-version-bump.yml) checks for `skip-release` and skips publication if it is present.
2. If the current version in `Cargo.toml` has no matching Git tag, that version is released as-is. Otherwise, the workflow increments the version according to the PR label and updates the lockfile.
3. The workflow uses `WORKFLOW_PAT` to push the version commit and Git tag atomically.
4. The [release workflow](../../.github/workflows/release.yml) verifies that the tag matches the Cargo version, runs formatting checks, compilation checks, and tests, then builds the Apple Silicon package.
5. The [packaging script](../../scripts/package-macos.sh) signs and verifies the app. With complete Apple credentials, it also notarizes and staples the distribution.
6. It publishes `Yes-Sessions-<version>-arm64.dmg` to the main repository's GitHub Release, then uploads the same file to the [Homebrew Tap release](https://github.com/KrabsWong/homebrew-yes-sessions/releases).
7. It generates the Cask using the final DMG's SHA256, synchronizes the Tap README, and pushes the Tap's `main` branch.

Distribution currently targets Apple Silicon Macs running macOS 13 or later. There is no Intel installer. A release is complete only when the release workflow succeeds, the Release asset is downloadable, and the Tap update has finished.

## Repository configuration

Configure these GitHub Actions Secrets. Never commit credentials to the repository.

| Secret | Purpose |
| --- | --- |
| `WORKFLOW_PAT` | Push version commits and Git tags to the main repository; repository rules must permit this |
| `HOMEBREW_TAP_TOKEN` | Upload Release assets and push the Cask to `KrabsWong/homebrew-yes-sessions` |
| `MACOS_CERTIFICATE` | Base64-encoded Developer ID Application `.p12` certificate |
| `MACOS_CERTIFICATE_PASSWORD` | Export password for the certificate |
| `CODESIGN_IDENTITY` | Developer ID Application signing identity |
| `APPLE_ID` | Apple account used for notarization |
| `APPLE_TEAM_ID` | Apple developer team ID |
| `APPLE_APP_PASSWORD` | App-specific password used for notarization |

The first two are required by the current automated pipeline. Configure all six Apple signing credentials or leave all six unset; partial configuration stops the release. Leaving them unset produces an ad-hoc signed installer without Apple notarization. Release notes state the signing status.

`WORKFLOW_PAT` ensures that pushing a Git tag can trigger the downstream release workflow; do not simply replace it with the default `GITHUB_TOKEN`. The Tap repository and assets must be accessible to installation users. Public distribution should not require users to authenticate to download the installer.

## Verification before merging

Run these commands from the repository root in a macOS development environment:

```bash
cargo fmt --all --check
cargo check --workspace --locked
cargo test --workspace --locked
./scripts/package-macos.sh
codesign --verify --deep --strict "target/macos/Yes Sessions.app"
```

Local packaging uses ad-hoc signing by default, writes the app to `target/macos/Yes Sessions.app`, and writes the DMG to `release/`. This verifies building and packaging; it does not prove that GitHub permissions, cross-repository uploads, or Apple notarization work.

Also confirm the PR label is correct, version changes are intentional, and the full outgoing Git history contains no secrets or credentials. Do not update the live Tap early with a local package's SHA256: the Cask must match the final published asset.

## Recovering from a failed release

- **Version workflow failure:** Check token permissions, repository rules, version numbers, and existing tags before rerunning the failed job. Do not move or overwrite published Git tags.
- **Build, signing, or notarization failure:** Fix the cause and rerun the release job. If the correct tag already exists, you can also run Release manually in Actions with that tag; another version increment is unnecessary.
- **Main release published but Tap update failed:** Fix Tap permissions or upload problems, then rerun the same version. The workflow can overwrite a Tap asset with the same name and does not create a duplicate commit if the Cask is unchanged.

A rerun rebuilds the installer. Allow the remaining upload and checksum update steps to finish so that the asset and Cask stay consistent. Do not rerun an old version over the current Tap, or publish different versions concurrently: the current workflow has no release concurrency lock.

## Maintenance entry points

- [Version workflow](../../.github/workflows/auto-version-bump.yml)
- [Release workflow](../../.github/workflows/release.yml)
- [macOS packaging script](../../scripts/package-macos.sh)
- [Cask template](../../packaging/yes-sessions.rb.in) and [renderer](../../scripts/render-homebrew-cask.py)
- [Tap README template](../../packaging/homebrew-README.md)
