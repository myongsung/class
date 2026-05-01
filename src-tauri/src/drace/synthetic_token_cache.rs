use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

const SYNTHETIC_TOKEN_CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct SyntheticTokenCachePrepared {
  pub exact_key: String,
  pub semantic_key: String,
  pub prefix_cache_path: PathBuf,
  pub prompt_chars: usize,
  pub model_id: String,
  pub stage_label: String,
  pub n_ctx: u32,
  pub max_tokens: u32,
  pub system_prompt_hash: String,
  pub user_prompt_hash: String,
  pub prefix_hash: String,
  pub suffix_hash: String,
}

fn hash_text(value: &str) -> String {
  let mut hasher = Sha256::new();
  hasher.update(value.as_bytes());
  format!("{:x}", hasher.finalize())
}

fn windowed_hash(value: &str, take_from_start: bool) -> String {
  let chars = value.chars().collect::<Vec<_>>();
  let limit = 1200usize.min(chars.len());
  let slice = if take_from_start {
    chars.into_iter().take(limit).collect::<String>()
  } else {
    chars
      .into_iter()
      .rev()
      .take(limit)
      .collect::<Vec<_>>()
      .into_iter()
      .rev()
      .collect::<String>()
  };
  hash_text(&slice)
}

fn synthetic_token_cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
  let dir = app
    .path()
    .app_data_dir()
    .map_err(|e| format!("DRaCE 캐시 폴더를 준비하지 못했어요: {e}"))?
    .join("drace")
    .join("synthetic-token-cache");
  fs::create_dir_all(&dir).map_err(|e| format!("DRaCE 캐시 폴더를 만들지 못했어요: {e}"))?;
  Ok(dir)
}

fn extract_prompt_block(prompt: &str, title: &str) -> Option<String> {
  let marker = format!("[{}]", title.trim());
  let start = prompt.find(&marker)?;
  let after = &prompt[(start + marker.len())..];
  let mut lines = Vec::<String>::new();
  for line in after.lines() {
    let trimmed = line.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
      break;
    }
    lines.push(line.to_string());
  }
  let block = lines.join("\n").trim().to_string();
  if block.is_empty() { None } else { Some(block) }
}

fn normalize_prompt_block(value: &str) -> String {
  value
    .lines()
    .map(|line| line.trim())
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

fn build_semantic_prompt_signature(stage_label: &str, user_prompt: &str) -> String {
  let stage = stage_label.trim();
  let mut parts = vec![format!("stage={stage}")];

  let record_titles = [
    "사용자 입력 상황",
    "사건 맥락",
    "참고 인물",
    "참고 시간 흐름",
    "참고 기록",
    "관련 법령/참고 기준",
    "확인이 더 필요한 부분",
    "추가 확인 필요",
    "전략 메모",
    "HyperCLOVA-X 기록 골격",
    "HyperCLOVA-X 초안",
    "Roosy-X 초안",
    "현재 기록 초안",
  ];
  let analysis_titles = [
    "현재 사건 맥락",
    "증거 패킷 요약",
    "핵심 인물",
    "시간 흐름",
    "비어 있는 정보",
    "증거 참조표",
    "관련 법령 참조표",
    "전략 메모",
    "질문",
    "이번 요청",
    "질문 초점",
    "핵심 근거",
    "관련 법령",
    "HyperCLOVA-X 초안",
    "Roosy-X 초안",
  ];

  let titles = if user_prompt.contains("[사용자 입력 상황]") {
    &record_titles[..]
  } else {
    &analysis_titles[..]
  };

  for title in titles {
    if let Some(block) = extract_prompt_block(user_prompt, title) {
      parts.push(format!("{}={}", title, normalize_prompt_block(&block)));
    }
  }

  if parts.len() == 1 {
    parts.push(normalize_prompt_block(user_prompt));
  }

  parts.join("\n\n")
}

pub fn prepare_synthetic_token_cache(
  app: &AppHandle,
  model_id: &str,
  stage_label: &str,
  system_prompt: &str,
  user_prompt: &str,
  n_ctx: u32,
  max_tokens: u32,
) -> Result<SyntheticTokenCachePrepared, String> {
  let system_prompt_hash = hash_text(system_prompt);
  let user_prompt_hash = hash_text(user_prompt);
  let semantic_prompt_signature = build_semantic_prompt_signature(stage_label, user_prompt);
  let semantic_prompt_hash = hash_text(&semantic_prompt_signature);
  let prefix_hash = windowed_hash(user_prompt, true);
  let suffix_hash = windowed_hash(user_prompt, false);
  let exact_key = hash_text(
    format!(
      "v{}|{}|{}|{}|{}|{}|{}",
      SYNTHETIC_TOKEN_CACHE_VERSION,
      model_id.trim(),
      stage_label.trim(),
      n_ctx,
      max_tokens,
      system_prompt_hash,
      user_prompt_hash
    )
    .as_str(),
  );
  let semantic_key = hash_text(
    format!(
      "v{}|{}|{}|{}|{}|{}|{}",
      SYNTHETIC_TOKEN_CACHE_VERSION,
      model_id.trim(),
      stage_label.trim(),
      n_ctx,
      max_tokens,
      system_prompt_hash,
      semantic_prompt_hash
    )
    .as_str(),
  );
  let dir = synthetic_token_cache_dir(app)?;
  let prefix_dir = dir.join("prefix-kv");
  fs::create_dir_all(&prefix_dir).map_err(|e| format!("DRaCE Prefix KV 캐시 폴더를 만들지 못했어요: {e}"))?;
  Ok(SyntheticTokenCachePrepared {
    exact_key: exact_key.clone(),
    semantic_key: semantic_key.clone(),
    prefix_cache_path: prefix_dir.join(format!("prefix-{}.bin", semantic_key)),
    prompt_chars: system_prompt.chars().count() + user_prompt.chars().count(),
    model_id: model_id.trim().to_string(),
    stage_label: stage_label.trim().to_string(),
    n_ctx,
    max_tokens,
    system_prompt_hash,
    user_prompt_hash,
    prefix_hash,
    suffix_hash,
  })
}
