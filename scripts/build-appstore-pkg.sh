#!/bin/zsh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_PATH="${APP_PATH:-$ROOT_DIR/src-tauri/target/release/bundle/macos/roosycozy.app}"
OUT_PATH="${OUT_PATH:-$ROOT_DIR/src-tauri/target/release/bundle/appstore/roosycozy.pkg}"
INSTALLER_IDENTITY="${MAC_INSTALLER_IDENTITY:-}"

if [[ -z "$INSTALLER_IDENTITY" ]]; then
  echo "MAC_INSTALLER_IDENTITY 환경변수를 설정해주세요."
  echo "예: export MAC_INSTALLER_IDENTITY='3rd Party Mac Developer Installer: MyongSung Noh (HDQ2YRZVGB)'"
  exit 1
fi

if [[ ! -d "$APP_PATH" ]]; then
  echo "앱 번들을 찾지 못했어요: $APP_PATH"
  echo "먼저 npm run appstore:build 또는 npm run appstore:build:unsigned 를 실행해주세요."
  exit 1
fi

mkdir -p "$(dirname "$OUT_PATH")"
rm -f "$OUT_PATH"

echo "App Store 제출용 pkg를 생성합니다..."
xcrun productbuild \
  --sign "$INSTALLER_IDENTITY" \
  --component "$APP_PATH" \
  /Applications \
  "$OUT_PATH"

echo "완료: $OUT_PATH"
