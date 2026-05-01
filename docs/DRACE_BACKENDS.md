# DRACE Backends

## 목적

이번 구조의 핵심은 `cache requested`와 `cache applied`를 분리하는 것입니다.  
지원되지 않는 backend에서 DRACE를 켠 것처럼 보이게 하지 않고, 실제 가속이 가능한 backend에서만 Prefix KV / `cache_prompt`를 적용합니다.

## 1. CLI backend

- kind: `CliSidecar`
- 목적: baseline 실행, 디버그, 호환 fallback
- resident model: 지원 안 함
- Prefix KV Cache: 지원 안 함
- Synthetic Token Cache verification: 지원 안 함
- 적용 원칙:
  - 사용자가 FullDRACE를 요청해도 `applied_mode=Off`
  - `bypass_reason=unsupported_backend_cli`
  - 실행 경로는 baseline과 같아야 함

CLI backend는 매 요청마다 별도 sidecar/CLI 프로세스를 새로 띄우므로,  
모델 내부 KV state를 재사용하는 진짜 Prefix KV Cache를 제공할 수 없습니다.

## 2. LlamaServer backend

- kind: `LlamaServer`
- 목적: resident model 기반 1차 실제 가속
- resident model: 지원
- Prefix KV / `cache_prompt`: 지원 가능
- Prompt Token Cache: 아직 active 아님
- Synthetic Token Cache verification: 1차 구현에서는 지원 안 함

적용 원칙:
- static prefix가 prompt 맨 앞에 와야 함
- `cache_prompt=true`를 요청에 포함할 수 있음
- slot/id 재사용이 가능하면 Prefix KV 계측을 활성화함
- record/report 계열 stage에서는 `PrefixKV+TemplateRenderer`를 적용할 수 있음
  - 모델은 `summary / actors / timeline / issues / evidence / recommended_questions` JSON만 생성
  - 섹션 제목, 표 머리글, 고정 안내 문구는 앱이 deterministic renderer로 삽입
- Prompt Token Cache는 tokenizer 제어 경로가 붙기 전까지 active로 표시하지 않음
- token verification이 없으므로 Synthetic Token Cache는 `loaded` 가능해도 `applied=false`

## 3. Native backend

- kind: `Native`
- 목적: 향후 token-level verification + Synthetic Token Cache
- resident model: 지원
- Prefix KV Cache: 지원 예정
- Synthetic Token Cache: target model verification 구현 후 활성화

현재는 placeholder 성격이며, verification hook이 실제로 붙기 전까지는 active처럼 표시하지 않습니다.

## 표시 원칙

### 나쁜 표시

- `DRACE active`
- `Prefix KV active`
- `Synthetic Token Cache active`

지원되지 않는 backend에서 위처럼 보이는 표시

### 좋은 표시

- `Cache requested: ON`
- `Cache loaded: false`
- `Cache applied: false`
- `Requested mode: FullDRACE`
- `Applied mode: Off`
- `Bypass reason: unsupported_backend_cli`

## 요약

- CLI: baseline/debug only
- LlamaServer: resident model + `cache_prompt` / Prefix KV 1차 실제 가속
- Native: future Synthetic Token Cache + token verification
