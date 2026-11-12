#!/usr/bin/env bash
# Build Shaft.app (and optionally a DMG).
#
#   scripts/bundle.sh                 # native arch, ad-hoc signed
#   UNIVERSAL=1 scripts/bundle.sh     # arm64 + x86_64 (needs both rustup targets)
#   SIGN_IDENTITY="Developer ID Application: …" scripts/bundle.sh
#   NOTARY_PROFILE=shaft DMG=1 scripts/bundle.sh   # notarize (xcrun notarytool store-credentials)
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)
VERSION=$(/usr/bin/sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
BUILD=${BUILD_NUMBER:-$(git rev-list --count HEAD 2>/dev/null || echo 1)}
DIST="$ROOT/dist"
APP="$DIST/Shaft.app"
IDENTITY=${SIGN_IDENTITY:--}

echo "==> Building Shaft $VERSION ($BUILD)"
if [[ "${UNIVERSAL:-0}" == "1" ]]; then
  cargo build --release -p shaft-app --target aarch64-apple-darwin
  cargo build --release -p shaft-app --target x86_64-apple-darwin
  BIN="$DIST/shaft-universal"
  mkdir -p "$DIST"
  /usr/bin/lipo -create -output "$BIN" \
    target/aarch64-apple-darwin/release/shaft \
    target/x86_64-apple-darwin/release/shaft
else
  cargo build --release -p shaft-app
  BIN="target/release/shaft"
fi

echo "==> Assembling $APP"
/bin/rm -rf "$APP"
/bin/mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
/bin/cp "$BIN" "$APP/Contents/MacOS/shaft"
/usr/bin/sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" packaging/Info.plist > "$APP/Contents/Info.plist"
if [[ -f packaging/AppIcon.icns ]]; then
  /bin/cp packaging/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
  /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string AppIcon" "$APP/Contents/Info.plist"
fi

echo "==> Signing ($IDENTITY)"
SIGN_ARGS=(--force --options runtime --entitlements packaging/entitlements.plist --sign "$IDENTITY")
[[ "$IDENTITY" != "-" ]] && SIGN_ARGS+=(--timestamp)
/usr/bin/codesign "${SIGN_ARGS[@]}" "$APP"
/usr/bin/codesign --verify --strict --verbose=2 "$APP"

if [[ "${DMG:-0}" == "1" ]]; then
  DMG_PATH="$DIST/Shaft-$VERSION.dmg"
  echo "==> Creating $DMG_PATH"
  /bin/rm -f "$DMG_PATH"
  STAGE=$(mktemp -d)
  /bin/cp -R "$APP" "$STAGE/"
  /bin/ln -s /Applications "$STAGE/Applications"
  /usr/bin/hdiutil create -volname "Shaft" -srcfolder "$STAGE" -ov -format UDZO "$DMG_PATH" >/dev/null
  /bin/rm -rf "$STAGE"
  [[ "$IDENTITY" != "-" ]] && /usr/bin/codesign --sign "$IDENTITY" --timestamp "$DMG_PATH"
  if [[ -n "${NOTARY_PROFILE:-}" ]]; then
    echo "==> Notarizing"
    /usr/bin/xcrun notarytool submit "$DMG_PATH" --keychain-profile "$NOTARY_PROFILE" --wait
    /usr/bin/xcrun stapler staple "$DMG_PATH"
  fi
  /usr/bin/shasum -a 256 "$DMG_PATH"
fi

echo "==> Done: $APP"
