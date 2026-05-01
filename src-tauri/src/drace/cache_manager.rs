use super::prompt_segments::{fast_hash64, PromptSegment};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BackendKind {
  CliSidecar,
  LlamaServer,
  Native,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendCapabilities {
  pub backend_kind: BackendKind,
  pub supports_resident: bool,
  pub supports_prefix_kv_cache: bool,
  pub supports_prompt_cache: bool,
  pub supports_token_verification: bool,
  pub supports_batch_verification: bool,
  pub supports_speculative_decode: bool,
  pub supports_prompt_token_cache: bool,
  pub supports_synthetic_token_cache: bool,
  pub supports_mmap_cache_pack: bool,
  pub supports_resident_model: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraceCacheConfig {
  pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraceCacheStats {
  pub prompt_token_cache_lookups: u64,
  pub prompt_token_cache_hits: u64,
  pub prompt_token_cache_lookup_ms_total: u128,
  pub warmed_prefix_count: u64,
  pub prefix_tokens_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct PromptTokenCacheKey {
  pub model_id: String,
  pub tokenizer_hash: u64,
  pub static_prefix_id: String,
  pub content_hash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTokenCacheValue {
  pub token_ids: Arc<Vec<u32>>,
  pub token_count: usize,
  pub byte_len: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefixKvHandle {
  pub model_id: String,
  pub prefix_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefixKvStats {
  pub warmed_prefix_count: u64,
  pub prefix_tokens_total: u64,
  pub prefix_reused_tokens: u64,
  pub prefix_reuse_ratio: f32,
  pub kv_memory_mb: u64,
  pub warmup_ms: u128,
  pub restore_ms: u128,
}

pub trait PrefixKvCacheBackend {
  fn supports_prefix_kv_cache(&self) -> bool;
  fn warm_prefix(&mut self, _model_id: &str, _prefix_id: &str, _tokens: &[u32]) -> Result<PrefixKvHandle, String>;
  fn start_from_prefix(&mut self, _handle: &PrefixKvHandle, _request_seq_id: u64) -> Result<(), String>;
  fn prefix_stats(&self) -> PrefixKvStats;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TokenAcceptRule {
  Top1,
  TopK { k: usize, min_logprob: f32 },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenVerifyResult {
  pub accepted_len: usize,
  pub accepted_tokens: Vec<u32>,
  pub rejected_at: Option<usize>,
  pub verify_ms: u128,
  pub target_top_tokens: Vec<u32>,
}

pub trait TokenCandidateVerifier {
  fn supports_token_verification(&self) -> bool;
  fn verify_candidate_tokens(
    &mut self,
    _model_id: &str,
    _stage: &HybridStage,
    _candidate_tokens: &[u32],
    _accept_rule: TokenAcceptRule,
  ) -> Result<TokenVerifyResult, String>;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum HybridStage {
  GeneralMain,
  GeneralDraft,
  GeneralSynthesis,
  RecordMain,
  RecordFill,
  RecordSynthesis,
  RecordReview,
  FastRoosy,
  RecordRecovery,
}

impl HybridStage {
  pub fn as_str(&self) -> &'static str {
    match self {
      HybridStage::GeneralMain => "general_main",
      HybridStage::GeneralDraft => "general_draft",
      HybridStage::GeneralSynthesis => "general_synthesis",
      HybridStage::RecordMain => "record_main",
      HybridStage::RecordFill => "record_fill",
      HybridStage::RecordSynthesis => "record_synthesis",
      HybridStage::RecordReview => "record_review",
      HybridStage::FastRoosy => "fast_roosy",
      HybridStage::RecordRecovery => "record_recovery",
    }
  }

  pub fn is_cache_friendly(&self) -> bool {
    matches!(
      self,
      HybridStage::RecordSynthesis
        | HybridStage::RecordMain
        | HybridStage::RecordReview
        | HybridStage::GeneralSynthesis
    )
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct StaticPrefixId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticPrefixSpec {
  pub id: String,
  pub model_id: String,
  pub stage_name: String,
  pub text: String,
  pub content_hash: u64,
  pub estimated_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageCachePlan {
  pub cache_requested: bool,
  pub cache_loaded: bool,
  pub cache_applied: bool,
  pub synthetic_cache_requested: bool,
  pub synthetic_cache_supported: bool,
  pub synthetic_cache_applied: bool,
  pub cache_mode_requested: String,
  pub cache_mode_applied: String,
  pub use_prompt_token_cache: bool,
  pub use_prefix_kv_cache: bool,
  pub use_template_renderer: bool,
  pub use_synthetic_token_cache: bool,
  pub max_candidate_tokens: usize,
  pub prompt_token_cache_hit_ratio: f32,
  pub token_cache_lookup_ms: u128,
  pub expected_accept_tokens_per_verify: f32,
  pub draft_provider: String,
  pub bypass_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedStagePrompt {
  pub stage: HybridStage,
  pub model_id: String,
  pub static_prefix: StaticPrefixSpec,
  pub static_prefix_previous_hash: Option<u64>,
  pub system_segments: Vec<PromptSegment>,
  pub user_segments: Vec<PromptSegment>,
  pub system_segment_order: Vec<String>,
  pub user_segment_order: Vec<String>,
  pub rendered_system: String,
  pub rendered_user: String,
  pub plan: StageCachePlan,
}

pub struct DraceCacheManager {
  capabilities: Mutex<BackendCapabilities>,
  config: DraceCacheConfig,
  stats: Mutex<DraceCacheStats>,
  static_prefixes: Mutex<HashMap<String, StaticPrefixSpec>>,
  prompt_token_cache: Mutex<HashMap<PromptTokenCacheKey, PromptTokenCacheValue>>,
  warmed_prefixes: Mutex<HashSet<String>>,
}

static DRACE_CACHE_MANAGER: OnceLock<DraceCacheManager> = OnceLock::new();
const DRACE_PERSISTENT_STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistentDraceCacheState {
  version: u32,
  static_prefixes: Vec<StaticPrefixSpec>,
  warmed_prefixes: Vec<String>,
}

fn estimate_tokens(text: &str) -> usize {
  text.chars().count().saturating_div(4).max(1)
}

fn drace_persistent_cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
  let dir = app
    .path()
    .app_data_dir()
    .map_err(|e| format!("DRaCE 캐시 폴더를 준비하지 못했어요: {e}"))?
    .join("drace");
  fs::create_dir_all(&dir).map_err(|e| format!("DRaCE 캐시 폴더를 만들지 못했어요: {e}"))?;
  Ok(dir)
}

fn drace_persistent_cache_state_path(app: &AppHandle) -> Result<PathBuf, String> {
  Ok(drace_persistent_cache_dir(app)?.join("persistent-cache-state.json"))
}

impl DraceCacheManager {
  pub fn global() -> &'static Self {
    DRACE_CACHE_MANAGER.get_or_init(|| Self::new_cli())
  }

  pub fn new_cli() -> Self {
    Self {
      capabilities: Mutex::new(BackendCapabilities {
        backend_kind: BackendKind::CliSidecar,
        supports_resident: false,
        supports_prefix_kv_cache: false,
        supports_prompt_cache: false,
        supports_token_verification: false,
        supports_batch_verification: false,
        supports_speculative_decode: false,
        supports_prompt_token_cache: false,
        supports_synthetic_token_cache: false,
        supports_mmap_cache_pack: false,
        supports_resident_model: false,
      }),
      config: DraceCacheConfig { enabled: true },
      stats: Mutex::new(DraceCacheStats::default()),
      static_prefixes: Mutex::new(HashMap::new()),
      prompt_token_cache: Mutex::new(HashMap::new()),
      warmed_prefixes: Mutex::new(HashSet::new()),
    }
  }

  pub fn capabilities_snapshot(&self) -> BackendCapabilities {
    self
      .capabilities
      .lock()
      .map(|capabilities| capabilities.clone())
      .unwrap_or_else(|_| BackendCapabilities {
        backend_kind: BackendKind::CliSidecar,
        supports_resident: false,
        supports_prefix_kv_cache: false,
        supports_prompt_cache: false,
        supports_token_verification: false,
        supports_batch_verification: false,
        supports_speculative_decode: false,
        supports_prompt_token_cache: false,
        supports_synthetic_token_cache: false,
        supports_mmap_cache_pack: false,
        supports_resident_model: false,
      })
  }

  pub fn configure_backend(&self, capabilities: BackendCapabilities) {
    if let Ok(mut slot) = self.capabilities.lock() {
      *slot = capabilities;
    }
  }

  pub fn config(&self) -> &DraceCacheConfig {
    &self.config
  }

  pub fn stats_snapshot(&self) -> DraceCacheStats {
    self.stats.lock().map(|stats| stats.clone()).unwrap_or_default()
  }

  pub fn prompt_token_cache_size(&self) -> usize {
    self.prompt_token_cache.lock().map(|cache| cache.len()).unwrap_or(0)
  }

  pub fn warmed_prefix_key(&self, model_id: &str, prefix_id: &str, slot: Option<u32>, content_hash: Option<u64>) -> String {
    format!(
      "{}::{}::{}::{:016x}",
      model_id.trim(),
      prefix_id.trim(),
      slot.map(|value| value.to_string()).unwrap_or_else(|| "noslot".to_string()),
      content_hash.unwrap_or_default()
    )
  }

  pub fn is_prefix_warm(&self, model_id: &str, prefix_id: &str, slot: Option<u32>, content_hash: Option<u64>) -> bool {
    let key = self.warmed_prefix_key(model_id, prefix_id, slot, content_hash);
    self
      .warmed_prefixes
      .lock()
      .map(|registry| registry.contains(&key))
      .unwrap_or(false)
  }

  pub fn mark_prefix_warm(
    &self,
    model_id: &str,
    prefix_id: &str,
    slot: Option<u32>,
    content_hash: Option<u64>,
    estimated_tokens: usize,
  ) {
    let key = self.warmed_prefix_key(model_id, prefix_id, slot, content_hash);
    let mut warmed_count = 0_u64;
    if let Ok(mut registry) = self.warmed_prefixes.lock() {
      registry.insert(key);
      warmed_count = registry.len() as u64;
    }
    if let Ok(mut stats) = self.stats.lock() {
      stats.warmed_prefix_count = warmed_count;
      stats.prefix_tokens_total = stats.prefix_tokens_total.saturating_add(estimated_tokens as u64);
    }
  }

  pub fn static_prefixes_for_model(&self, model_id: &str) -> Vec<StaticPrefixSpec> {
    self
      .static_prefixes
      .lock()
      .map(|registry| {
        registry
          .values()
          .filter(|spec| spec.model_id == model_id.trim())
          .cloned()
          .collect::<Vec<_>>()
      })
      .unwrap_or_default()
  }

  pub fn load_persistent_state(&self, app: &AppHandle) -> Result<(), String> {
    let path = drace_persistent_cache_state_path(app)?;
    if !path.exists() {
      return Ok(());
    }
    let raw = fs::read_to_string(&path)
      .map_err(|e| format!("DRaCE 영속 캐시를 읽지 못했어요: {e}"))?;
    let parsed: PersistentDraceCacheState = serde_json::from_str(&raw)
      .map_err(|e| format!("DRaCE 영속 캐시를 해석하지 못했어요: {e}"))?;
    if parsed.version != DRACE_PERSISTENT_STATE_VERSION {
      return Ok(());
    }
    if let Ok(mut registry) = self.static_prefixes.lock() {
      registry.clear();
      for spec in parsed.static_prefixes {
        registry.insert(spec.id.clone(), spec);
      }
    }
    let _persisted_warm_keys = parsed.warmed_prefixes;
    if let Ok(mut warmed) = self.warmed_prefixes.lock() {
      warmed.clear();
    }
    if let Ok(mut stats) = self.stats.lock() {
      stats.warmed_prefix_count = 0;
    }
    Ok(())
  }

  pub fn persist_persistent_state(&self, app: &AppHandle) -> Result<(), String> {
    let path = drace_persistent_cache_state_path(app)?;
    let static_prefixes = self
      .static_prefixes
      .lock()
      .map(|registry| registry.values().cloned().collect::<Vec<_>>())
      .unwrap_or_default();
    let warmed_prefixes = self
      .warmed_prefixes
      .lock()
      .map(|registry| registry.iter().cloned().collect::<Vec<_>>())
      .unwrap_or_default();
    let payload = PersistentDraceCacheState {
      version: DRACE_PERSISTENT_STATE_VERSION,
      static_prefixes,
      warmed_prefixes,
    };
    let raw = serde_json::to_string_pretty(&payload)
      .map_err(|e| format!("DRaCE 영속 캐시를 직렬화하지 못했어요: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("DRaCE 영속 캐시를 저장하지 못했어요: {e}"))?;
    Ok(())
  }

  pub fn ensure_static_prefix(
    &self,
    model_id: &str,
    stage: HybridStage,
    segments: &[PromptSegment],
  ) -> (StaticPrefixSpec, Option<u64>) {
    let registry_key = format!("{}::{}", model_id.trim(), stage.as_str());
    let text = segments
      .iter()
      .filter(|segment| segment.is_static)
      .map(|segment| segment.text.trim())
      .filter(|text| !text.is_empty())
      .collect::<Vec<_>>()
      .join("\n\n");
    let content_hash = fast_hash64(&text);
    let estimated_tokens = estimate_tokens(&text);
    if let Ok(mut registry) = self.static_prefixes.lock() {
      if let Some(existing) = registry.get(&registry_key) {
        if existing.content_hash == content_hash {
          return (existing.clone(), None);
        }
        let previous_hash = existing.content_hash;
        let spec = StaticPrefixSpec {
          id: registry_key.clone(),
          model_id: model_id.trim().to_string(),
          stage_name: stage.as_str().to_string(),
          content_hash,
          estimated_tokens,
          text,
        };
        registry.insert(registry_key, spec.clone());
        return (spec, Some(previous_hash));
      }
      let spec = StaticPrefixSpec {
        id: registry_key.clone(),
        model_id: model_id.trim().to_string(),
        stage_name: stage.as_str().to_string(),
        content_hash,
        estimated_tokens,
        text,
      };
      registry.insert(registry_key, spec.clone());
      return (spec, None);
    }
    (
      StaticPrefixSpec {
        id: registry_key,
        model_id: model_id.trim().to_string(),
        stage_name: stage.as_str().to_string(),
        content_hash,
        estimated_tokens,
        text,
      },
      None,
    )
  }

  pub fn plan_stage(
    &self,
    cache_requested: bool,
    stage: HybridStage,
    static_prefix_tokens: usize,
    expected_output_tokens: usize,
  ) -> StageCachePlan {
    let capabilities = self.capabilities_snapshot();
    let synthetic_cache_requested = cache_requested;
    let synthetic_cache_supported =
      capabilities.supports_token_verification && capabilities.supports_synthetic_token_cache;
    let mut bypass_reason = if !cache_requested {
      "cache disabled".to_string()
    } else if matches!(capabilities.backend_kind, BackendKind::CliSidecar) {
      "unsupported_backend_cli".to_string()
    } else if !capabilities.supports_prefix_kv_cache
      && !capabilities.supports_token_verification
      && !capabilities.supports_prompt_token_cache
      && !capabilities.supports_synthetic_token_cache
    {
      "unsupported_backend".to_string()
    } else if expected_output_tokens > 0 && expected_output_tokens < 80 {
      "short output".to_string()
    } else if !stage.is_cache_friendly() {
      "stage not cache-friendly".to_string()
    } else {
      String::new()
    };

    let use_prompt_token_cache = cache_requested
      && capabilities.supports_prompt_token_cache
      && !matches!(capabilities.backend_kind, BackendKind::CliSidecar)
      && static_prefix_tokens >= 128
      && bypass_reason.is_empty();
    let use_prefix_kv_cache = cache_requested
      && capabilities.supports_prefix_kv_cache
      && capabilities.supports_resident_model
      && stage.is_cache_friendly()
      && bypass_reason.is_empty();
    let use_template_renderer = cache_requested
      && use_prefix_kv_cache
      && matches!(
        stage,
        HybridStage::RecordSynthesis | HybridStage::RecordReview | HybridStage::GeneralSynthesis
      )
      && bypass_reason.is_empty();
    let use_synthetic_token_cache = cache_requested
      && synthetic_cache_supported
      && matches!(stage, HybridStage::RecordReview)
      && expected_output_tokens >= 80
      && bypass_reason.is_empty();

    if bypass_reason.is_empty()
      && !(use_prompt_token_cache || use_prefix_kv_cache || use_synthetic_token_cache || use_template_renderer)
    {
      bypass_reason = "no beneficial cache path".to_string();
    }

    if cache_requested && !synthetic_cache_supported && (use_prefix_kv_cache || use_template_renderer) {
      bypass_reason = if use_template_renderer {
        "synthetic_token_cache_verification=unsupported; fallback=PrefixKV+TemplateRenderer".to_string()
      } else {
        "synthetic_token_cache_verification=unsupported; fallback=PrefixKV".to_string()
      };
    }

    let cache_applied = use_prompt_token_cache || use_prefix_kv_cache || use_synthetic_token_cache || use_template_renderer;
    let prompt_token_cache_loaded = use_prompt_token_cache && capabilities.supports_prompt_token_cache;
    let prefix_kv_loaded = use_prefix_kv_cache && capabilities.supports_prefix_kv_cache && capabilities.supports_resident_model;
    let synthetic_token_cache_loaded =
      use_synthetic_token_cache && capabilities.supports_synthetic_token_cache && capabilities.supports_mmap_cache_pack;
    let cache_loaded = prompt_token_cache_loaded || prefix_kv_loaded || synthetic_token_cache_loaded || use_template_renderer;
    let cache_mode_applied = if use_synthetic_token_cache {
      "FullDRACE".to_string()
    } else if use_prefix_kv_cache && use_template_renderer {
      "PrefixKV+TemplateRenderer".to_string()
    } else if use_prefix_kv_cache {
      "PrefixKV".to_string()
    } else if use_prompt_token_cache {
      "PrefixKV".to_string()
    } else {
      "Off".to_string()
    };

    StageCachePlan {
      cache_requested,
      cache_loaded,
      cache_applied,
      synthetic_cache_requested,
      synthetic_cache_supported,
      synthetic_cache_applied: use_synthetic_token_cache,
      cache_mode_requested: if cache_requested { "FullDRACE".to_string() } else { "Off".to_string() },
      cache_mode_applied,
      use_prompt_token_cache,
      use_prefix_kv_cache,
      use_template_renderer,
      use_synthetic_token_cache,
      max_candidate_tokens: if use_synthetic_token_cache { 16 } else { 0 },
      prompt_token_cache_hit_ratio: 0.0,
      token_cache_lookup_ms: 0,
      expected_accept_tokens_per_verify: 0.0,
      draft_provider: if use_template_renderer || use_synthetic_token_cache {
        "template".to_string()
      } else {
        "noop".to_string()
      },
      bypass_reason,
    }
  }

  pub fn prepare_prompt(
    &self,
    model_id: &str,
    stage: HybridStage,
    cache_requested: bool,
    mut system_segments: Vec<PromptSegment>,
    mut user_segments: Vec<PromptSegment>,
    expected_output_tokens: usize,
  ) -> PreparedStagePrompt {
    system_segments.sort_by_key(|segment| segment.kind.order_rank());
    user_segments.sort_by_key(|segment| segment.kind.order_rank());
    let system_segment_order = system_segments
      .iter()
      .map(|segment| segment.kind.label().to_string())
      .collect::<Vec<_>>();
    let user_segment_order = user_segments
      .iter()
      .map(|segment| segment.kind.label().to_string())
      .collect::<Vec<_>>();
    let (static_prefix, static_prefix_previous_hash) = self.ensure_static_prefix(model_id, stage, &system_segments);
    let rendered_system = system_segments
      .iter()
      .map(|segment| segment.text.trim())
      .filter(|text| !text.is_empty())
      .collect::<Vec<_>>()
      .join("\n\n");
    let rendered_user = user_segments
      .iter()
      .map(|segment| segment.text.trim())
      .filter(|text| !text.is_empty())
      .collect::<Vec<_>>()
      .join("\n\n");
    let plan = self.plan_stage(cache_requested, stage, static_prefix.estimated_tokens, expected_output_tokens);
    PreparedStagePrompt {
      stage,
      model_id: model_id.to_string(),
      static_prefix,
      static_prefix_previous_hash,
      system_segments,
      user_segments,
      system_segment_order,
      user_segment_order,
      rendered_system,
      rendered_user,
      plan,
    }
  }
}
