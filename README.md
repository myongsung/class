# roosycozy (Tauri v2 + Vite)

웹(`npm run dev`)과 데스크톱(`npm run tauri dev`) 모두에서 **동일한 코드**로 동작하는 완성 예제입니다.

- Web(dev): `localStorage`
- Desktop(Tauri v2): `AppDataDir/roosycozy_state_v1.json` 파일에 저장 (tauri plugin-fs)
- 기록 삭제 정책: 해당 기록이 포함되는 케이스가 존재하면 삭제 불가
- 케이스 삭제: 언제든 가능
- 디버그 패널: 오른쪽 상단 **🐞** 버튼 또는 `Ctrl/Cmd + \` 로 토글 (Tauri에서 콘솔이 안 보일 때 유용)

## 실행

```bash
npm install --legacy-peer-deps

# 브라우저에서만 실행
npm run dev

# 데스크톱(Tauri) 실행
npm run tauri dev
```

## 데스크톱에서 DevTools(Inspect) 열기

Tauri dev 실행 중에는 보통 **우클릭 → Inspect** 로 웹 인스펙터를 열 수 있습니다. (OS/환경에 따라 단축키가 다를 수 있어요.)

만약 버튼이 먹통처럼 보이면, 이 프로젝트는 화면 안에 **🐞 디버그 패널**을 제공해서 저장/에러 로그를 바로 볼 수 있게 해뒀습니다.

## 저장 파일 위치

Tauri(AppDataDir)에 아래 파일로 저장됩니다.

- `roosycozy_state_v1.json`

(정확한 경로는 OS별 AppDataDir 규칙에 따릅니다.)

## 빌드 전략

- `Windows 배포용`은 GitHub Actions에서 **portable zip**을 만들고 GitHub Release에 자동 업로드합니다.
- 현재 배포 기준은 **Windows 중심**이며, AI 모델과 runtime은 앱 실행 흐름 안에서 필요한 시점에 정리되도록 맞춰져 있습니다.

## GitHub Actions

### Windows Release Build

파일: `.github/workflows/windows-release.yml`

- `main` 브랜치 push 또는 수동 실행 시 동작합니다.
- 결과물은 GitHub Release와 Actions artifact로 함께 올라갑니다.
- Windows portable 배포 zip은 이제 `roosycozy.exe`만 담습니다. resident `llama-server`와 관련 DLL은 첫 실행 시 `%APPDATA%/.../sidecar` 아래로 자동 확보되고, 모델은 필요할 때 `Public/Documents/.../models` 아래로 자동 다운로드됩니다. 기본 Windows runtime 소스는 2026년 4월 11일 기준 공식 llama.cpp Windows x64 CPU asset입니다.

```text
https://github.com/ggml-org/llama.cpp/releases/download/b8763/llama-b8763-bin-win-cpu-x64.zip
```

필요하면 `ROOSYCOZY_WINDOWS_RUNTIME_URL`로 다른 runtime zip을 지정할 수 있습니다. 예를 들어 네가 직접 빌드한 같은 버전의 DLL 묶음을 올려두고 그 주소를 넣는 방식이 가장 안전합니다.

## GitHub Actions secrets / vars

모델 파일이 저장소에 없을 경우 아래 둘 중 하나가 필요합니다.

- `ROOSYCOZY_MODEL_PATH`
  - self-hosted runner나 로컬 경로가 있을 때 사용
- `ROOSYCOZY_MODEL_URL`
  - GitHub Actions가 모델을 받아올 URL
- `ROOSYCOZY_ROOSY_MODEL_PATH`
  - Roosy-X 로컬 모델 경로
- `ROOSYCOZY_ROOSY_MODEL_URL`
  - GitHub Actions가 Roosy-X 모델을 받아올 URL
- `ROOSYCOZY_WINDOWS_RUNTIME_URL`
  - Windows llama-server / DLL 묶음 zip URL
- `ROOSYCOZY_MODEL_SHA256`
  - 선택사항
  - 내려받은 모델 무결성 검사용
- `ROOSYCOZY_ROOSY_MODEL_SHA256`
  - 선택사항
  - 내려받은 Roosy-X 모델 무결성 검사용

현재 워크플로에는 아래 public release asset URL이 기본값으로 들어가 있어, 별도 설정이 없어도 우선 이 주소에서 모델을 받습니다.

```text
https://github.com/myongsung/roosycozy-models/releases/download/model_v1/HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf
```

Roosy-X도 기본값이 연결되어 있어, 별도 설정이 없으면 아래 public release asset URL에서 같이 받아옵니다.

```text
https://github.com/myongsung/roosycozy-models2/releases/download/model/hyperclovax_roosy_Q4_K_M.gguf
```
