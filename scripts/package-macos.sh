#!/bin/bash

set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
bundle_root="$project_root/target/macos/Yes Sessions.app"
contents="$bundle_root/Contents"
identity="${CODESIGN_IDENTITY:--}"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$project_root/Cargo.toml" | head -n 1)"
marketing_version="${version%%-*}"
build_target="${CARGO_BUILD_TARGET:-aarch64-apple-darwin}"
case "$build_target" in
  aarch64-apple-darwin) architecture="arm64" ;;
  *)
    echo "Unsupported macOS build target: $build_target" >&2
    exit 1
    ;;
esac
dmg_root="$project_root/target/macos/dmg-root"
dmg_path="$project_root/release/Yes-Sessions-$version-$architecture.dmg"
notary_zip="$project_root/target/macos/Yes-Sessions-notarization.zip"

notarize=false
if [ -n "${APPLE_ID:-}" ] || [ -n "${APPLE_TEAM_ID:-}" ] || [ -n "${APPLE_APP_PASSWORD:-}" ]; then
  if [ -z "${APPLE_ID:-}" ] || [ -z "${APPLE_TEAM_ID:-}" ] || [ -z "${APPLE_APP_PASSWORD:-}" ]; then
    echo "APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD must be provided together" >&2
    exit 1
  fi
  if [ "$identity" = "-" ]; then
    echo "A Developer ID signing identity is required for notarization" >&2
    exit 1
  fi
  notarize=true
fi

cd "$project_root"
export MACOSX_DEPLOYMENT_TARGET=13.0
export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-mmacosx-version-min=13.0"
cargo build --release --package yes-sessions --target "$build_target"

rm -rf "$bundle_root"
mkdir -p "$contents/MacOS" "$contents/Resources"
cp "$project_root/target/$build_target/release/yes-sessions" "$contents/MacOS/yes-sessions"
cp "$project_root/packaging/Info.plist" "$contents/Info.plist"
cp "$project_root/build/icon.icns" "$contents/Resources/icon.icns"
cp "$project_root/crates/yes-app/assets/mermaid.min.js" "$contents/Resources/mermaid.min.js"
cp "$project_root/crates/yes-app/third-party/MERMAID-LICENSE" "$contents/Resources/MERMAID-LICENSE"

plutil -replace CFBundleShortVersionString -string "$marketing_version" "$contents/Info.plist"
plutil -replace CFBundleVersion -string "$marketing_version" "$contents/Info.plist"

chmod 755 "$contents/MacOS/yes-sessions"
if [ "$identity" = "-" ]; then
  codesign --force --options runtime --timestamp=none --sign - "$bundle_root"
else
  codesign --force --options runtime --timestamp --sign "$identity" "$bundle_root"
fi

plutil -lint "$contents/Info.plist"
codesign --verify --deep --strict --verbose=2 "$bundle_root"
file "$contents/MacOS/yes-sessions" | grep -q "arm64"

if [ "$notarize" = true ]; then
  rm -f "$notary_zip"
  ditto -c -k --keepParent "$bundle_root" "$notary_zip"
  xcrun notarytool submit "$notary_zip" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" \
    --wait
  xcrun stapler staple "$bundle_root"
  xcrun stapler validate "$bundle_root"
  rm -f "$notary_zip"
fi

rm -rf "$dmg_root"
mkdir -p "$dmg_root" "$project_root/release"
cp -R "$bundle_root" "$dmg_root/Yes Sessions.app"
ln -s /Applications "$dmg_root/Applications"
rm -f "$dmg_path"
hdiutil create \
  -volname "Yes Sessions" \
  -srcfolder "$dmg_root" \
  -ov \
  -format UDZO \
  "$dmg_path"
rm -rf "$dmg_root"

if [ "$identity" != "-" ]; then
  codesign --force --timestamp --sign "$identity" "$dmg_path"
  codesign --verify --verbose=2 "$dmg_path"
fi

if [ "$notarize" = true ]; then
  xcrun notarytool submit "$dmg_path" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" \
    --wait
  xcrun stapler staple "$dmg_path"
  xcrun stapler validate "$dmg_path"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg_path"
fi

echo "$bundle_root"
du -sh "$bundle_root"
echo "$dmg_path"
du -sh "$dmg_path"
