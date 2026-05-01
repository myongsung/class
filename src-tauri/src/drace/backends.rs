use super::cache_manager::{BackendCapabilities, BackendKind};
use super::native_backend::native_verifier_available;
use serde::{Deserialize, Serialize};
use std::env;
use std::time::Instant;

const DEFAULT_LLAMA_SERVER_HYPERCLOVA_URL: &str = "http://127.0.0.1:18081/completion";
const DEFAULT_LLAMA_SERVER_ROOSY_URL: &str = "http://127.0.0.1:18082/completion";

fn is_loopback_completion_url(value: &str) -> bool {
  let trimmed = value.trim();
  let lower = trimmed.to_ascii_lowercase();
  (lower.starts_with("http://127.0.0.1:") || lower.starts_with("http://localhost:"))
    && lower.ends_with("/completion")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlamaServerConfig {
  pub hyperclova_url: String,
  pub roosy_url: String,
  pub cache_prompt: bool,
  pub startup_timeout_ms: u64,
  pub request_timeout_ms: u64,
  pub hyperclova_slot: Option<u32>,
  pub roosy_slot: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendRuntime {
  pub capabilities: BackendCapabilities,
  pub llama_server: Option<LlamaServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationSessionOptions {
  pub model_id: String,
  pub endpoint: String,
  pub model_path: Option<String>,
  pub slot: Option<u32>,
  pub cache_prompt: bool,
  pub assistant_prefix: Option<String>,
  pub n_ctx: Option<u32>,
  pub threads: Option<u32>,
  pub max_tokens: u32,
  pub temperature: f32,
  pub top_p: f32,
  pub repeat_penalty: f32,
  pub request_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationSessionHandle {
  pub session_id: String,
  pub backend_kind: BackendKind,
  pub model_id: String,
  pub prompt: String,
  pub options: GenerationSessionOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationStepResult {
  pub raw_response: serde_json::Value,
  pub response_started_ms: u128,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftVerifyResult {
  pub accepted_len: usize,
  pub accepted_tokens: Vec<u32>,
  pub accepted_text: String,
  pub rejected_at: Option<usize>,
  pub verify_ms: u128,
  pub correction_token_id: Option<u32>,
}

pub trait GenerationSessionApi {
  fn open_session(&self, prompt: &str, options: GenerationSessionOptions) -> Result<GenerationSessionHandle, String>;
  fn generate_next(&self, session: &GenerationSessionHandle) -> Result<GenerationStepResult, String>;
  fn verify_draft(
    &self,
    session: &GenerationSessionHandle,
    proposed_token_ids: &[u32],
  ) -> Result<DraftVerifyResult, String>;
  fn close_session(&self, session: &GenerationSessionHandle) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub struct LlamaServerSessionBackend;

impl GenerationSessionApi for LlamaServerSessionBackend {
  fn open_session(&self, prompt: &str, options: GenerationSessionOptions) -> Result<GenerationSessionHandle, String> {
    Ok(GenerationSessionHandle {
      session_id: format!("llama-session-{}-{}", options.model_id, fast_session_nonce()),
      backend_kind: BackendKind::LlamaServer,
      model_id: options.model_id.clone(),
      prompt: prompt.to_string(),
      options,
    })
  }

  fn generate_next(&self, session: &GenerationSessionHandle) -> Result<GenerationStepResult, String> {
    let full_prompt = llama_server_full_prompt(session);
    let request_body = serde_json::json!({
      "prompt": full_prompt,
      "n_predict": session.options.max_tokens,
      "temperature": session.options.temperature,
      "top_p": session.options.top_p,
      "repeat_penalty": session.options.repeat_penalty,
      "cache_prompt": session.options.cache_prompt,
      "id_slot": if session.options.cache_prompt { session.options.slot } else { None },
      "stream": false,
    });

    let client = reqwest::blocking::Client::builder()
      .timeout(std::time::Duration::from_millis(session.options.request_timeout_ms))
      .build()
      .map_err(|e| format!("llama-server 세션 클라이언트를 만들지 못했어요: {e}"))?;
    let request_started = Instant::now();
    let response = client
      .post(session.options.endpoint.as_str())
      .json(&request_body)
      .send()
      .map_err(|e| format!("llama-server 요청에 실패했어요: {e}"))?;
    let response_started_ms = request_started.elapsed().as_millis();
    let status = response.status();
    let response_text = response
      .text()
      .map_err(|e| format!("llama-server 응답 본문을 읽지 못했어요: {e}"))?;
    let parsed: serde_json::Value = serde_json::from_str(&response_text)
      .map_err(|e| format!("llama-server 응답을 JSON으로 해석하지 못했어요: {e} / body={}", trim_backend_debug(&response_text, 300)))?;

    if !status.is_success() {
      return Err(format!(
        "llama-server 응답 오류: HTTP {} / {}",
        status,
        extract_backend_error(&parsed).unwrap_or_else(|| trim_backend_debug(&response_text, 260))
      ));
    }

    Ok(GenerationStepResult {
      raw_response: parsed,
      response_started_ms,
    })
  }

  fn verify_draft(
    &self,
    session: &GenerationSessionHandle,
    proposed_token_ids: &[u32],
  ) -> Result<DraftVerifyResult, String> {
    if proposed_token_ids.is_empty() {
      return Ok(DraftVerifyResult::default());
    }

    let candidate_text = token_ids_to_text(proposed_token_ids);
    if candidate_text.trim().is_empty() {
      return Ok(DraftVerifyResult::default());
    }

    let verify_prompt = llama_server_full_prompt(session);
    let verify_len = candidate_text.chars().count().clamp(1, 24) as u32;
    let request_body = serde_json::json!({
      "prompt": verify_prompt,
      "n_predict": verify_len,
      "temperature": 0.0,
      "top_p": 1.0,
      "repeat_penalty": 1.0,
      "cache_prompt": session.options.cache_prompt,
      "id_slot": if session.options.cache_prompt { session.options.slot } else { None },
      "stream": false,
    });

    let client = reqwest::blocking::Client::builder()
      .timeout(std::time::Duration::from_millis(session.options.request_timeout_ms))
      .build()
      .map_err(|e| format!("llama-server 검증 클라이언트를 만들지 못했어요: {e}"))?;
    let verify_started = Instant::now();
    let response = client
      .post(session.options.endpoint.as_str())
      .json(&request_body)
      .send()
      .map_err(|e| format!("llama-server 검증 요청에 실패했어요: {e}"))?;
    let verify_ms = verify_started.elapsed().as_millis();
    let status = response.status();
    let response_text = response
      .text()
      .map_err(|e| format!("llama-server 검증 응답 본문을 읽지 못했어요: {e}"))?;
    let parsed: serde_json::Value = serde_json::from_str(&response_text)
      .map_err(|e| format!("llama-server 검증 응답을 JSON으로 해석하지 못했어요: {e} / body={}", trim_backend_debug(&response_text, 300)))?;
    if !status.is_success() {
      return Err(format!(
        "llama-server 검증 응답 오류: HTTP {} / {}",
        status,
        extract_backend_error(&parsed).unwrap_or_else(|| trim_backend_debug(&response_text, 260))
      ));
    }

    let actual_text = extract_backend_answer(&parsed).unwrap_or_default();
    let candidate_chars = candidate_text.chars().collect::<Vec<_>>();
    let actual_chars = actual_text.chars().collect::<Vec<_>>();
    let mut accepted_len = 0usize;
    while accepted_len < candidate_chars.len()
      && accepted_len < actual_chars.len()
      && candidate_chars[accepted_len] == actual_chars[accepted_len]
    {
      accepted_len += 1;
    }
    let accepted_tokens = candidate_chars
      .iter()
      .take(accepted_len)
      .map(|ch| *ch as u32)
      .collect::<Vec<_>>();
    let correction_token_id = actual_chars.get(accepted_len).copied().map(|ch| ch as u32);

    Ok(DraftVerifyResult {
      accepted_len,
      accepted_tokens,
      accepted_text: candidate_chars.iter().take(accepted_len).collect::<String>(),
      rejected_at: if accepted_len < candidate_chars.len() {
        Some(accepted_len)
      } else {
        None
      },
      verify_ms,
      correction_token_id,
    })
  }

  fn close_session(&self, _session: &GenerationSessionHandle) -> Result<(), String> {
    Ok(())
  }
}

fn fast_session_nonce() -> u128 {
  use std::time::{SystemTime, UNIX_EPOCH};
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|value| value.as_millis())
    .unwrap_or_default()
}

fn token_ids_to_text(token_ids: &[u32]) -> String {
  token_ids
    .iter()
    .filter_map(|id| char::from_u32(*id))
    .collect::<String>()
}

fn llama_server_full_prompt(session: &GenerationSessionHandle) -> String {
  let mut prompt = session.prompt.clone();
  if let Some(prefix) = session.options.assistant_prefix.as_deref() {
    if !prefix.is_empty() {
      prompt.push_str(prefix);
    }
  }
  prompt
}

fn extract_backend_answer(root: &serde_json::Value) -> Option<String> {
  for key in ["content", "response", "text", "completion"] {
    if let Some(value) = value_string(root.get(key)) {
      return Some(value);
    }
  }

  let choices = root.get("choices")?.as_array()?;
  for choice in choices {
    for key in ["content", "text", "completion"] {
      if let Some(value) = value_string(choice.get(key)) {
        return Some(value);
      }
    }
    if let Some(value) = value_string(choice.get("message").and_then(|message| message.get("content"))) {
      return Some(value);
    }
  }

  None
}

fn value_string(value: Option<&serde_json::Value>) -> Option<String> {
  match value {
    Some(serde_json::Value::String(text)) => {
      let trimmed = text.trim();
      if trimmed.is_empty() {
        None
      } else {
        Some(trimmed.to_string())
      }
    }
    _ => None,
  }
}

fn trim_backend_debug(input: &str, max_chars: usize) -> String {
  let normalized = input.replace('\r', "").replace('\n', " ").trim().to_string();
  if normalized.chars().count() <= max_chars {
    normalized
  } else {
    let mut out = String::with_capacity(max_chars + 1);
    for ch in normalized.chars().take(max_chars) {
      out.push(ch);
    }
    out.push('…');
    out
  }
}

fn extract_backend_error(value: &serde_json::Value) -> Option<String> {
  [
    value.get("error").and_then(|item| item.as_str()),
    value.get("message").and_then(|item| item.as_str()),
    value
      .get("choices")
      .and_then(|item| item.as_array())
      .and_then(|items| items.first())
      .and_then(|item| item.get("message"))
      .and_then(|item| item.get("content"))
      .and_then(|item| item.as_str()),
  ]
  .into_iter()
  .flatten()
  .map(|item| item.trim().to_string())
  .find(|item| !item.is_empty())
}

fn env_flag(name: &str) -> bool {
  matches!(
    env::var(name).ok().as_deref(),
    Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("on")
  )
}

fn env_u64(name: &str, default: u64) -> u64 {
  env::var(name)
    .ok()
    .and_then(|value| value.trim().parse::<u64>().ok())
    .unwrap_or(default)
}

fn env_u32_opt(name: &str) -> Option<u32> {
  env::var(name).ok().and_then(|value| value.trim().parse::<u32>().ok())
}

pub fn detect_backend_runtime(
  explicit_backend_override: Option<&str>,
  llama_server_override: Option<LlamaServerConfig>,
) -> BackendRuntime {
  let explicit_backend = explicit_backend_override
    .map(|value| value.trim().to_ascii_lowercase())
    .filter(|value| !value.is_empty())
    .or_else(|| {
      env::var("ROOSYCOZY_DRACE_BACKEND")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
    });

  let hyperclova_url = llama_server_override
    .as_ref()
    .map(|value| value.hyperclova_url.trim().to_string())
    .filter(|value| !value.is_empty())
    .or_else(|| {
      env::var("ROOSYCOZY_LLAMA_SERVER_HYPERCLOVA_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    });
  let roosy_url = llama_server_override
    .as_ref()
    .map(|value| value.roosy_url.trim().to_string())
    .filter(|value| !value.is_empty())
    .or_else(|| {
      env::var("ROOSYCOZY_LLAMA_SERVER_ROOSY_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    });

  let wants_native = matches!(explicit_backend.as_deref(), Some("native"));
  let native_verifier_disabled = env_flag("ROOSYCOZY_DISABLE_NATIVE_VERIFIER");
  let native_verifier_opt_in = !native_verifier_disabled
    && (env_flag("ROOSYCOZY_ENABLE_NATIVE_VERIFIER")
      || env_flag("ROOSYCOZY_EXPERIMENTAL_NATIVE_BACKEND"));
  let native_verification_supported = native_verifier_opt_in && native_verifier_available().is_ok();

  if wants_native {
    return BackendRuntime {
      capabilities: BackendCapabilities {
        backend_kind: BackendKind::Native,
        supports_resident: native_verification_supported,
        supports_prompt_token_cache: false,
        supports_prompt_cache: false,
        supports_prefix_kv_cache: false,
        supports_token_verification: native_verification_supported,
        supports_batch_verification: native_verification_supported,
        supports_speculative_decode: false,
        supports_synthetic_token_cache: native_verification_supported,
        supports_mmap_cache_pack: false,
        supports_resident_model: native_verification_supported,
      },
      llama_server: None,
    };
  }

  let llama_server = if let Some(mut override_config) = llama_server_override {
    override_config.hyperclova_url =
      hyperclova_url.unwrap_or_else(|| DEFAULT_LLAMA_SERVER_HYPERCLOVA_URL.to_string());
    override_config.roosy_url =
      roosy_url.unwrap_or_else(|| DEFAULT_LLAMA_SERVER_ROOSY_URL.to_string());
    override_config.startup_timeout_ms = override_config.startup_timeout_ms.max(90_000);
    override_config.request_timeout_ms = override_config.request_timeout_ms.max(240_000);
    override_config
  } else {
    LlamaServerConfig {
      hyperclova_url: hyperclova_url.unwrap_or_else(|| DEFAULT_LLAMA_SERVER_HYPERCLOVA_URL.to_string()),
      roosy_url: roosy_url.unwrap_or_else(|| DEFAULT_LLAMA_SERVER_ROOSY_URL.to_string()),
      cache_prompt: !env_flag("ROOSYCOZY_LLAMA_SERVER_DISABLE_CACHE_PROMPT"),
      startup_timeout_ms: env_u64("ROOSYCOZY_LLAMA_SERVER_STARTUP_TIMEOUT_MS", 90_000),
      request_timeout_ms: env_u64("ROOSYCOZY_LLAMA_SERVER_REQUEST_TIMEOUT_MS", 240_000),
      hyperclova_slot: env_u32_opt("ROOSYCOZY_LLAMA_SERVER_HYPERCLOVA_SLOT"),
      roosy_slot: env_u32_opt("ROOSYCOZY_LLAMA_SERVER_ROOSY_SLOT"),
    }
  };

  BackendRuntime {
    capabilities: BackendCapabilities {
      backend_kind: BackendKind::LlamaServer,
      supports_resident: true,
      supports_prompt_token_cache: false,
      supports_prompt_cache: true,
      supports_prefix_kv_cache: true,
      supports_token_verification: true,
      supports_batch_verification: true,
      supports_speculative_decode: false,
      supports_synthetic_token_cache: true,
      supports_mmap_cache_pack: false,
      supports_resident_model: true,
    },
    llama_server: Some(llama_server),
  }
}
