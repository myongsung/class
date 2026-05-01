# DRACE Benchmark Notes

이번 작업의 목적은 "캐시를 무조건 켜는 것"이 아니라, 캐시가 실제 forward/prefill 계산을 줄였는지 계측하고, 이득이 없으면 자동으로 우회하는 adaptive DRACE benchmark/runtime을 만드는 것이다.

## Current backend reality

RoosyCozy의 현재 로컬 추론 경로는 stage마다 별도 llama sidecar/CLI 프로세스를 실행한다.

- Backend type: `CLI`
- Persistent Prefix KV Cache: `unsupported`
- Verified Synthetic Token Cache: `unsupported`

따라서 현재 DRaCE 런타임은 다음 원칙을 따른다.

1. `cacheRequested=true`여도 실제 Prefix KV state를 재사용하지 않으면 `cacheApplied=false`로 기록한다.
2. target model logits/top-token 검증이 없는 synthetic cache는 `tokenCacheSupported=false`로 기록한다.
3. 지원되지 않는 캐시는 지표에서 가속 기여로 계산하지 않는다.
4. CLI backend에서는 baseline과 동일한 실제 추론 경로를 사용하고, cache 관련 메트릭은 `requested but bypassed`로 남긴다.

## Metric definitions

### Top-level

- `E2E Latency`: 입력부터 최종 답변 완료까지
- `TTFT`: 첫 토큰/첫 문장까지 시간
- `Peak Working Set`: 현재는 stage별 모델 footprint 합산 기반의 근사치
- `E2E TPS`: 전체 완료 시간 기준 초당 출력 토큰 수
- `Decode TPS`: 전체 decode 시간 기준 초당 출력 토큰 수
- `Final Stage TPS`: 최종 답변 stage 기준 초당 출력 토큰 수

### Stage metrics

각 stage는 아래 값을 기록한다.

- `stageName`
- `modelId`
- `e2eMs`
- `ttftMs`
- `promptTokens`
- `outputTokens`
- `promptEvalMs`
- `decodeMs`
- `e2eTps`
- `decodeTps`
- `peakMemoryMb`

### Cache metrics

각 stage cache 블록은 아래를 포함한다.

- `cacheRequested`
- `cacheSupported`
- `cacheWarm`
- `cacheApplied`
- `bypassReason`
- `prefixKvSupported`
- `prefixReusedTokens`
- `prefixTotalTokens`
- `prefixReuseRatio`
- `kvLoadMs`
- `kvSaveMs`
- `tokenCacheSupported`
- `tokenCacheLookupMs`
- `proposedTokens`
- `acceptedTokens`
- `verifyBatches`
- `rejectedBatches`
- `acceptedTokensPerVerify`
- `fallbackTokens`

## Speedup wording

UI에서는 아래 규칙으로만 비교 문구를 표시한다.

- `speed_ratio = baseline_e2e / current_e2e`
- `speed_ratio > 1.05` → `x× faster`
- `0.95 <= speed_ratio <= 1.05` → `about same`
- `speed_ratio < 0.95` → `% slower`

같은 규칙을 TPS에도 적용한다.

## Warmup rule

프론트 benchmark 패널은 아래 방식으로 rolling sample을 관리한다.

- Baseline 최근 3회
- DRaCE 최근 5회
- `cacheRequested && cacheApplied && !cacheWarm` 인 실행은 warmup으로 간주하고 평균에서 제외

현재 CLI backend에서는 `cacheApplied=false`이므로 warmup run이 생성되지 않는다.

## Benchmark export

성공한 strategy run은 stage별 row를 JSONL로 저장한다.

- path: `app_data_dir()/benchmark_results/rc_disputebench_runs.jsonl`

각 row는 다음 필드를 포함한다.

- `run_id`
- `cache_requested`
- `cache_loaded`
- `cache_applied`
- `requested_mode`
- `applied_mode`
- `bypass_reason`
- `prompt_hash`
- `model_config_hash`
- `model_id`
- `stage_name`
- `prompt_tokens`
- `output_tokens`
- `e2e_ms`
- `ttft_ms`
- `decode_ms`
- `e2e_tps`
- `decode_tps`
- `peak_memory_mb`
- `prefix_reuse_ratio`
- `accepted_tokens_per_verify`
- `cache_bypass_reason`
- `orchestration_overhead_ms`
- `backend_type`
- `prefix_kv_supported`
- `token_cache_supported`
- `benchmark_phase`

## Why DRACE may show no gain

현재 CLI backend에서는 진짜 Prefix KV와 검증형 Synthetic Token Cache가 없으므로,

- DRaCE가 baseline보다 느리면 `slower`
- 비슷하면 `about same`
- 빨라지지 않았는데 `faster`라고 표시하는 일은 없도록 설계한다.
