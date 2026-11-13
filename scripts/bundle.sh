#!/usr/bin/env bash
# Build Shaft.app (and optionally a signed, notarized DMG).
#
#   scripts/bundle.sh                 # native arch, ad-hoc signed (local use only)
#   UNIVERSAL=1 scripts/bundle.sh     # arm64 + x86_64 (needs both rustup targets)
#   SIGN_IDENTITY="Developer ID Application: Name (TEAMID)" DMG=1 scripts/bundle.sh
#
# Notarization (needs a "Developer ID Application" identity) runs when one of
# these credential sets is present; see docs/RELEASING.md:
#   NOTARY_PROFILE=shaft                           # xcrun notarytool store-credentials
#   NOTARY_KEY=AuthKey.p8 NOTARY_KEY_ID=… NOTARY_ISSUER=…   # App Store Connect API key
#   APPLE_ID=… APPLE_TEAM_ID=… APPLE_APP_PASSWORD=…         # app-specific password
# The app is notarized and stapled first, then the DMG that contains it.
set -euo pipefail

cd "$(dirname "$0")/.."

# rustup installs cargo in ~/.cargo/bin, which non-login shells may not have on PATH.
if ! command -v cargo >/dev/null 2>&1; then
  if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
  fi
  export PATH="$HOME/.cargo/bin:$PATH"
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found. Install Rust from https://rustup.rs and retry." >&2
  exit 1
fi
ROOT=$(pwd)
VERSION=$(/usr/bin/sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
BUILD=${BUILD_NUMBER:-$(git rev-list --count HEAD 2>/dev/null || echo 1)}
DIST="$ROOT/dist"
APP="$DIST/Shaft.app"
IDENTITY=${SIGN_IDENTITY:--}

echo "==> Building Shaft $VERSION ($BUILD)"
if [[ "${UNIVERSAL:-0}" == "1" ]]; then
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    if ! rustup target list --installed | grep -qx "$target"; then
      echo "error: Rust target $target is not installed. Run: rustup target add $target" >&2
      exit 1
    fi
  done
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

NOTARY_ARGS=()
if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  NOTARY_ARGS=(--keychain-profile "$NOTARY_PROFILE")
elif [[ -n "${NOTARY_KEY:-}" ]]; then
  NOTARY_ARGS=(--key "$NOTARY_KEY" --key-id "${NOTARY_KEY_ID:?NOTARY_KEY_ID is required with NOTARY_KEY}")
  [[ -n "${NOTARY_ISSUER:-}" ]] && NOTARY_ARGS+=(--issuer "$NOTARY_ISSUER")
elif [[ -n "${APPLE_ID:-}" ]]; then
  NOTARY_ARGS=(--apple-id "$APPLE_ID" --team-id "${APPLE_TEAM_ID:?APPLE_TEAM_ID is required with APPLE_ID}"
    --password "${APPLE_APP_PASSWORD:?APPLE_APP_PASSWORD is required with APPLE_ID}")
fi
if [[ ${#NOTARY_ARGS[@]} -gt 0 && "$IDENTITY" != Developer\ ID\ Application* ]]; then
  echo "error: notarization needs SIGN_IDENTITY=\"Developer ID Application: …\" (got: $IDENTITY)" >&2
  exit 1
fi

# Submit $1 to the notary service, wait, and fail with the log if rejected.
notarize() {
  local file=$1 out id status
  echo "==> Notarizing $(basename "$file")"
  out=$(/usr/bin/xcrun notarytool submit "$file" "${NOTARY_ARGS[@]}" --wait --output-format json)
  id=$(/usr/bin/plutil -extract id raw -o - - <<<"$out")
  status=$(/usr/bin/plutil -extract status raw -o - - <<<"$out")
  if [[ "$status" != "Accepted" ]]; then
    echo "error: notarization of $(basename "$file") ended with status: $status" >&2
    /usr/bin/xcrun notarytool log "$id" "${NOTARY_ARGS[@]}" >&2 || true
    exit 1
  fi
}

echo "==> Signing ($IDENTITY)"
SIGN_ARGS=(--force --options runtime --entitlements packaging/entitlements.plist --sign "$IDENTITY")
[[ "$IDENTITY" != "-" ]] && SIGN_ARGS+=(--timestamp)
/usr/bin/codesign "${SIGN_ARGS[@]}" "$APP"
/usr/bin/codesign --verify --strict --deep --verbose=2 "$APP"

if [[ ${#NOTARY_ARGS[@]} -gt 0 ]]; then
  ZIP="$DIST/Shaft-$VERSION-notarize.zip"
  /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
  notarize "$ZIP"
  /bin/rm -f "$ZIP"
  /usr/bin/xcrun stapler staple "$APP"
  /usr/sbin/spctl --assess --type execute --verbose=2 "$APP"
fi

if [[ "${DMG:-0}" == "1" ]]; then
  DMG_PATH="$DIST/Shaft-$VERSION.dmg"
  echo "==> Creating $DMG_PATH"
  /bin/rm -f "$DMG_PATH"
  STAGE=$(mktemp -d)
  /usr/bin/ditto "$APP" "$STAGE/Shaft.app"
  /bin/ln -s /Applications "$STAGE/Applications"
  /usr/bin/hdiutil create -volname "Shaft" -srcfolder "$STAGE" -ov -format UDZO "$DMG_PATH" >/dev/null
  /bin/rm -rf "$STAGE"
  if [[ "$IDENTITY" != "-" ]]; then
    /usr/bin/codesign --force --sign "$IDENTITY" --timestamp "$DMG_PATH"
  fi
  if [[ ${#NOTARY_ARGS[@]} -gt 0 ]]; then
    notarize "$DMG_PATH"
    /usr/bin/xcrun stapler staple "$DMG_PATH"
    /usr/sbin/spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG_PATH"
  fi
  (cd "$DIST" && /usr/bin/shasum -a 256 "$(basename "$DMG_PATH")" | tee "$(basename "$DMG_PATH").sha256")
fi

echo "==> Done: $APP"
