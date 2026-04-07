#!/usr/bin/env bash
set -euo pipefail

WITH_INTEL_MAC=0
WITH_WINDOWS=0
MODEL_ONLY=0
BIN_ONLY=0

for arg in "$@"; do
  case "$arg" in
    --with-intel-mac) WITH_INTEL_MAC=1 ;;
    --with-windows) WITH_WINDOWS=1 ;;
    --model-only) MODEL_ONLY=1 ;;
    --bin-only) BIN_ONLY=1 ;;
    *)
      echo "알 수 없는 옵션: $arg"
      echo "사용법: bash setup_roosycozy_sidecar.sh [--with-intel-mac] [--with-windows] [--model-only] [--bin-only]"
      exit 1
      ;;
  esac
done

ROOT_DIR="$(pwd)"
SRC_TAURI_DIR="$ROOT_DIR/src-tauri"
BIN_DIR="$SRC_TAURI_DIR/binaries"
MODEL_DIR="$SRC_TAURI_DIR/resources/models"
TMP_DIR="$ROOT_DIR/.tmp_roosycozy_sidecar"

mkdir -p "$BIN_DIR" "$MODEL_DIR" "$TMP_DIR"

echo "프로젝트 루트: $ROOT_DIR"
echo "binaries 경로: $BIN_DIR"
echo "models 경로:   $MODEL_DIR"

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "필수 명령어가 없습니다: $1"
    exit 1
  }
}

need_cmd python3
need_cmd curl
need_cmd unzip

download_latest_llama_asset() {
  local target_name="$1"
  local out_path="$2"

  python3 - "$target_name" "$out_path" <<'PY'
import json, os, sys, urllib.request, zipfile, tarfile, shutil, tempfile

target = sys.argv[1]
out_path = sys.argv[2]

api = "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest"
with urllib.request.urlopen(api) as r:
    data = json.load(r)

assets = data.get("assets", [])
names = [a.get("name","") for a in assets]

patterns = {
    "llama-sidecar-aarch64-apple-darwin": [
        "macos", "arm64"
    ],
    "llama-sidecar-x86_64-apple-darwin": [
        "macos", "x64"
    ],
    "llama-sidecar-x86_64-pc-windows-msvc.exe": [
        "win", "x64", "cpu"
    ],
}

want = patterns[target]

def score(name: str):
    low = name.lower()
    score = 0
    for token in want:
        if token in low:
            score += 1
    return score

ranked = sorted(
    [(score(a.get("name","")), a.get("name",""), a.get("browser_download_url","")) for a in assets],
    reverse=True
)

best = None
for sc, name, url in ranked:
    low = name.lower()
    if sc >= len(want) - 1 and (name.endswith(".zip") or name.endswith(".tar.gz") or name.endswith(".tgz")):
        best = (name, url)
        break

if not best:
    print("최신 llama.cpp 릴리스에서 적절한 자산을 찾지 못했습니다.")
    print("사용 가능한 asset 목록:")
    for n in names:
        print(" -", n)
    sys.exit(2)

name, url = best
print(f"다운로드 자산 선택: {name}")

tmp = tempfile.mkdtemp(prefix="roosycozy_llama_")
archive_path = os.path.join(tmp, name)
urllib.request.urlretrieve(url, archive_path)

extract_dir = os.path.join(tmp, "extract")
os.makedirs(extract_dir, exist_ok=True)

if name.endswith(".zip"):
    with zipfile.ZipFile(archive_path) as zf:
        zf.extractall(extract_dir)
elif name.endswith(".tar.gz") or name.endswith(".tgz"):
    with tarfile.open(archive_path, "r:gz") as tf:
        tf.extractall(extract_dir)
else:
    print("지원하지 않는 압축 형식:", name)
    sys.exit(3)

candidate_names = ["llama-cli", "llama-cli.exe"]
found = None
for root, dirs, files in os.walk(extract_dir):
    for fn in files:
        if fn in candidate_names:
            found = os.path.join(root, fn)
            break
    if found:
        break

if not found:
    print("압축 해제 후 llama-cli 실행 파일을 찾지 못했습니다.")
    sys.exit(4)

os.makedirs(os.path.dirname(out_path), exist_ok=True)
shutil.copy2(found, out_path)
os.chmod(out_path, 0o755)
print(f"설치 완료: {out_path}")
PY
}

download_model() {
  local model_url="https://huggingface.co/Mungert/HyperCLOVAX-SEED-Text-Instruct-0.5B-GGUF/resolve/main/HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf?download=true"
  local out_path="$MODEL_DIR/HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf"

  if [ -f "$out_path" ]; then
    echo "모델 이미 존재: $out_path"
    return
  fi

  echo "모델 다운로드 중..."
  curl -L "$model_url" -o "$out_path"
  echo "모델 설치 완료: $out_path"
}

if [ "$BIN_ONLY" -eq 0 ]; then
  download_model
fi

if [ "$MODEL_ONLY" -eq 0 ]; then
  echo "macOS Apple Silicon sidecar 설치 중..."
  download_latest_llama_asset \
    "llama-sidecar-aarch64-apple-darwin" \
    "$BIN_DIR/llama-sidecar-aarch64-apple-darwin"

  if [ "$WITH_INTEL_MAC" -eq 1 ]; then
    echo "macOS Intel sidecar 설치 중..."
    download_latest_llama_asset \
      "llama-sidecar-x86_64-apple-darwin" \
      "$BIN_DIR/llama-sidecar-x86_64-apple-darwin"
  fi

  if [ "$WITH_WINDOWS" -eq 1 ]; then
    echo "Windows x64 sidecar 설치 중..."
    download_latest_llama_asset \
      "llama-sidecar-x86_64-pc-windows-msvc.exe" \
      "$BIN_DIR/llama-sidecar-x86_64-pc-windows-msvc.exe"
  fi
fi

echo
echo "완료."
echo "다음 위치를 확인하세요:"
echo " - sidecar: $BIN_DIR"
echo " - model:   $MODEL_DIR"
