pub mod backends;
pub mod cache_manager;
pub mod draft_provider;
pub mod native_backend;
pub mod prompt_segments;
pub mod synthetic_token_cache;

pub use backends::{
  detect_backend_runtime, BackendRuntime, DraftVerifyResult, GenerationSessionApi,
  GenerationSessionHandle, GenerationSessionOptions, GenerationStepResult,
  LlamaServerConfig, LlamaServerSessionBackend,
};
pub use native_backend::{native_verifier_available, NativeSessionBackend};
pub use cache_manager::{
  BackendCapabilities, BackendKind, DraceCacheConfig, DraceCacheManager, DraceCacheStats,
  HybridStage, PrefixKvCacheBackend, PrefixKvHandle, PrefixKvStats, PreparedStagePrompt,
  PromptTokenCacheKey, PromptTokenCacheValue, StageCachePlan, StaticPrefixId, StaticPrefixSpec,
  TokenAcceptRule, TokenCandidateVerifier, TokenVerifyResult,
};
pub use draft_provider::{
  DraftProposal, DraftProvider, DraftProviderState, NoopDraftProvider, TemplateDraftProvider,
};
pub use prompt_segments::{fast_hash64, render_prompt_segments, PromptSegment, PromptSegmentKind};
