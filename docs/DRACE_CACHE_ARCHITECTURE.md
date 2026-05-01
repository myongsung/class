# DRACE Cache Architecture

## 목적

이번 작업의 목표는 **캐시를 더 많이 쓰는 것**이 아니라, **캐시 miss 비용을 거의 0으로 만들고 cache hit일 때만 실제 계산량을 줄이는 DRACE Cache v2**를 구현하는 것이다. 캐시가 이득이 없으면 자동으로 bypass되어야 하며, unsupported 기능은 절대 active처럼 표시하지 않는다.

## 지원 원칙

- **Response cache 금지**
  - 최종 답변 본문을 그대로 재사용하지 않는다.
  - cached answer를 현재 사건 사실처럼 삽입하지 않는다.
- **Prefix KV Cache**
  - 모델 프로세스가 상주하고 같은 context/slot 안에서 KV state를 재사용할 때만 활성화한다.
  - 현재 CLI sidecar backend에서는 `unsupported`로 처리한다.
- **Synthetic Token Cache**
  - target model의 token-level verification이 있을 때만 active로 표시한다.
  - verification이 없으면 loaded 상태로도 active로 표시하지 않는다.
- **Prompt Token Cache**
  - static prefix tokenization을 재사용할 수 있는 backend에서만 active로 표시한다.
  - 현재 CLI sidecar backend에서는 tokenizer state를 제어할 수 없어 `unsupported`로 처리한다.

## 계층형 가속 구조

RoosyCozy의 DRACE v2는 다음 순서로만 가속을 적용한다.

1. **PrefixKV / cache_prompt**
   - resident backend에서 static prefix를 먼저 재사용해 prefill 비용을 줄인다.
2. **PrefixKV + TemplateRenderer**
   - 보고서의 고정 섹션, 표 머리글, 안내 문구는 앱이 deterministic renderer로 삽입한다.
   - 모델은 구조화된 JSON만 생성한다.
3. **FullDRACE**
   - target model token verification이 가능한 backend에서만 Synthetic Token Cache까지 확장한다.

즉 현재 `llama-server`는 1차 PrefixKV와 `PrefixKV+TemplateRenderer`까지만 활성화되고,
Synthetic Token Cache는 `verification unsupported`이면 반드시 PrefixKV로 안전하게 fallback 한다.

## Backend 지원 상태

| Backend | Prefix KV | Prompt Token Cache | Synthetic Token Verify |
| --- | --- | --- | --- |
| CLI sidecar | unsupported | unsupported | unsupported |
| LlamaServer | supported (`cache_prompt`) | not active yet | unsupported |
| Native | future | future | future |

## Prompt Segment 구조

프롬프트는 다음 순서로 조립한다.

1. `StaticSystemPrefix`
2. `StaticModeTemplate`
3. `StaticStageInstruction`
4. `StaticOutputFormat`
5. `DynamicCaseContext`
6. `DynamicEvidencePacket`
7. `DynamicLegalRefs`
8. `DynamicConversation`
9. `DynamicUserMessage`
10. `DynamicDraftArtifacts`

정적 prefix에는 날짜, UUID, 현재 시각, 사건별 동적 값을 넣지 않는다.

## Cache 상태 정의

- `cache requested`
  - 사용자가 캐시 토글을 켠 상태
- `cache loaded`
  - backend가 해당 캐시 자산/기능을 실제로 준비했고 hot path에 올릴 수 있는 상태
- `cache applied`
  - 현재 stage에서 실제로 계산 절감 경로가 적용된 상태

### Requested / Applied mode

- `Off`
- `PrefixKV`
- `PrefixKV+TemplateRenderer`
- `FullDRACE`

`FullDRACE`가 요청되더라도 token verification이 없으면 `Applied mode`는 `PrefixKV` 또는
`PrefixKV+TemplateRenderer`로만 내려가야 한다.

## Bypass 규칙

다음 중 하나면 cache는 자동 우회한다.

- backend unsupported
- 짧은 출력
- cache-friendly stage 아님
- cold cache
- acceptance/lookup 조건 불충분

## 지표 정의

- **E2E Latency**: 입력부터 최종 답변 완료까지
- **TTFT**: 첫 토큰/첫 문장까지 시간
- **Peak Working Set**: 실행 중 최대 메모리 사용량
- **E2E TPS**: 전체 완료 시간 기준 초당 출력 토큰 수
- **Decode TPS**: decode 구간 기준 초당 출력 토큰 수
- **Final Stage TPS**: 최종 답변 stage 기준 초당 출력 토큰 수

## Benchmark 출력

실행 결과는 앱 데이터 폴더 아래 `benchmark_results/rc_disputebench_runs.jsonl`에 기록된다.

각 row는 stage 기준이며 다음을 포함한다.

- `run_id`
- `cache_enabled`
- `cache_loaded`
- `cache_applied`
- `cache_mode_requested`
- `cache_mode_applied`
- `model_id`
- `stage_name`
- `prompt_tokens`
- `output_tokens`
- `e2e_ms`
- `ttft_ms`
- `tps`
- `peak_memory_mb`
- `prompt_token_cache_*`
- `prefix_*`
- `token_cache_*`
- `accepted_tokens_per_verify`
- `synthetic_cache_*`
- `draft_provider`
- `renderer_inserted_tokens`
- `llm_generated_tokens`
- `output_token_reduction_ratio`
- `cache_bypass_reason`

## 현재 한계

현재 RoosyCozy는 기본적으로 local CLI sidecar를 매 요청마다 새로 띄운다. 따라서:

- resident Prefix KV reuse는 없다.
- target-model token verification hook도 없다.
- CLI에서는 DRACE가 **정확한 계측 + 자동 우회** 중심으로만 동작한다.
- 실제 Prefix KV/cache_prompt 가속은 resident backend인 LlamaServer에서만 활성화할 수 있다.

진짜 cache speedup을 원하면 다음 단계는 resident backend 전환이다.
