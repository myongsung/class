#!/bin/zsh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_PATH="${APP_PATH:-$ROOT_DIR/src-tauri/target/release/bundle/macos/roosycozy.app}"
APP_IDENTITY="${APP_SIGN_IDENTITY:-Apple Distribution: MyongSung Noh (HDQ2YRZVGB)}"
APP_ENTITLEMENTS="$ROOT_DIR/src-tauri/entitlements.macos.plist"
SIDECAR_ENTITLEMENTS="$ROOT_DIR/src-tauri/entitlements.sidecar.plist"
PLIST="$APP_PATH/Contents/Info.plist"
RESOURCES_DIR="$APP_PATH/Contents/Resources"
SIDECAR_DIR="$APP_PATH/Contents/MacOS/sidecar"
MAIN_EXECUTABLE="$APP_PATH/Contents/MacOS/roosycozy"
ICON_SOURCE="$ROOT_DIR/src-tauri/icons/icon.icns"
ICON_DEST="$RESOURCES_DIR/icon.icns"
EMBEDDED_PROFILE_SOURCE="$ROOT_DIR/src-tauri/embedded.provisionprofile"
EMBEDDED_PROFILE_DEST="$APP_PATH/Contents/embedded.provisionprofile"

if [[ ! -d "$APP_PATH" ]]; then
  echo "앱 번들을 찾지 못했어요: $APP_PATH"
  echo "먼저 appstore 빌드를 실행해주세요."
  exit 1
fi

if [[ ! -f "$ICON_SOURCE" ]]; then
  echo "아이콘 파일을 찾지 못했어요: $ICON_SOURCE"
  exit 1
fi

xattr -d com.apple.quarantine "$EMBEDDED_PROFILE_SOURCE" 2>/dev/null || true
xattr -d com.apple.metadata:kMDItemWhereFroms "$EMBEDDED_PROFILE_SOURCE" 2>/dev/null || true

mkdir -p "$RESOURCES_DIR"
cp "$ICON_SOURCE" "$ICON_DEST"

/usr/libexec/PlistBuddy -c "Set :ITSAppUsesNonExemptEncryption false" "$PLIST" 2>/dev/null \
  || /usr/libexec/PlistBuddy -c "Add :ITSAppUsesNonExemptEncryption bool false" "$PLIST"
/usr/libexec/PlistBuddy -c "Set :CFBundleIconFile icon.icns" "$PLIST" 2>/dev/null \
  || /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string icon.icns" "$PLIST"

if [[ -f "$EMBEDDED_PROFILE_DEST" ]]; then
  xattr -d com.apple.quarantine "$EMBEDDED_PROFILE_DEST" 2>/dev/null || true
  xattr -d com.apple.metadata:kMDItemWhereFroms "$EMBEDDED_PROFILE_DEST" 2>/dev/null || true
fi

xattr -dr com.apple.quarantine "$APP_PATH" 2>/dev/null || true

if [[ -d "$SIDECAR_DIR" ]]; then
  find "$SIDECAR_DIR" -type f -name '*.dylib' -print0 | while IFS= read -r -d '' dylib; do
    codesign --force --sign "$APP_IDENTITY" "$dylib"
  done

  if [[ -f "$SIDECAR_DIR/llama-sidecar-aarch64-apple-darwin" ]]; then
    codesign \
      --force \
      --sign "$APP_IDENTITY" \
      --entitlements "$SIDECAR_ENTITLEMENTS" \
      "$SIDECAR_DIR/llama-sidecar-aarch64-apple-darwin"
  fi
fi

codesign --force --sign "$APP_IDENTITY" "$MAIN_EXECUTABLE"
codesign \
  --force \
  --sign "$APP_IDENTITY" \
  --entitlements "$APP_ENTITLEMENTS" \
  "$APP_PATH"

echo "App Store 제출용 앱 번들을 정리하고 다시 서명했습니다: $APP_PATH"
