use crate::drace::{
  detect_backend_runtime, render_prompt_segments, BackendCapabilities, BackendKind, DraceCacheManager, DraftProposal,
  DraftProvider, DraftProviderState, GenerationSessionApi, GenerationSessionOptions, HybridStage, LlamaServerConfig,
  LlamaServerSessionBackend, NativeSessionBackend, NoopDraftProvider, PromptSegment, PromptSegmentKind,
  TemplateDraftProvider,
};
use chrono::Local;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH}; // 시간 처리를 위한 표준 라이브러리 추가
use tauri::{path::BaseDirectory, AppHandle, Emitter, Manager};

static STRATEGY_LEGAL_DATASET: OnceLock<StrategyLegalDataset> = OnceLock::new();
static STRATEGY_LEGAL_FLAT_CHUNKS: OnceLock<Vec<StrategyLegalFlatChunk>> = OnceLock::new();
static STRATEGY_RECORD_TEMPLATE_DATASET: OnceLock<StrategyRecordTemplateDataset> = OnceLock::new();
static STRATEGY_DRACE_PERSISTENT_STATE_LOADED: AtomicBool = AtomicBool::new(false);
static STRATEGY_MODEL_DOWNLOAD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static STRATEGY_MODEL_DOWNLOAD_RUNNING: OnceLock<Mutex<bool>> = OnceLock::new();
static STRATEGY_MODEL_DOWNLOAD_LAST_EVENT: OnceLock<Mutex<Option<(String, String, usize, usize)>>> =
  OnceLock::new();
static STRATEGY_LLAMA_SERVER_REGISTRY: OnceLock<Mutex<HashMap<String, StrategyLlamaServerProcess>>> = OnceLock::new();
static STRATEGY_LLAMA_SERVER_ENDPOINTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/* -------------------- tiny helpers -------------------- */

fn norm(s: &str) -> String {
  s.to_lowercase()
    .replace('\u{200B}', "")
    .replace('\u{200C}', "")
    .replace('\u{200D}', "")
    .replace('\u{FEFF}', "")
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ")
}

fn within_range(ts: &str, from: &str, to: &str) -> bool {
  if !from.is_empty() && ts < from {
    return false;
  }
  if !to.is_empty() && ts > to {
    return false;
  }
  true
}

fn is_word_char(cp: u32) -> bool {
  let is_ascii_num = cp >= 0x30 && cp <= 0x39;
  let is_ascii_upper = cp >= 0x41 && cp <= 0x5A;
  let is_ascii_lower = cp >= 0x61 && cp <= 0x7A;
  let is_hangul_syllable = cp >= 0xAC00 && cp <= 0xD7A3;
  let is_hangul_jamo1 = cp >= 0x3131 && cp <= 0x314E;
  let is_hangul_jamo2 = cp >= 0x314F && cp <= 0x3163;
  is_ascii_num || is_ascii_upper || is_ascii_lower || is_hangul_syllable || is_hangul_jamo1 || is_hangul_jamo2
}

fn tokenize(s: &str) -> Vec<String> {
  let mut out: Vec<String> = Vec::new();
  let mut cur = String::new();

  for ch in s.chars() {
    let cp = ch as u32;
    if is_word_char(cp) {
      cur.push(ch);
    } else {
      let t = norm(&cur);
      if t.len() >= 2 {
        out.push(t);
      }
      cur.clear();
    }
  }
  let t = norm(&cur);
  if t.len() >= 2 {
    out.push(t);
  }
  out
}

fn text_similarity_stats(q_tokens: &[String], summary: &str) -> (usize, usize, f32) {
  if q_tokens.is_empty() {
    return (0, 0, 0.0);
  }
  let s = norm(summary);
  let mut hit = 0usize;
  for qt in q_tokens {
    if qt.len() >= 2 && s.contains(qt) {
      hit += 1;
    }
  }
  let total = q_tokens.len();
  let ratio = hit as f32 / total as f32;
  (hit, total, ratio)
}


fn record_main_actor_names(r: &RecordItem) -> Vec<String> {
  let mut out = Vec::<String>::new();
  let mut seen = HashSet::<String>::new();

  for a in &r.actors {
    let n = norm(&a.name);
    if !n.is_empty() && seen.insert(n.clone()) {
      out.push(n);
    }
  }

  let fallback = norm(&r.actor.name);
  if !fallback.is_empty() && seen.insert(fallback.clone()) {
    out.push(fallback);
  }

  out
}

/* -------------------- shared types (proto) -------------------- */

pub type Sensitivity = String;
pub type StoreType = String;
pub type PlaceType = String;
pub type CaseSensFilter = String;
pub type CaseStatus = String;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RecordSummaryParts {
  #[serde(default)]
  pub overview: String,
  #[serde(default)]
  pub background: String,
  #[serde(default)]
  pub issues: String,
  #[serde(default)]
  pub evidence_list: String,
  #[serde(default)]
  pub teacher_actions: String,
  #[serde(default)]
  pub other: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorRef {
  #[serde(rename = "type")]
  pub r#type: String,
  pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordItem {
  pub id: String,
  pub ts: String,
  pub store_type: StoreType,
  pub store_other: String,
  pub lv: Sensitivity,
  pub actor: ActorRef,
  #[serde(default)]
  pub actors: Vec<ActorRef>,
  #[serde(default)]
  pub related: Vec<ActorRef>,
  pub place: PlaceType,
  pub place_other: String,
  pub summary: String,
  #[serde(default)]
  pub summary_parts: Option<RecordSummaryParts>,
  #[serde(default)]
  pub risk: Option<LegacyRecordRisk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseItem {
  pub id: String,
  pub title: String,

  #[serde(default)]
  pub query: String,

  #[serde(default)]
  pub time_from: String,
  #[serde(default)]
  pub time_to: String,

  #[serde(default)]
  pub max_results: Option<u32>,

  #[serde(default)]
  pub actors: Vec<ActorRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankWeights {
  #[serde(default)]
  pub actor: Option<f32>,
  #[serde(default)]
  pub related: Option<f32>,
  #[serde(default)]
  pub text: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankOpts {
  #[serde(default, alias = "limit", alias = "maxResults")]
  pub max_results: Option<u32>,

  #[serde(default)]
  pub weights: Option<RankWeights>,

  #[serde(default, alias = "minScore")]
  pub min_score: Option<f32>,

  #[serde(default, alias = "minTextSim")]
  pub min_text_sim: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RankedComponents {
  pub keyword_score: f32,
  pub text_sim: f32,
  pub q_hit: u32,
  pub q_total: u32,

  pub actor_score: f32,
  pub actor_match: bool,
  #[serde(default)]
  pub actor_hits: u32,
  pub is_main_actor: bool,

  pub related_score: f32,
  pub related_hits: u32,

  pub in_range: Option<bool>,

  pub w_actor: f32,
  pub w_related: f32,
  pub w_text: f32,
  pub min_score: f32,
  pub min_text_sim: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedHit {
  pub id: String,
  pub score: f32,
  pub rank: u32,
  #[serde(default)]
  pub reasons: Vec<String>,
  #[serde(default)]
  pub components: RankedComponents,
}

/* -------------------- core: rank -------------------- */

pub fn rank_records_for_case(
  records: &[RecordItem],
  case_item: &CaseItem,
  opts: Option<RankOpts>,
) -> Vec<RankedHit> {
  let (k, w_actor, w_related, w_text, min_score, min_text_sim) = {
    let k = opts
      .as_ref()
      .and_then(|o| o.max_results)
      .or(case_item.max_results)
      .unwrap_or(80)
      .clamp(1, 400) as usize;

    let w = opts.as_ref().and_then(|o| o.weights.clone());
    let w_actor = w.as_ref().and_then(|x| x.actor).unwrap_or(2.5);
    let w_related = w.as_ref().and_then(|x| x.related).unwrap_or(1.0);
    let w_text = w.as_ref().and_then(|x| x.text).unwrap_or(2.0);

    let min_score = opts.as_ref().and_then(|o| o.min_score).unwrap_or(0.8);
    let min_text_sim = opts.as_ref().and_then(|o| o.min_text_sim).unwrap_or(0.34);

    (k, w_actor, w_related, w_text, min_score, min_text_sim)
  };

  let q = case_item.query.trim();
  let q_tokens = if q.is_empty() { vec![] } else { tokenize(q) };

  let case_actor_names: HashSet<String> = case_item
    .actors
    .iter()
    .map(|a| norm(&a.name))
    .filter(|s| !s.is_empty())
    .collect();

  let main_actor_name = case_item
    .actors
    .get(0)
    .map(|a| norm(&a.name))
    .filter(|s| !s.is_empty());

  let has_range = !case_item.time_from.is_empty() || !case_item.time_to.is_empty();

  #[derive(Clone)]
  struct Tmp {
    id: String,
    score: f32,
    ts: String,
    reasons: Vec<String>,
    components: RankedComponents,
  }

  let mut main_hits: Vec<Tmp> = Vec::new();
  let mut candidates: Vec<Tmp> = Vec::new();

  for r in records {
    let in_range = if has_range {
      within_range(&r.ts, &case_item.time_from, &case_item.time_to)
    } else {
      true
    };
    if has_range && !in_range {
      continue;
    }

    let r_actor_names = record_main_actor_names(r);
    let actor_hits = r_actor_names
      .iter()
      .filter(|name| case_actor_names.contains(*name))
      .count();
    let actor_match_any = actor_hits > 0;

    let is_main_actor = main_actor_name
      .as_ref()
      .map(|m| r_actor_names.iter().any(|name| name == m))
      .unwrap_or(false);

    let main_actor_name_set: HashSet<String> = r_actor_names.iter().cloned().collect();
    let mut related_hits = 0usize;
    for ra in &r.related {
      let rn = norm(&ra.name);
      if !rn.is_empty() && !main_actor_name_set.contains(&rn) && case_actor_names.contains(&rn) {
        related_hits += 1;
      }
    }

    let (q_hit, q_total, sim) = text_similarity_stats(&q_tokens, &r.summary);

    let actor_bonus = if actor_hits > 1 {
      (((actor_hits - 1) as f32) * (w_actor * 0.35)).min(w_actor * 0.75)
    } else {
      0.0
    };
    let actor_score = if actor_match_any { w_actor + actor_bonus } else { 0.0 };
    let related_score = (related_hits as f32) * w_related;
    let keyword_score = sim * w_text;
    let score: f32 = actor_score + related_score + keyword_score;

    let mut reasons: Vec<String> = Vec::new();
    reasons.push("자동(랭킹)".into());
    if is_main_actor {
      reasons.push("주요 당사자 포함".into());
    }
    if actor_hits > 0 {
      reasons.push(format!("주체 일치 {}명", actor_hits));
    }
    if related_hits > 0 {
      reasons.push(format!("관련자 일치 {}명", related_hits));
    }
    if !q_tokens.is_empty() {
      reasons.push(format!("키워드 {}/{}", q_hit, q_total));
    }
    if has_range {
      reasons.push(if in_range { "기간 내".into() } else { "기간 밖".into() });
    }

    let components = RankedComponents {
      keyword_score,
      text_sim: sim,
      q_hit: q_hit as u32,
      q_total: q_total as u32,

      actor_score,
      actor_match: actor_match_any,
      actor_hits: actor_hits as u32,
      is_main_actor,

      related_score,
      related_hits: related_hits as u32,

      in_range: if has_range { Some(in_range) } else { None },

      w_actor,
      w_related,
      w_text,
      min_score,
      min_text_sim,
    };

    let tmp = Tmp {
      id: r.id.clone(),
      score,
      ts: r.ts.clone(),
      reasons,
      components,
    };

    if is_main_actor {
      main_hits.push(tmp);
      continue;
    }

    let passes_logic = actor_match_any
      || related_hits > 0
      || (!q_tokens.is_empty() && sim >= min_text_sim);

    if passes_logic && score >= min_score {
      candidates.push(tmp);
    }
  }

  main_hits.sort_by(|a, b| match b.ts.cmp(&a.ts) {
    Ordering::Equal => match b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal) {
      Ordering::Equal => a.id.cmp(&b.id),
      other => other,
    },
    other => other,
  });

  candidates.sort_by(|a, b| match b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal) {
    Ordering::Equal => match b.ts.cmp(&a.ts) {
      Ordering::Equal => a.id.cmp(&b.id),
      other => other,
    },
    other => other,
  });

  let mut merged: Vec<Tmp> = Vec::new();

  for t in main_hits.into_iter().take(k) {
    merged.push(t);
  }

  if merged.len() < k {
    let remain = k - merged.len();
    for t in candidates.into_iter().take(remain) {
      if merged.iter().any(|x| x.id == t.id) {
        continue;
      }
      merged.push(t);
    }
  }

  merged
    .into_iter()
    .enumerate()
    .map(|(i, t)| RankedHit {
      id: t.id,
      score: t.score,
      rank: (i + 1) as u32,
      reasons: t.reasons,
      components: t.components,
    })
    .collect()
}


/* -------------------- legacy risk compatibility -------------------- */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyRecordRisk {
  pub label: u8,
  pub label_text: String,
  #[serde(default)]
  pub probs: Vec<f32>,
  #[serde(default)]
  pub confidence: f32,
  #[serde(default)]
  pub reasons: Vec<String>,
  #[serde(default)]
  pub model_version: String,
  #[serde(default)]
  pub scored_at: String,
}


/* -------------------- core: advise -------------------- */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvisorItem {
  pub id: String,
  pub ts: String,
  pub title: String,
  pub body: String,
  pub level: String,
  #[serde(default)]
  pub tags: Vec<String>,
  pub state: String,
  pub rule_id: Option<String>,
}

// [수정] js_sys::Date::now() 대신 Rust 표준 라이브러리 사용
fn uid(prefix: &str) -> String {
  let start = SystemTime::now();
  let since_the_epoch = start
    .duration_since(UNIX_EPOCH)
    .expect("Time went backwards");
  let timestamp = since_the_epoch.as_millis();
  format!("{}_{}", prefix, timestamp)
}

fn chrono_like_now_iso() -> String {
  // TODO: 실제 ISO8601 문자열이 필요하면 chrono::Utc::now().to_rfc3339() 등을 사용
  "1970-01-01T00:00:00Z".into()
}

pub fn generate_advisors_for_case(case_item: &CaseItem, _records: &[RecordItem]) -> Vec<AdvisorItem> {
  let ts = chrono_like_now_iso();
  let mut out: Vec<AdvisorItem> = Vec::new();

  out.push(AdvisorItem {
    id: uid("ADV"),
    ts: ts.clone(),
    title: "증빙 정리".into(),
    body: "시간순으로 사실만 정리하고, 원본 증빙(녹취/문서/메신저)을 함께 묶어두세요.".into(),
    level: "info".into(),
    tags: vec!["정리".into()],
    state: "active".into(),
    rule_id: Some("proto:pack".into()),
  });

  out.push(AdvisorItem {
    id: uid("ADV"),
    ts: ts.clone(),
    title: "상대에게 전달".into(),
    body: "감정 표현 대신 사실과 조치만 전달하고, 필요하면 외부 전문기관/관리자 경로를 안내하세요.".into(),
    level: "warn".into(),
    tags: vec!["대화".into()],
    state: "active".into(),
    rule_id: Some("proto:talk".into()),
  });

  out.push(AdvisorItem {
    id: uid("ADV"),
    ts,
    title: "후속 조치".into(),
    body: "내부 보고/기록 보관/재발 방지 계획을 남겨두면 추후 방어에 도움이 됩니다.".into(),
    level: "info".into(),
    tags: vec!["후속".into()],
    state: "active".into(),
    rule_id: Some("proto:follow".into()),
  });

  out
}


const STRATEGY_MODEL_DEFAULT_ID: &str = "hyperclova-x";
const STRATEGY_MODEL_ROOSY_ID: &str = "roosy-x";
const STRATEGY_MODEL_HYBRID_ID: &str = "roosy-hybrid";
const MANAGED_LLAMA_SERVER_HYPERCLOVA_URL: &str = "http://127.0.0.1:18081/completion";
const MANAGED_LLAMA_SERVER_ROOSY_URL: &str = "http://127.0.0.1:18082/completion";
const LEGACY_LLAMA_SERVER_HYPERCLOVA_URL: &str = "http://127.0.0.1:8081/completion";
const LEGACY_LLAMA_SERVER_ROOSY_URL: &str = "http://127.0.0.1:8082/completion";
const STRATEGY_MODEL_FILENAME: &str = "HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf";
const STRATEGY_MODEL_RESOURCE_PATH: &str = "models/HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf";
const STRATEGY_MODEL_ROOSY_FILENAME: &str = "hyperclovax_roosy_Q4_K_M.gguf";
const STRATEGY_MODEL_ROOSY_RESOURCE_PATH: &str = "models/hyperclovax_roosy_Q4_K_M.gguf";
const STRATEGY_MODEL_DEFAULT_URL: &str = "https://github.com/myongsung/roosycozy-models/releases/download/model_v1/HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf";
const STRATEGY_MODEL_ROOSY_DEFAULT_URL: &str = "https://github.com/myongsung/roosycozy-models2/releases/download/model/hyperclovax_roosy_Q4_K_M.gguf";
const STRATEGY_SIDECAR_STEM: &str = "llama-sidecar";
const STRATEGY_LLAMA_SERVER_STEM: &str = "llama-server";
const STRATEGY_PROGRESS_EVENT: &str = "strategy-chat-progress";
const STRATEGY_CHAT_TIMEOUT_SECS: u64 = 90;
const STRATEGY_LEGAL_RAG_JSON: &str = include_str!("../legal/kr_school_guidance_laws_rag_expanded.json");
const STRATEGY_LEGAL_RAG_JSONL: &str = include_str!("../legal/kr_school_guidance_laws_rag_expanded_flat.jsonl");
const STRATEGY_RECORD_TEMPLATE_JSON: &str = include_str!("../legal/record_mode_template_ko.json");
#[cfg(target_os = "windows")]
const STRATEGY_CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
const STRATEGY_SIDECAR_GENERIC_FILENAME: &str = "llama-sidecar.exe";
#[cfg(target_os = "windows")]
const STRATEGY_LLAMA_SERVER_GENERIC_FILENAME: &str = "llama-server.exe";
#[cfg(not(target_os = "windows"))]
const STRATEGY_SIDECAR_GENERIC_FILENAME: &str = STRATEGY_SIDECAR_STEM;
#[cfg(not(target_os = "windows"))]
const STRATEGY_LLAMA_SERVER_GENERIC_FILENAME: &str = STRATEGY_LLAMA_SERVER_STEM;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar-aarch64-apple-darwin";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const STRATEGY_LLAMA_SERVER_FILENAME: &str = "llama-server-aarch64-apple-darwin";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar-x86_64-apple-darwin";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const STRATEGY_LLAMA_SERVER_FILENAME: &str = "llama-server-x86_64-apple-darwin";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar-x86_64-pc-windows-msvc.exe";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const STRATEGY_LLAMA_SERVER_FILENAME: &str = "llama-server-x86_64-pc-windows-msvc.exe";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar-x86_64-unknown-linux-gnu";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const STRATEGY_LLAMA_SERVER_FILENAME: &str = "llama-server-x86_64-unknown-linux-gnu";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar-aarch64-unknown-linux-gnu";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const STRATEGY_LLAMA_SERVER_FILENAME: &str = "llama-server-aarch64-unknown-linux-gnu";
#[cfg(not(any(
  all(target_os = "macos", target_arch = "aarch64"),
  all(target_os = "macos", target_arch = "x86_64"),
  all(target_os = "windows", target_arch = "x86_64"),
  all(target_os = "linux", target_arch = "x86_64"),
  all(target_os = "linux", target_arch = "aarch64")
)))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar";
#[cfg(not(any(
  all(target_os = "macos", target_arch = "aarch64"),
  all(target_os = "macos", target_arch = "x86_64"),
  all(target_os = "windows", target_arch = "x86_64"),
  all(target_os = "linux", target_arch = "x86_64"),
  all(target_os = "linux", target_arch = "aarch64")
)))]
const STRATEGY_LLAMA_SERVER_FILENAME: &str = "llama-server";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyChatTurn {
  pub role: String,
  pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StrategyChatOptions {
  #[serde(default)]
  pub model: Option<String>,
  #[serde(default)]
  pub max_tokens: Option<u32>,
  #[serde(default)]
  pub n_ctx: Option<u32>,
  #[serde(default)]
  pub threads: Option<u32>,
  #[serde(default)]
  pub synthetic_cache_enabled: Option<bool>,
  #[serde(default)]
  pub backend_mode: Option<String>,
  #[serde(default)]
  pub llama_server: Option<LlamaServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyChatRunResult {
  pub answer: String,
  pub model_path: String,
  pub runner: String,
  pub prompt_chars: usize,
  pub records_used: usize,
  pub retrieval_query: String,
  pub evidence_packet: StrategyEvidencePacket,
  pub perf_metrics: StrategyPerfMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyBackendPrewarmResult {
  pub backend_kind: String,
  pub ready: bool,
  pub reason: String,
  pub hyperclova_endpoint: String,
  pub roosy_endpoint: String,
}

#[derive(Debug, Clone)]
struct StrategyModelExecution {
  answer: String,
  model_path: String,
  runner: String,
  prompt_chars: usize,
  metrics: StrategyStagePerf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyPerfMetrics {
  pub mode: String,
  pub run_id: String,
  pub prompt_hash: String,
  pub case_hash: String,
  pub records_hash: String,
  pub model_config_hash: String,
  pub backend_kind: String,
  pub cache_requested: bool,
  pub cache_loaded: bool,
  pub cache_applied: bool,
  pub requested_mode: String,
  pub applied_mode: String,
  pub bypass_reason: String,
  pub total_e2e_ms: u128,
  pub ttft_ms: Option<u128>,
  pub total_prompt_tokens: usize,
  pub total_output_tokens: usize,
  pub e2e_tps: f32,
  pub decode_tps: f32,
  pub final_stage_tps: f32,
  pub full_drace_applied_stages: Vec<String>,
  pub peak_memory_mb: u64,
  pub stage_execution_sum_ms: u128,
  pub orchestration_overhead_ms: u128,
  pub prompt_build_ms: u128,
  pub cache_capability_ms: u128,
  pub cache_plan_ms: u128,
  pub cache_lookup_ms: u128,
  pub prompt_file_write_ms: u128,
  pub process_spawn_ms: u128,
  pub stdout_read_ms: u128,
  pub postprocess_ms: u128,
  pub other_overhead_ms: u128,
  pub stages: Vec<StrategyStagePerf>,
  pub cache_summary: StrategyCacheSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyStageCachePerf {
  pub cache_requested: bool,
  pub cache_loaded: bool,
  pub cache_enabled: bool,
  pub cache_supported: bool,
  pub cache_warm: bool,
  pub cache_applied: bool,
  pub synthetic_cache_requested: bool,
  pub synthetic_cache_supported: bool,
  pub synthetic_cache_applied: bool,
  pub cache_mode_requested: String,
  pub cache_mode_applied: String,
  pub bypass_reason: String,
  pub draft_provider: String,
  pub prompt_token_cache_supported: bool,
  pub prompt_token_cache_loaded: bool,
  pub prompt_token_cache_applied: bool,
  pub prompt_token_cache_hit_ratio: f32,
  pub prefix_kv_supported: bool,
  pub prefix_kv_applied: bool,
  pub prefix_reused_tokens: usize,
  pub prefix_total_tokens: usize,
  pub prefix_reuse_ratio: f32,
  pub kv_load_ms: u128,
  pub kv_save_ms: u128,
  pub token_cache_supported: bool,
  pub token_cache_loaded: bool,
  pub token_cache_applied: bool,
  pub token_verification_supported: bool,
  pub token_cache_lookup_ms: u128,
  pub proposed_tokens: usize,
  pub accepted_tokens: usize,
  pub rejected_tokens: usize,
  pub acceptance_ratio: f32,
  pub verify_batches: usize,
  pub rejected_batches: usize,
  pub avg_proposed_batch_size: f32,
  pub avg_accepted_batch_size: f32,
  pub accepted_tokens_per_verify: f32,
  pub fallback_tokens: usize,
  pub fallback_decode_tokens: usize,
  pub renderer_inserted_tokens: usize,
  pub llm_generated_tokens: usize,
  pub output_token_reduction_ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyStagePerf {
  pub stage_name: String,
  pub model_id: String,
  pub backend_kind: String,
  pub runner_path: String,
  model_path: String,
  pub threads: u32,
  pub threads_batch: u32,
  pub n_ctx: u32,
  pub max_tokens: u32,
  pub temperature: f32,
  pub top_p: f32,
  pub repeat_penalty: f32,
  pub e2e_ms: u128,
  pub ttft_ms: Option<u128>,
  pub prompt_tokens: usize,
  pub output_tokens: usize,
  pub prompt_eval_ms: Option<u128>,
  pub decode_ms: Option<u128>,
  pub e2e_tps: f32,
  pub decode_tps: f32,
  pub peak_memory_mb: u64,
  pub process_spawn_ms: u128,
  pub prompt_file_write_ms: u128,
  pub stdout_read_ms: u128,
  pub postprocess_ms: u128,
  pub cache: StrategyStageCachePerf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyCacheSummary {
  pub backend_type: String,
  pub cache_requested: bool,
  pub cache_loaded: bool,
  pub cache_applied: bool,
  pub cache_supported: bool,
  pub cache_warm: bool,
  pub synthetic_cache_requested: bool,
  pub synthetic_cache_supported: bool,
  pub synthetic_cache_applied: bool,
  pub cache_mode_requested: String,
  pub cache_mode_applied: String,
  pub bypass_reason: String,
  pub draft_provider: String,
  pub prompt_token_cache_supported: bool,
  pub prompt_token_cache_loaded: bool,
  pub prompt_token_cache_applied: bool,
  pub prompt_token_cache_hit_ratio: f32,
  pub prefix_kv_supported: bool,
  pub prefix_kv_applied: bool,
  pub prefix_reused_tokens: usize,
  pub prefix_total_tokens: usize,
  pub prefix_reuse_ratio: f32,
  pub token_cache_supported: bool,
  pub token_cache_loaded: bool,
  pub token_cache_applied: bool,
  pub token_verification_supported: bool,
  pub proposed_tokens: usize,
  pub accepted_tokens: usize,
  pub rejected_tokens: usize,
  pub acceptance_ratio: f32,
  pub verify_batches: usize,
  pub rejected_batches: usize,
  pub avg_proposed_batch_size: f32,
  pub avg_accepted_batch_size: f32,
  pub accepted_tokens_per_verify: f32,
  pub fallback_tokens: usize,
  pub fallback_decode_tokens: usize,
  pub renderer_inserted_tokens: usize,
  pub llm_generated_tokens: usize,
  pub output_token_reduction_ratio: f32,
}

#[derive(Debug, Clone, Copy)]
struct StrategyRuntimeConfig {
  n_ctx: u32,
  threads: u32,
  n_gpu_layers: u32,
  device: &'static str,
}

#[derive(Debug, Clone)]
struct StrategyBackendRuntime {
  capabilities: BackendCapabilities,
  llama_server: Option<LlamaServerConfig>,
  llama_server_available: bool,
  llama_server_unavailable_reason: String,
}

struct StrategyLlamaServerProcess {
  child: std::process::Child,
  endpoint: String,
  ctx_size: u32,
  last_start_attempt: Instant,
  last_failure_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct StrategyOverheadMetrics {
  prompt_build_ms: u128,
  cache_capability_ms: u128,
  cache_plan_ms: u128,
  cache_lookup_ms: u128,
  prompt_file_write_ms: u128,
  process_spawn_ms: u128,
  stdout_read_ms: u128,
  postprocess_ms: u128,
}

#[derive(Debug, Clone, Copy)]
struct StrategyGenerationTuning {
  temperature: f32,
  top_p: f32,
  repeat_penalty: f32,
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn strategy_default_threads() -> u32 {
  let logical_cores = thread::available_parallelism()
    .map(|value| value.get() as u32)
    .unwrap_or(4);
  let tuned = ((logical_cores as f32) * 0.75).round() as u32;
  tuned.max(1).clamp(1, 10)
}

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
fn strategy_default_threads() -> u32 {
  let logical_cores = thread::available_parallelism()
    .map(|value| value.get() as u32)
    .unwrap_or(4);
  let reserve = if logical_cores >= 10 { 2 } else { 1 };
  logical_cores.saturating_sub(reserve).max(1).clamp(1, 12)
}

fn strategy_runtime_device_config() -> (&'static str, u32) {
  ("none", 0)
}

fn strategy_runtime_config(
  requested_n_ctx: Option<u32>,
  requested_threads: Option<u32>,
) -> StrategyRuntimeConfig {
  let (device, n_gpu_layers) = strategy_runtime_device_config();
  let n_ctx = requested_n_ctx.unwrap_or(4096).clamp(2048, 4096);
  let threads = requested_threads.unwrap_or_else(strategy_default_threads).clamp(1, 12);
  StrategyRuntimeConfig {
    n_ctx,
    threads,
    n_gpu_layers,
    device,
  }
}

#[derive(Debug, Clone)]
struct StrategyLlamaServerEndpointSpec {
  completion_url: String,
  base_url: String,
  host: String,
  port: u16,
  loopback: bool,
}

fn strategy_llama_server_registry() -> &'static Mutex<HashMap<String, StrategyLlamaServerProcess>> {
  STRATEGY_LLAMA_SERVER_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn strategy_llama_server_endpoint_registry() -> &'static Mutex<HashMap<String, String>> {
  STRATEGY_LLAMA_SERVER_ENDPOINTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn parse_llama_server_endpoint(endpoint: &str) -> Result<StrategyLlamaServerEndpointSpec, String> {
  let trimmed = endpoint.trim();
  if trimmed.is_empty() {
    return Err("endpoint_missing".to_string());
  }
  let mut url = Url::parse(trimmed).map_err(|e| format!("invalid_url: {e}"))?;
  let host = url
    .host_str()
    .map(|v| v.to_string())
    .ok_or_else(|| "missing_host".to_string())?;
  let port = url.port_or_known_default().ok_or_else(|| "missing_port".to_string())?;
  let path = url.path().trim_end_matches('/').to_string();
  let base_path = if path.ends_with("/completion") {
    path.trim_end_matches("/completion").to_string()
  } else {
    path
  };
  url.set_path(if base_path.is_empty() { "/" } else { &base_path });
  url.set_query(None);
  url.set_fragment(None);
  let base_url = url.to_string().trim_end_matches('/').to_string();
  let loopback = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
  Ok(StrategyLlamaServerEndpointSpec {
    completion_url: trimmed.to_string(),
    base_url,
    host,
    port,
    loopback,
  })
}

fn classify_llama_server_reqwest_error(scope: &str, endpoint: &str, err: &reqwest::Error) -> String {
  if err.is_timeout() {
    format!("{scope}: timeout ({endpoint})")
  } else if err.is_connect() {
    format!("{scope}: connection_refused ({endpoint})")
  } else {
    format!("{scope}: request_failed ({endpoint}) {err}")
  }
}

fn strategy_error_indicates_ctx_exhausted(message: &str) -> bool {
  let lower = message.to_ascii_lowercase();
  lower.contains("exceeds the available context size")
    || lower.contains("available context size")
    || lower.contains("try increasing it")
    || lower.contains("context size")
}

fn normalize_loopback_llama_server_endpoint(configured: &str, managed: &str, legacy: &str) -> String {
  let trimmed = configured.trim();
  if trimmed.is_empty() {
    return managed.to_string();
  }
  let lower = trimmed.to_ascii_lowercase();
  if lower == managed.to_ascii_lowercase() || lower == legacy.to_ascii_lowercase() {
    return managed.to_string();
  }
  trimmed.to_string()
}

fn strategy_endpoint_is_managed_candidate(configured: &str, managed: &str, legacy: &str) -> bool {
  let trimmed = configured.trim();
  if trimmed.is_empty() {
    return true;
  }
  let lower = trimmed.to_ascii_lowercase();
  lower == managed.to_ascii_lowercase() || lower == legacy.to_ascii_lowercase()
}

fn strategy_port_is_available(port: u16) -> bool {
  TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn strategy_port_reserved_in_endpoint_registry(guard: &HashMap<String, String>, port: u16) -> bool {
  guard.values().any(|endpoint| {
    parse_llama_server_endpoint(endpoint)
      .map(|spec| spec.port == port)
      .unwrap_or(false)
  })
}

fn strategy_allocate_managed_endpoint(model_id: &str) -> String {
  let (start_port, end_port) = if model_id == STRATEGY_MODEL_ROOSY_ID {
    (19082u16, 19140u16)
  } else {
    (19081u16, 19139u16)
  };

  if let Ok(mut guard) = strategy_llama_server_endpoint_registry().lock() {
    if let Some(saved) = guard.get(model_id) {
      return saved.clone();
    }
    for port in start_port..=end_port {
      if strategy_port_reserved_in_endpoint_registry(&guard, port) {
        continue;
      }
      if strategy_port_is_available(port) {
        let endpoint = format!("http://127.0.0.1:{port}/completion");
        guard.insert(model_id.to_string(), endpoint.clone());
        return endpoint;
      }
    }
  }

  if model_id == STRATEGY_MODEL_ROOSY_ID {
    MANAGED_LLAMA_SERVER_ROOSY_URL.to_string()
  } else {
    MANAGED_LLAMA_SERVER_HYPERCLOVA_URL.to_string()
  }
}

fn probe_llama_server_endpoint(endpoint: &str, timeout_ms: u64) -> Result<(), String> {
  let spec = parse_llama_server_endpoint(endpoint)?;
  let client = reqwest::blocking::Client::builder()
    .timeout(Duration::from_millis(timeout_ms.max(250)))
    .build()
    .map_err(|e| format!("llama-server healthcheck 클라이언트를 만들지 못했어요: {e}"))?;

  let slots_url = format!("{}/slots", spec.base_url);
  match client.get(&slots_url).send() {
    Ok(response) if response.status().is_success() => return Ok(()),
    Ok(response) => {
      if response.status().is_server_error() {
        return Err(format!("slots_probe_failed: {} ({})", response.status(), slots_url));
      }
    }
    Err(err) => {
      let reason = classify_llama_server_reqwest_error("slots_probe_failed", &slots_url, &err);
      if !err.is_connect() && !err.is_timeout() {
        return Err(reason);
      }
    }
  }

  client
    .post(&spec.completion_url)
    .json(&serde_json::json!({
      "prompt": "",
      "n_predict": 0,
      "stream": false,
    }))
    .send()
    .map_err(|e| classify_llama_server_reqwest_error("completion_probe_failed", &spec.completion_url, &e))
    .and_then(|response| {
      if response.status().is_success() {
        Ok(())
      } else {
        Err(format!(
          "completion_probe_failed: http_{} ({})",
          response.status(),
          spec.completion_url
        ))
      }
    })
}

fn wait_for_strategy_llama_server_ready(
  model_id: &str,
  endpoint: &str,
  startup_timeout_ms: u64,
) -> Result<(), String> {
  let deadline = Instant::now() + Duration::from_millis(startup_timeout_ms.max(1_000));
  let mut last_error = "startup_probe_pending".to_string();
  let mut consecutive_successes = 0u8;

  while Instant::now() < deadline {
    match probe_llama_server_endpoint(endpoint, 1_000) {
      Ok(()) => {
        consecutive_successes = consecutive_successes.saturating_add(1);
        if consecutive_successes >= 2 {
          return Ok(());
        }
      }
      Err(err) => {
        consecutive_successes = 0;
        last_error = err;
      }
    }

    if let Ok(mut guard) = strategy_llama_server_registry().lock() {
      if let Some(process) = guard.get_mut(model_id) {
        match process.child.try_wait() {
          Ok(Some(status)) => {
            last_error = format!("server_exited_early: {}", status);
            process.last_failure_reason = Some(last_error.clone());
            guard.remove(model_id);
            break;
          }
          Ok(None) => {}
          Err(err) => {
            last_error = format!("server_wait_failed: {err}");
            process.last_failure_reason = Some(last_error.clone());
            guard.remove(model_id);
            break;
          }
        }
      } else {
        last_error = "server_registry_missing".to_string();
        break;
      }
    }

    thread::sleep(Duration::from_millis(300));
  }

  if let Ok(mut guard) = strategy_llama_server_registry().lock() {
    if let Some(process) = guard.get_mut(model_id) {
      process.last_failure_reason = Some(last_error.clone());
    }
  }

  Err(format!("llama_server_startup_timeout: {}", last_error))
}

fn ensure_strategy_llama_server_process(
  app: Option<&AppHandle>,
  endpoint: &str,
  model_id: &str,
  requested_ctx_size: u32,
  startup_timeout_ms: u64,
) -> Result<(), String> {
  let spec = parse_llama_server_endpoint(endpoint)?;
  if !spec.loopback {
    return Err(format!("manual_endpoint_required: {}", endpoint));
  }
  let desired_ctx_size = requested_ctx_size.max(2048).clamp(2048, 4096);

  let registry = strategy_llama_server_registry();
  let mut reuse_existing_process = false;
  if let Ok(mut guard) = registry.lock() {
    if let Some(process) = guard.get_mut(model_id) {
      match process.child.try_wait() {
        Ok(None) => {
          if process.endpoint == endpoint && process.ctx_size >= desired_ctx_size {
            reuse_existing_process = true;
          } else {
            let _ = process.child.kill();
            let _ = process.child.wait();
            process.last_failure_reason = Some(format!(
              "server_restarted_for_ctx_or_endpoint: endpoint={} ctx_size={} desired_ctx={}",
              process.endpoint,
              process.ctx_size,
              desired_ctx_size
            ));
          }
        }
        Ok(Some(status)) => {
          process.last_failure_reason = Some(format!("server_exited: {}", status));
        }
        Err(err) => {
          process.last_failure_reason = Some(format!("server_wait_failed: {err}"));
        }
      }
      if !reuse_existing_process {
        guard.remove(model_id);
      }
    }
  }

  if reuse_existing_process {
    if wait_for_strategy_llama_server_ready(model_id, endpoint, startup_timeout_ms).is_ok() {
      return Ok(());
    }
    if let Ok(mut retry_guard) = registry.lock() {
      if let Some(retry_process) = retry_guard.get_mut(model_id) {
        let _ = retry_process.child.kill();
        let _ = retry_process.child.wait();
        retry_process.last_failure_reason = Some("server_unhealthy_during_reuse_probe".to_string());
      }
      retry_guard.remove(model_id);
    }
  }

  let runner = resolve_llama_server_runner_path(app)?;
  let model_path = resolve_strategy_model_path(app, model_id)?;
  let threads = strategy_default_threads().max(2);
  let mut command = Command::new(&runner);
  command
    .arg("--host")
    .arg(&spec.host)
    .arg("--port")
    .arg(spec.port.to_string())
    .arg("--slots")
    .arg("--cache-ram")
    .arg("4096")
    .arg("--ctx-checkpoints")
    .arg("32")
    .arg("--parallel")
    .arg("1")
    .arg("--ctx-size")
    .arg(desired_ctx_size.to_string())
    .arg("--threads")
    .arg(threads.to_string())
    .arg("--threads-batch")
    .arg(threads.to_string())
    .arg("--no-warmup")
    .arg("--model")
    .arg(&model_path)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
  #[cfg(target_os = "windows")]
  command.creation_flags(STRATEGY_CREATE_NO_WINDOW);

  let child = command.spawn().map_err(|e| {
    format!(
      "llama_server_spawn_failed: {} ({})",
      strategy_trim(&e.to_string(), 180),
      runner.display()
    )
  })?;

  if let Ok(mut guard) = registry.lock() {
    guard.insert(
      model_id.to_string(),
      StrategyLlamaServerProcess {
        child,
        endpoint: endpoint.to_string(),
        ctx_size: desired_ctx_size,
        last_start_attempt: Instant::now(),
        last_failure_reason: None,
      },
    );
  }

  wait_for_strategy_llama_server_ready(model_id, endpoint, startup_timeout_ms)
}

fn reset_strategy_llama_server_process(model_id: &str) {
  let registry = strategy_llama_server_registry();
  if let Ok(mut guard) = registry.lock() {
    if let Some(mut process) = guard.remove(model_id) {
      let _ = process.child.kill();
      let _ = process.child.wait();
    }
  }
}

fn check_llama_server_runtime(app: Option<&AppHandle>, config: Option<&LlamaServerConfig>) -> (bool, String) {
  let Some(config) = config else {
    return (false, "llama_server_endpoint_missing".to_string());
  };

  let timeout_ms = config.startup_timeout_ms.min(3_000).max(500);
  if config.hyperclova_url.trim().is_empty() || config.roosy_url.trim().is_empty() {
    return (false, "llama_server_endpoint_missing".to_string());
  }

  let (hyper_endpoint, _) = effective_llama_server_endpoint_for_model(config, STRATEGY_MODEL_DEFAULT_ID);
  let (roosy_endpoint, _) = effective_llama_server_endpoint_for_model(config, STRATEGY_MODEL_ROOSY_ID);
  if hyper_endpoint.eq_ignore_ascii_case(&roosy_endpoint) {
    return (
      false,
      format!(
        "llama_server_endpoint_collision: hyperclova={} / roosy={}",
        hyper_endpoint,
        roosy_endpoint
      ),
    );
  }

  let mut hyper = probe_llama_server_endpoint(&hyper_endpoint, timeout_ms);
  if let Err(probe_err) = hyper.clone() {
    hyper = match ensure_strategy_llama_server_process(
      app,
      &hyper_endpoint,
      STRATEGY_MODEL_DEFAULT_ID,
      4096,
      config.startup_timeout_ms,
    ) {
      Ok(()) => probe_llama_server_endpoint(&hyper_endpoint, timeout_ms),
      Err(start_err) => Err(format!("{probe_err}; {start_err}")),
    };
    if let Err(first_retry_err) = hyper.clone() {
      reset_strategy_llama_server_process(STRATEGY_MODEL_DEFAULT_ID);
      hyper = match ensure_strategy_llama_server_process(
        app,
        &hyper_endpoint,
        STRATEGY_MODEL_DEFAULT_ID,
        4096,
        config.startup_timeout_ms,
      ) {
        Ok(()) => probe_llama_server_endpoint(&hyper_endpoint, timeout_ms),
        Err(restart_err) => Err(format!("{first_retry_err}; restart_retry_failed: {restart_err}")),
      };
    }
  }

  let mut roosy = probe_llama_server_endpoint(&roosy_endpoint, timeout_ms);
  if let Err(probe_err) = roosy.clone() {
    roosy = match ensure_strategy_llama_server_process(
      app,
      &roosy_endpoint,
      STRATEGY_MODEL_ROOSY_ID,
      4096,
      config.startup_timeout_ms,
    ) {
      Ok(()) => probe_llama_server_endpoint(&roosy_endpoint, timeout_ms),
      Err(start_err) => Err(format!("{probe_err}; {start_err}")),
    };
    if let Err(first_retry_err) = roosy.clone() {
      reset_strategy_llama_server_process(STRATEGY_MODEL_ROOSY_ID);
      roosy = match ensure_strategy_llama_server_process(
        app,
        &roosy_endpoint,
        STRATEGY_MODEL_ROOSY_ID,
        4096,
        config.startup_timeout_ms,
      ) {
        Ok(()) => probe_llama_server_endpoint(&roosy_endpoint, timeout_ms),
        Err(restart_err) => Err(format!("{first_retry_err}; restart_retry_failed: {restart_err}")),
      };
    }
  }

  match (hyper, roosy) {
    (Ok(_), Ok(_)) => (true, String::new()),
    (Err(hyper_err), Err(roosy_err)) => (
      false,
      format!("llama_server_unavailable: hyperclova={}, roosy={}", hyper_err, roosy_err),
    ),
    (Err(hyper_err), Ok(_)) => (
      false,
      format!("llama_server_unavailable: hyperclova={}", hyper_err),
    ),
    (Ok(_), Err(roosy_err)) => (
      false,
      format!("llama_server_unavailable: roosy={}", roosy_err),
    ),
  }
}

fn strategy_backend_runtime(app: Option<&AppHandle>, opts: Option<&StrategyChatOptions>) -> StrategyBackendRuntime {
  strategy_load_drace_persistent_state(app);
  let runtime = detect_backend_runtime(
    opts.and_then(|value| value.backend_mode.as_deref()),
    opts.and_then(|value| value.llama_server.clone()),
  );
  let (llama_server_available, llama_server_unavailable_reason) =
    if matches!(runtime.capabilities.backend_kind, BackendKind::LlamaServer) {
      check_llama_server_runtime(app, runtime.llama_server.as_ref())
    } else {
      (false, String::new())
    };
  let actual_capabilities = if matches!(runtime.capabilities.backend_kind, BackendKind::LlamaServer) && !llama_server_available {
    BackendCapabilities {
      backend_kind: BackendKind::CliSidecar,
      supports_resident: false,
      supports_prompt_token_cache: false,
      supports_prompt_cache: false,
      supports_prefix_kv_cache: false,
      supports_token_verification: false,
      supports_batch_verification: false,
      supports_speculative_decode: false,
      supports_synthetic_token_cache: false,
      supports_mmap_cache_pack: false,
      supports_resident_model: false,
    }
  } else {
    runtime.capabilities.clone()
  };
  let manager = DraceCacheManager::global();
  manager.configure_backend(actual_capabilities.clone());
  StrategyBackendRuntime {
    capabilities: actual_capabilities,
    llama_server: runtime.llama_server,
    llama_server_available,
    llama_server_unavailable_reason,
  }
}

pub fn prewarm_strategy_backend(
  app: Option<&AppHandle>,
  opts: Option<StrategyChatOptions>,
) -> StrategyBackendPrewarmResult {
  strategy_load_drace_persistent_state(app);
  let runtime = detect_backend_runtime(
    opts.as_ref().and_then(|value| value.backend_mode.as_deref()),
    opts.as_ref().and_then(|value| value.llama_server.clone()),
  );
  if !matches!(runtime.capabilities.backend_kind, BackendKind::LlamaServer) {
    return StrategyBackendPrewarmResult {
      backend_kind: match runtime.capabilities.backend_kind {
        BackendKind::CliSidecar => "cli".to_string(),
        BackendKind::LlamaServer => "llama-server".to_string(),
        BackendKind::Native => "native".to_string(),
      },
      ready: false,
      reason: "cache_disabled_or_cli_mode".to_string(),
      hyperclova_endpoint: String::new(),
      roosy_endpoint: String::new(),
    };
  }
  let Some(config) = runtime.llama_server.as_ref() else {
    return StrategyBackendPrewarmResult {
      backend_kind: "llama-server".to_string(),
      ready: false,
      reason: "llama_server_endpoint_missing".to_string(),
      hyperclova_endpoint: String::new(),
      roosy_endpoint: String::new(),
    };
  };
  let (hyperclova_endpoint, _) = effective_llama_server_endpoint_for_model(config, STRATEGY_MODEL_DEFAULT_ID);
  let (roosy_endpoint, _) = effective_llama_server_endpoint_for_model(config, STRATEGY_MODEL_ROOSY_ID);
  let (ready, reason) = check_llama_server_runtime(app, Some(config));
  if ready {
    strategy_prewarm_known_static_prefixes_for_model(STRATEGY_MODEL_DEFAULT_ID, app, config);
    strategy_prewarm_known_static_prefixes_for_model(STRATEGY_MODEL_ROOSY_ID, app, config);
  }
  StrategyBackendPrewarmResult {
    backend_kind: if ready { "llama-server".to_string() } else { "cli".to_string() },
    ready,
    reason,
    hyperclova_endpoint,
    roosy_endpoint,
  }
}

fn strategy_fast_hash_hex(value: &str) -> String {
  format!("{:016x}", crate::drace::fast_hash64(value))
}

fn strategy_hash_json<T: Serialize>(value: &T) -> String {
  serde_json::to_string(value)
    .map(|json| strategy_fast_hash_hex(&json))
    .unwrap_or_else(|_| "0".repeat(16))
}

fn strategy_generation_tuning() -> StrategyGenerationTuning {
  StrategyGenerationTuning {
    temperature: 0.15,
    top_p: 0.85,
    repeat_penalty: 1.12,
  }
}

fn strategy_hybrid_draft_n_ctx(base_n_ctx: u32) -> u32 {
  base_n_ctx.min(3584).max(2560)
}

fn strategy_approx_output_tokens(text: &str) -> usize {
  let chars = text.chars().count();
  let words = text.split_whitespace().filter(|part| !part.trim().is_empty()).count();
  chars
    .saturating_div(4)
    .max(words)
    .max(1)
}

fn strategy_estimate_model_footprint_mb(model_path: &str) -> u64 {
  let bytes = fs::metadata(model_path).map(|meta| meta.len()).unwrap_or(0);
  if bytes == 0 {
    return 0;
  }
  bytes.div_ceil(1024_u64 * 1024_u64)
}

fn strategy_compute_output_tps(answer: &str, e2e_ms: u64, ttft_ms: u64) -> f32 {
  let effective_ms = e2e_ms.saturating_sub(ttft_ms).max(1);
  let seconds = (effective_ms as f32) / 1000.0;
  (strategy_approx_output_tokens(answer) as f32 / seconds * 100.0).round() / 100.0
}

fn strategy_backend_type_label() -> &'static str {
  match DraceCacheManager::global().capabilities_snapshot().backend_kind {
    BackendKind::CliSidecar => "cli",
    BackendKind::LlamaServer => "llama-server",
    BackendKind::Native => "native",
  }
}

fn strategy_prefix_kv_supported() -> bool {
  DraceCacheManager::global().capabilities_snapshot().supports_prefix_kv_cache
}

fn strategy_prompt_token_cache_supported() -> bool {
  DraceCacheManager::global()
    .capabilities_snapshot()
    .supports_prompt_token_cache
}

fn strategy_token_cache_supported() -> bool {
  DraceCacheManager::global()
    .capabilities_snapshot()
    .supports_synthetic_token_cache
}

fn strategy_token_verification_supported() -> bool {
  DraceCacheManager::global()
    .capabilities_snapshot()
    .supports_token_verification
}

fn strategy_cache_bypass_reason(cache_requested: bool) -> String {
  if !cache_requested {
    return "cache disabled".to_string();
  }
  "CLI backend does not support persistent Prefix KV Cache, and Synthetic Token Cache verification is unavailable".to_string()
}

fn build_strategy_stage_cache_perf(
  capabilities: &BackendCapabilities,
  plan: &crate::drace::StageCachePlan,
  prompt_tokens: usize,
  output_tokens: usize,
) -> StrategyStageCachePerf {
  let prefix_kv_supported = capabilities.supports_prefix_kv_cache;
  let prompt_token_cache_supported = capabilities.supports_prompt_token_cache;
  let token_cache_supported = capabilities.supports_synthetic_token_cache;
  let cache_supported = prefix_kv_supported || token_cache_supported || prompt_token_cache_supported;
  StrategyStageCachePerf {
    cache_requested: plan.cache_requested,
    cache_loaded: plan.cache_loaded,
    cache_enabled: plan.cache_requested,
    cache_supported,
    cache_warm: false,
    cache_applied: plan.cache_applied,
    synthetic_cache_requested: plan.synthetic_cache_requested,
    synthetic_cache_supported: plan.synthetic_cache_supported,
    synthetic_cache_applied: plan.synthetic_cache_applied,
    cache_mode_requested: plan.cache_mode_requested.clone(),
    cache_mode_applied: plan.cache_mode_applied.clone(),
    bypass_reason: if plan.bypass_reason.trim().is_empty() {
      strategy_cache_bypass_reason(plan.cache_requested)
    } else {
      plan.bypass_reason.clone()
    },
    draft_provider: plan.draft_provider.clone(),
    prompt_token_cache_supported,
    prompt_token_cache_loaded: plan.cache_loaded && prompt_token_cache_supported,
    prompt_token_cache_applied: plan.use_prompt_token_cache,
    prompt_token_cache_hit_ratio: plan.prompt_token_cache_hit_ratio,
    prefix_kv_supported,
    prefix_kv_applied: plan.use_prefix_kv_cache,
    prefix_reused_tokens: 0,
    prefix_total_tokens: if plan.cache_requested && prefix_kv_supported { prompt_tokens } else { 0 },
    prefix_reuse_ratio: 0.0,
    kv_load_ms: 0,
    kv_save_ms: 0,
    token_cache_supported,
    token_cache_loaded: plan.cache_loaded && token_cache_supported,
    token_cache_applied: plan.use_synthetic_token_cache,
    token_verification_supported: capabilities.supports_token_verification,
    token_cache_lookup_ms: plan.token_cache_lookup_ms,
    proposed_tokens: 0,
    accepted_tokens: 0,
    rejected_tokens: 0,
    acceptance_ratio: 0.0,
    verify_batches: 0,
    rejected_batches: 0,
    avg_proposed_batch_size: 0.0,
    avg_accepted_batch_size: 0.0,
    accepted_tokens_per_verify: plan.expected_accept_tokens_per_verify,
    fallback_tokens: if plan.cache_requested { output_tokens } else { 0 },
    fallback_decode_tokens: if plan.cache_requested { output_tokens } else { 0 },
    renderer_inserted_tokens: 0,
    llm_generated_tokens: output_tokens,
    output_token_reduction_ratio: 0.0,
  }
}

fn strategy_cli_bypass_plan(cache_requested: bool) -> crate::drace::StageCachePlan {
  strategy_cli_bypass_plan_with_reason(
    cache_requested,
    if cache_requested {
      "unsupported_backend_cli".to_string()
    } else {
      "cache disabled".to_string()
    },
  )
}

fn strategy_cli_bypass_plan_with_reason(
  cache_requested: bool,
  bypass_reason: String,
) -> crate::drace::StageCachePlan {
  crate::drace::StageCachePlan {
    cache_requested,
    cache_loaded: false,
    cache_applied: false,
    synthetic_cache_requested: cache_requested,
    synthetic_cache_supported: false,
    synthetic_cache_applied: false,
    cache_mode_requested: if cache_requested {
      "FullDRACE".to_string()
    } else {
      "Off".to_string()
    },
    cache_mode_applied: "Off".to_string(),
    use_prompt_token_cache: false,
    use_prefix_kv_cache: false,
    use_template_renderer: false,
    use_synthetic_token_cache: false,
    max_candidate_tokens: 0,
    prompt_token_cache_hit_ratio: 0.0,
    token_cache_lookup_ms: 0,
    expected_accept_tokens_per_verify: 0.0,
    draft_provider: "noop".to_string(),
    bypass_reason,
  }
}

fn summarize_strategy_cache(stages: &[StrategyStagePerf], cache_requested: bool) -> StrategyCacheSummary {
  let mut prefix_reused_tokens = 0usize;
  let mut prefix_total_tokens = 0usize;
  let mut proposed_tokens = 0usize;
  let mut accepted_tokens = 0usize;
  let mut rejected_tokens = 0usize;
  let mut verify_batches = 0usize;
  let mut rejected_batches = 0usize;
  let mut fallback_tokens = 0usize;
  let mut fallback_decode_tokens = 0usize;
  let mut renderer_inserted_tokens = 0usize;
  let mut llm_generated_tokens = 0usize;
  let mut cache_loaded = false;
  let mut cache_applied = false;
  let mut cache_warm = false;
  let mut reasons = Vec::<String>::new();
  let mut cache_supported = false;
  let mut cache_mode_requested = if cache_requested {
    "FullDRACE".to_string()
  } else {
    "Off".to_string()
  };
  let mut cache_mode_applied = "Off".to_string();
  let mut prompt_token_cache_supported = false;
  let mut prompt_token_cache_loaded = false;
  let mut prompt_token_cache_applied = false;
  let mut prompt_token_cache_hit_ratio = 0.0_f32;
  let mut prefix_kv_supported = false;
  let mut prefix_kv_applied = false;
  let mut synthetic_cache_requested = false;
  let mut synthetic_cache_supported = false;
  let mut synthetic_cache_applied = false;
  let mut draft_provider = String::new();
  let mut token_cache_supported = false;
  let mut token_cache_loaded = false;
  let mut token_cache_applied = false;
  let mut token_verification_supported = false;
  let mut backend_type = if let Some(last_stage) = stages.last() {
    last_stage.backend_kind.clone()
  } else {
    strategy_backend_type_label().to_string()
  };
  for stage in stages {
    let cache = &stage.cache;
    if backend_type.is_empty() {
      backend_type = stage.backend_kind.clone();
    }
    prefix_reused_tokens = prefix_reused_tokens.saturating_add(cache.prefix_reused_tokens);
    prefix_total_tokens = prefix_total_tokens.saturating_add(cache.prefix_total_tokens);
    proposed_tokens = proposed_tokens.saturating_add(cache.proposed_tokens);
    accepted_tokens = accepted_tokens.saturating_add(cache.accepted_tokens);
    rejected_tokens = rejected_tokens.saturating_add(cache.rejected_tokens);
    verify_batches = verify_batches.saturating_add(cache.verify_batches);
    rejected_batches = rejected_batches.saturating_add(cache.rejected_batches);
    fallback_tokens = fallback_tokens.saturating_add(cache.fallback_tokens);
    fallback_decode_tokens = fallback_decode_tokens.saturating_add(cache.fallback_decode_tokens);
    renderer_inserted_tokens = renderer_inserted_tokens.saturating_add(cache.renderer_inserted_tokens);
    llm_generated_tokens = llm_generated_tokens.saturating_add(cache.llm_generated_tokens);
    cache_loaded |= cache.cache_loaded;
    cache_applied |= cache.cache_applied;
    cache_warm |= cache.cache_warm;
    cache_supported |= cache.cache_supported;
    if cache.cache_requested && cache_mode_requested == "Off" {
      cache_mode_requested = cache.cache_mode_requested.clone();
    }
    if cache.cache_applied && cache_mode_applied == "Off" {
      cache_mode_applied = cache.cache_mode_applied.clone();
    }
    prompt_token_cache_supported |= cache.prompt_token_cache_supported;
    prompt_token_cache_loaded |= cache.prompt_token_cache_loaded;
    prompt_token_cache_applied |= cache.prompt_token_cache_applied;
    prompt_token_cache_hit_ratio = prompt_token_cache_hit_ratio.max(cache.prompt_token_cache_hit_ratio);
    prefix_kv_supported |= cache.prefix_kv_supported;
    prefix_kv_applied |= cache.prefix_kv_applied;
    synthetic_cache_requested |= cache.synthetic_cache_requested;
    synthetic_cache_supported |= cache.synthetic_cache_supported;
    synthetic_cache_applied |= cache.synthetic_cache_applied;
    if draft_provider.is_empty() && !cache.draft_provider.trim().is_empty() && cache.draft_provider != "noop" {
      draft_provider = cache.draft_provider.clone();
    }
    token_cache_supported |= cache.token_cache_supported;
    token_cache_loaded |= cache.token_cache_loaded;
    token_cache_applied |= cache.token_cache_applied;
    token_verification_supported |= cache.token_verification_supported;
    if !cache.bypass_reason.trim().is_empty() {
      reasons.push(cache.bypass_reason.clone());
    }
  }
  let accepted_tokens_per_verify = if verify_batches > 0 {
    accepted_tokens as f32 / verify_batches as f32
  } else {
    0.0
  };
  let acceptance_ratio = if proposed_tokens > 0 {
    accepted_tokens as f32 / proposed_tokens as f32
  } else {
    0.0
  };
  StrategyCacheSummary {
    backend_type,
    cache_requested,
    cache_loaded,
    cache_applied,
    cache_supported,
    cache_warm,
    synthetic_cache_requested,
    synthetic_cache_supported,
    synthetic_cache_applied,
    cache_mode_requested,
    cache_mode_applied,
    bypass_reason: reasons.into_iter().collect::<HashSet<_>>().into_iter().collect::<Vec<_>>().join(" / "),
    draft_provider: if draft_provider.is_empty() { "noop".to_string() } else { draft_provider },
    prompt_token_cache_supported,
    prompt_token_cache_loaded,
    prompt_token_cache_applied,
    prompt_token_cache_hit_ratio,
    prefix_kv_supported,
    prefix_kv_applied,
    prefix_reused_tokens,
    prefix_total_tokens,
    prefix_reuse_ratio: if prefix_total_tokens > 0 {
      prefix_reused_tokens as f32 / prefix_total_tokens as f32
    } else {
      0.0
    },
    token_cache_supported,
    token_cache_loaded,
    token_cache_applied,
    token_verification_supported,
    proposed_tokens,
    accepted_tokens,
    rejected_tokens,
    acceptance_ratio,
    verify_batches,
    rejected_batches,
    avg_proposed_batch_size: if verify_batches > 0 {
      proposed_tokens as f32 / verify_batches as f32
    } else {
      0.0
    },
    avg_accepted_batch_size: if verify_batches > 0 {
      accepted_tokens as f32 / verify_batches as f32
    } else {
      0.0
    },
    accepted_tokens_per_verify,
    fallback_tokens,
    fallback_decode_tokens,
    renderer_inserted_tokens,
    llm_generated_tokens,
    output_token_reduction_ratio: if renderer_inserted_tokens + llm_generated_tokens > 0 {
      renderer_inserted_tokens as f32 / (renderer_inserted_tokens + llm_generated_tokens) as f32
    } else {
      0.0
    },
  }
}

fn aggregate_strategy_perf_metrics(
  run_id: &str,
  prompt_hash: &str,
  case_hash: &str,
  records_hash: &str,
  model_config_hash: &str,
  mode: &str,
  cache_enabled: bool,
  total_e2e_ms: u64,
  final_answer: &str,
  stage_metrics: &[StrategyStagePerf],
) -> StrategyPerfMetrics {
  let ttft_ms = stage_metrics
    .iter()
    .filter_map(|item| item.ttft_ms)
    .filter(|value| *value > 0)
    .min()
    .or(Some(total_e2e_ms.max(1) as u128));
  let mut unique_models = HashSet::<String>::new();
  let mut memory_mb = 0_u64;
  for item in stage_metrics {
    if !item.model_path.is_empty() && unique_models.insert(item.model_path.clone()) {
      memory_mb = memory_mb.saturating_add(item.peak_memory_mb);
    }
  }
  let ttft_ms_u64 = ttft_ms.unwrap_or(total_e2e_ms.max(1) as u128).min(u64::MAX as u128) as u64;
  let total_output_tokens = strategy_approx_output_tokens(final_answer);
  let total_prompt_tokens = stage_metrics.iter().map(|item| item.prompt_tokens).max().unwrap_or(0);
  let stage_execution_sum_ms = stage_metrics.iter().map(|item| item.e2e_ms).sum::<u128>();
  let prompt_build_ms = stage_metrics.iter().map(|item| item.prompt_file_write_ms).sum::<u128>();
  let cache_lookup_ms = stage_metrics
    .iter()
    .map(|item| item.cache.token_cache_lookup_ms)
    .sum::<u128>();
  let process_spawn_ms = stage_metrics.iter().map(|item| item.process_spawn_ms).sum::<u128>();
  let stdout_read_ms = stage_metrics.iter().map(|item| item.stdout_read_ms).sum::<u128>();
  let postprocess_ms = stage_metrics.iter().map(|item| item.postprocess_ms).sum::<u128>();
  let orchestration_overhead_ms = total_e2e_ms.max(1) as u128 - stage_execution_sum_ms.min(total_e2e_ms.max(1) as u128);
  let e2e_tps = {
    let seconds = (total_e2e_ms.max(1) as f32) / 1000.0;
    ((total_output_tokens as f32) / seconds * 100.0).round() / 100.0
  };
  let total_decode_ms = stage_metrics.iter().filter_map(|item| item.decode_ms).sum::<u128>();
  let decode_tps = {
    let seconds = (total_decode_ms.max(1) as f32) / 1000.0;
    ((total_output_tokens as f32) / seconds * 100.0).round() / 100.0
  };
  let final_stage_tps = stage_metrics.last().map(|item| item.decode_tps).unwrap_or(0.0);
  let full_drace_applied_stages = stage_metrics
    .iter()
    .filter(|item| item.cache.synthetic_cache_applied || item.cache.cache_mode_applied == "FullDRACE")
    .map(|item| item.stage_name.clone())
    .collect::<Vec<_>>();
  let cache_summary = summarize_strategy_cache(stage_metrics, cache_enabled);
  StrategyPerfMetrics {
    mode: mode.to_string(),
    run_id: run_id.to_string(),
    prompt_hash: prompt_hash.to_string(),
    case_hash: case_hash.to_string(),
    records_hash: records_hash.to_string(),
    model_config_hash: model_config_hash.to_string(),
    backend_kind: cache_summary.backend_type.clone(),
    cache_requested: cache_summary.cache_requested,
    cache_loaded: cache_summary.cache_loaded,
    cache_applied: cache_summary.cache_applied,
    requested_mode: cache_summary.cache_mode_requested.clone(),
    applied_mode: cache_summary.cache_mode_applied.clone(),
    bypass_reason: cache_summary.bypass_reason.clone(),
    total_e2e_ms: total_e2e_ms.max(1) as u128,
    ttft_ms,
    total_prompt_tokens,
    total_output_tokens,
    e2e_tps,
    decode_tps,
    final_stage_tps,
    full_drace_applied_stages,
    peak_memory_mb: memory_mb,
    stage_execution_sum_ms,
    orchestration_overhead_ms,
    prompt_build_ms,
    cache_capability_ms: 0,
    cache_plan_ms: 0,
    cache_lookup_ms,
    prompt_file_write_ms: prompt_build_ms,
    process_spawn_ms,
    stdout_read_ms,
    postprocess_ms,
    other_overhead_ms: orchestration_overhead_ms,
    stages: stage_metrics.to_vec(),
    cache_summary,
  }
}

fn strategy_benchmark_results_path(app: &AppHandle) -> Result<PathBuf, String> {
  let dir = app
    .path()
    .app_data_dir()
    .map_err(|e| format!("벤치마크 결과 폴더를 찾지 못했어요: {e}"))?
    .join("benchmark_results");
  fs::create_dir_all(&dir).map_err(|e| format!("벤치마크 결과 폴더를 만들지 못했어요: {e}"))?;
  Ok(dir.join("rc_disputebench_runs.jsonl"))
}

fn strategy_benchmark_run_id() -> String {
  let millis = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|value| value.as_millis())
    .unwrap_or_default();
  format!("run-{}-{}", millis, std::process::id())
}

fn append_strategy_benchmark_rows(
  app: Option<&AppHandle>,
  run_id: &str,
  perf_metrics: &StrategyPerfMetrics,
) {
  let Some(app_handle) = app else {
    return;
  };
  let Ok(path) = strategy_benchmark_results_path(app_handle) else {
    return;
  };
  let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) else {
    return;
  };

  for stage in &perf_metrics.stages {
    let row = serde_json::json!({
      "run_id": run_id,
      "prompt_hash": perf_metrics.prompt_hash,
      "case_hash": perf_metrics.case_hash,
      "records_hash": perf_metrics.records_hash,
      "model_config_hash": perf_metrics.model_config_hash,
      "backend_kind": perf_metrics.backend_kind,
      "cache_requested": perf_metrics.cache_requested,
      "cache_loaded": stage.cache.cache_loaded,
      "cache_applied": stage.cache.cache_applied,
      "requested_mode": perf_metrics.requested_mode,
      "applied_mode": perf_metrics.applied_mode,
      "bypass_reason": perf_metrics.bypass_reason,
      "model_id": stage.model_id,
      "stage_name": stage.stage_name,
      "prompt_tokens": stage.prompt_tokens,
      "output_tokens": stage.output_tokens,
      "e2e_ms": stage.e2e_ms,
      "ttft_ms": stage.ttft_ms,
      "decode_ms": stage.decode_ms,
      "e2e_tps": stage.e2e_tps,
      "decode_tps": stage.decode_tps,
      "peak_memory_mb": stage.peak_memory_mb,
      "orchestration_overhead_ms": perf_metrics.orchestration_overhead_ms,
      "prompt_token_cache_supported": stage.cache.prompt_token_cache_supported,
      "prompt_token_cache_loaded": stage.cache.prompt_token_cache_loaded,
      "prompt_token_cache_applied": stage.cache.prompt_token_cache_applied,
      "prompt_token_cache_hit_ratio": stage.cache.prompt_token_cache_hit_ratio,
      "prefix_reuse_ratio": stage.cache.prefix_reuse_ratio,
      "prefix_kv_supported": stage.cache.prefix_kv_supported,
      "prefix_kv_applied": stage.cache.prefix_kv_applied,
      "token_cache_supported": stage.cache.token_cache_supported,
      "token_cache_loaded": stage.cache.token_cache_loaded,
      "token_cache_applied": stage.cache.token_cache_applied,
      "synthetic_cache_requested": stage.cache.synthetic_cache_requested,
      "synthetic_cache_supported": stage.cache.synthetic_cache_supported,
      "synthetic_cache_applied": stage.cache.synthetic_cache_applied,
      "draft_provider": stage.cache.draft_provider,
      "token_verification_supported": stage.cache.token_verification_supported,
      "token_cache_lookup_ms": stage.cache.token_cache_lookup_ms,
      "accepted_tokens_per_verify": stage.cache.accepted_tokens_per_verify,
      "proposed_tokens": stage.cache.proposed_tokens,
      "accepted_tokens": stage.cache.accepted_tokens,
      "rejected_tokens": stage.cache.rejected_tokens,
      "acceptance_ratio": stage.cache.acceptance_ratio,
      "verification_batches": stage.cache.verify_batches,
      "avg_proposed_batch_size": stage.cache.avg_proposed_batch_size,
      "avg_accepted_batch_size": stage.cache.avg_accepted_batch_size,
      "fallback_decode_tokens": stage.cache.fallback_decode_tokens,
      "renderer_inserted_tokens": stage.cache.renderer_inserted_tokens,
      "llm_generated_tokens": stage.cache.llm_generated_tokens,
      "output_token_reduction_ratio": stage.cache.output_token_reduction_ratio,
      "full_drace_applied_stages": perf_metrics.full_drace_applied_stages,
      "cache_bypass_reason": stage.cache.bypass_reason,
      "benchmark_phase": if perf_metrics.cache_summary.cache_requested && perf_metrics.cache_summary.cache_applied && !perf_metrics.cache_summary.cache_warm {
        "warmup"
      } else {
        "measured"
      },
    });
    let _ = writeln!(file, "{}", row);
  }
}

fn finalize_strategy_chat_run_result(
  app: Option<&AppHandle>,
  benchmark_run_id: &str,
  answer: String,
  model_path: String,
  runner: String,
  prompt_chars: usize,
  records_used: usize,
  retrieval_query: String,
  evidence_packet: StrategyEvidencePacket,
  perf_metrics: StrategyPerfMetrics,
) -> StrategyChatRunResult {
  append_strategy_benchmark_rows(app, benchmark_run_id, &perf_metrics);
  StrategyChatRunResult {
    answer,
    model_path,
    runner,
    prompt_chars,
    records_used,
    retrieval_query,
    evidence_packet,
    perf_metrics,
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrategyQuestionRoute {
  FastRoosy,
  Hybrid,
}

fn strategy_question_is_comparison(message: &str) -> bool {
  let compact = strategy_compact_text(message);
  [
    "누가가장문제",
    "누가더문제",
    "누가문제",
    "누가더잘못",
    "누가잘못",
    "누가더책임",
    "책임은누구",
    "가해자",
    "a의잘못",
    "b의잘못",
    "비하면어떰",
    "비하면어때",
  ]
  .iter()
  .any(|keyword| compact.contains(keyword))
}

fn strategy_question_is_message_drafting(message: &str) -> bool {
  let compact = strategy_compact_text(message);
  [
    "뭐라고말",
    "어떻게말",
    "어떻게정리",
    "답장",
    "문자",
    "보낼지",
    "써줘",
  ]
  .iter()
  .any(|keyword| compact.contains(keyword))
}

fn strategy_question_route(message: &str) -> StrategyQuestionRoute {
  let compact = strategy_compact_text(message);
  let is_trivial_smalltalk = [
    "안녕",
    "반가워",
    "고마워",
    "감사",
    "오케이",
    "확인",
    "좋아",
  ]
  .iter()
  .any(|keyword| compact.contains(keyword))
    && compact.chars().count() <= 12;

  if is_trivial_smalltalk {
    StrategyQuestionRoute::FastRoosy
  } else {
    StrategyQuestionRoute::Hybrid
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StrategyEvidenceRecord {
  pub ref_id: String,
  pub record_id: String,
  pub ts: String,
  pub actor: String,
  pub place: String,
  pub store: String,
  pub summary: String,
  pub score: f32,
  #[serde(default)]
  pub risk_label: String,
  #[serde(default)]
  pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StrategyEvidencePacket {
  #[serde(default)]
  pub mode: String,
  #[serde(default)]
  pub case_title: String,
  #[serde(default)]
  pub focus_summary: String,
  #[serde(default)]
  pub overview: String,
  #[serde(default)]
  pub actor_summary: Vec<String>,
  #[serde(default)]
  pub timeline_summary: Vec<String>,
  #[serde(default)]
  pub risk_summary: Vec<String>,
  #[serde(default)]
  pub gaps: Vec<String>,
  #[serde(default)]
  pub evidence_records: Vec<StrategyEvidenceRecord>,
  #[serde(default)]
  pub legal_references: Vec<StrategyLegalReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StrategyLegalReference {
  pub ref_id: String,
  pub law_id: String,
  pub law_name: String,
  #[serde(default)]
  pub short_name: String,
  #[serde(default)]
  pub article_ref: String,
  #[serde(default)]
  pub article_title: String,
  #[serde(default)]
  pub legal_point: String,
  #[serde(default)]
  pub teacher_use_case: String,
  #[serde(default)]
  pub source_url: String,
  #[serde(default)]
  pub status_label: String,
  #[serde(default)]
  pub relevance_reasons: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyLegalDataset {
  retrieval_boosters: StrategyLegalRetrievalBoosters,
  records: Vec<StrategyLegalLawRecord>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyLegalRetrievalBoosters {
  concept_map: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyLegalLawRecord {
  record_id: String,
  official_name: String,
  short_name: String,
  current_status_label: String,
  source_url: String,
  school_relevance: String,
  rag: StrategyLegalLawRag,
  key_articles: Vec<StrategyLegalArticle>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyLegalLawRag {
  aliases: Vec<String>,
  topical_tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyLegalArticle {
  article_no: String,
  article_title: String,
  legal_point: String,
  teacher_use_case: String,
  keywords: Vec<String>,
  retrieval_text: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyLegalFlatChunk {
  record_id: String,
  official_name: String,
  short_name: String,
  current_status_label: String,
  source_url: String,
  school_relevance: String,
  topical_tags: Vec<String>,
  aliases: Vec<String>,
  chunk_type: String,
  chunk_id: String,
  article_no: String,
  article_title: String,
  legal_point: String,
  teacher_use_case: String,
  keywords: Vec<String>,
  retrieval_text: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyRecordTemplateDataset {
  record_mode: StrategyRecordModeTemplate,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyRecordModeTemplate {
  schema_version: String,
  purpose: String,
  principles: Vec<String>,
  quality_checklist: Vec<String>,
  forbidden: Vec<String>,
  section_guides: Vec<StrategyRecordSectionGuide>,
  output_contract: StrategyRecordOutputContract,
  input_grounding_rules: Vec<String>,
  risk_sensitive_rules: HashMap<String, Vec<String>>,
  completion_questions: Vec<String>,
  evaluation_rubric: HashMap<String, StrategyRecordEvaluationCriterion>,
  dynamic_balancing_hints: StrategyRecordDynamicBalancingHints,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyRecordSectionGuide {
  section: String,
  objective: String,
  include_points: Vec<String>,
  writing_rules: Vec<String>,
  missing_value_policy: Vec<String>,
  quality_signals: Vec<String>,
  example_phrases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyRecordOutputContract {
  language: String,
  tone: String,
  default_format: String,
  section_order: Vec<String>,
  citation_policy: String,
  legal_policy: String,
  teacher_confirmation_policy: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyRecordEvaluationCriterion {
  weight: f32,
  description: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct StrategyRecordDynamicBalancingHints {
  roosy_x_preferred_when: Vec<String>,
  hyperclova_x_preferred_when: Vec<String>,
  parallel_or_guard_required_when: Vec<String>,
  router_features: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StrategyProgressPayload {
  stage: String,
  message: String,
}

#[derive(Debug, Clone)]
struct StrategyRecordGenerationProfile {
  sensitivity_key: String,
  hyper_focus: Vec<String>,
  roosy_focus: Vec<String>,
  guard_required: bool,
  submission_intent: bool,
}

fn strategy_trim(s: &str, limit: usize) -> String {
  s.chars().take(limit).collect::<String>()
}

fn strategy_is_supported_char(ch: char) -> bool {
  let cp = ch as u32;
  if cp > 0xFFFF {
    return false;
  }
  if (0xE000..=0xF8FF).contains(&cp) {
    return false;
  }
  if (0xFDD0..=0xFDEF).contains(&cp) {
    return false;
  }
  if (cp & 0xFFFF) == 0xFFFE || (cp & 0xFFFF) == 0xFFFF {
    return false;
  }
  true
}

fn strategy_sanitize_text(input: &str) -> String {
  let mut out = String::with_capacity(input.len());
  let mut prev_blank = false;
  for ch in input.chars() {
    if ch == '\u{fffd}' {
      continue;
    }
    if !strategy_is_supported_char(ch) {
      continue;
    }
    if ch == '\r' {
      continue;
    }
    let normalized = if ch == '\t' { ' ' } else { ch };
    if normalized == '\n' {
      if prev_blank {
        continue;
      }
      out.push('\n');
      prev_blank = true;
      continue;
    }
    if normalized.is_control() {
      continue;
    }
    if normalized.is_whitespace() {
      out.push(' ');
      prev_blank = false;
      continue;
    }
    out.push(normalized);
    prev_blank = false;
  }
  out.trim().to_string()
}

fn strategy_compact_text(input: &str) -> String {
  strategy_sanitize_text(input)
    .chars()
    .filter(|ch| !ch.is_whitespace())
    .collect::<String>()
}

fn strategy_question_focus_hint(message: &str) -> Option<String> {
  if strategy_question_is_comparison(message) {
    return Some(
      "- 이번 질문은 책임·잘못 비교 요청이다.\n- 첫 문장에서 바로 비교 결론을 답하라.\n- 현재 증거만으로 한쪽이 더 문제라고 단정하기 어렵다면 그 점을 첫 문장에서 분명히 말하라.\n- 증거 목록 나열보다 결론 → 이유 → 바로 쓸 말 순서로 정리하라.".to_string(),
    );
  }

  if strategy_question_is_message_drafting(message) {
    return Some(
      "- 이번 질문은 바로 전달할 문장을 원하는 요청이다.\n- 첫 문단에서 상황판단을 짧게 말한 뒤, 바로 복사해 쓸 수 있는 문장을 먼저 제시하라.".to_string(),
    );
  }

  None
}

fn strategy_question_needs_legal_refs(message: &str) -> bool {
  let compact = strategy_compact_text(message);
  [
    "법",
    "법적",
    "조문",
    "근거",
    "규정",
    "법령",
    "위법",
    "처벌",
    "고소",
    "신고",
  ]
  .iter()
  .any(|keyword| compact.contains(keyword))
}

fn write_strategy_prompt_file(prefix: &str, content: &str) -> Result<PathBuf, String> {
  let dir = std::env::temp_dir().join("roosycozy_strategy");
  fs::create_dir_all(&dir).map_err(|e| format!("전략자문 임시 폴더를 만들지 못했어요: {e}"))?;
  let stamp = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_millis();
  let name = format!("{}_{}_{}.txt", prefix, std::process::id(), stamp);
  let path = dir.join(name);
  fs::write(&path, content.as_bytes()).map_err(|e| format!("전략자문 임시 프롬프트를 쓰지 못했어요: {e}"))?;
  Ok(path)
}

fn cleanup_strategy_prompt_file(path: &Path) {
  let _ = fs::remove_file(path);
}

fn strategy_fit_prompt_to_budget(input: &str, n_ctx: u32, max_tokens: u32) -> String {
  let budget = (n_ctx as usize).saturating_sub(max_tokens as usize + 320).max(1400);
  let char_count = input.chars().count();
  if char_count <= budget {
    return input.to_string();
  }

  let head_len = ((budget as f32) * 0.68) as usize;
  let tail_len = budget.saturating_sub(head_len + 18);
  let head = input.chars().take(head_len).collect::<String>();
  let tail_chars = input.chars().rev().take(tail_len).collect::<Vec<_>>();
  let tail = tail_chars.into_iter().rev().collect::<String>();
  format!("{}\n\n[중간 맥락 일부 압축]\n\n{}", head.trim_end(), tail.trim_start())
}

fn strategy_strip_prompt_echo(input: &str) -> String {
  const MARKERS: [&str; 25] = [
    "[현재 사건 맥락]",
    "[증거 패킷 요약]",
    "[핵심 인물]",
    "[시간 흐름]",
    "[비어 있는 정보]",
    "[증거 참조표]",
    "[관련 법령 참조표]",
    "[전략 메모]",
    "[직전 대화]",
    "[이번 요청]",
    "[응답 조건]",
    "[중간 맥락 일부 압축]",
    "[질문]",
    "[핵심 근거]",
    "[관련 법령]",
    "[HyperCLOVA-X 초안]",
    "[Roosy-X 초안]",
    "[합성 지침]",
    "[추가 정보]",
    "[관련 법령/참고 기준]",
    "[기록 작성 템플릿]",
    "[핵심 원칙]",
    "[품질 점검표]",
    "[금지 사항]",
    "[섹션별 가이드]",
  ];

  let mut cut = input.len();
  for marker in MARKERS {
    if let Some(idx) = input.find(marker) {
      if idx > 80 && idx < cut {
        cut = idx;
      }
    }
  }
  input[..cut].trim_end().to_string()
}

const STRATEGY_RECORD_SECTION_HEADERS: [&str; 7] = [
  "[기록 기본정보]",
  "[상황 요약]",
  "[배경 흐름]",
  "[핵심 포인트]",
  "[관련 자료]",
  "[내 대응 메모]",
  "[추가 메모]",
];

fn strategy_is_record_section_header(line: &str) -> bool {
  STRATEGY_RECORD_SECTION_HEADERS.iter().any(|header| line == *header)
}

fn strategy_is_record_tail_noise(line: &str) -> bool {
  let trimmed = line.trim();
  trimmed.starts_with("관련 법령으로는 ")
    || trimmed.starts_with("추가로 ")
    || trimmed.starts_with("참고로 지금 판단의 중심 근거는 ")
    || trimmed.starts_with("로컬 브리핑 지표:")
    || trimmed.starts_with("[관련 법령]")
    || trimmed.starts_with("[합성 지침]")
    || trimmed.starts_with("[추가 정보]")
    || trimmed == "기록 작성 기준"
    || trimmed == "- 핵심 원칙"
    || trimmed == "- 입력 grounding 규칙"
    || trimmed == "- 품질 점검"
    || trimmed == "- 평가 매트릭"
    || trimmed == "- 보강 질문"
}

fn strategy_trim_repeated_record_halves(input: &str) -> String {
  let paragraphs = input
    .split("\n\n")
    .map(str::trim)
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>();
  if paragraphs.len() >= 2 && paragraphs.len() % 2 == 0 {
    let half = paragraphs.len() / 2;
    let first = paragraphs[..half]
      .iter()
      .map(|part| strategy_compact_text(part))
      .collect::<Vec<_>>();
    let second = paragraphs[half..]
      .iter()
      .map(|part| strategy_compact_text(part))
      .collect::<Vec<_>>();
    if first == second {
      return paragraphs[..half].join("\n\n");
    }
  }
  input.trim().to_string()
}

fn strategy_dedupe_record_sections(input: &str) -> String {
  let mut seen = HashSet::<String>::new();
  let mut lines = Vec::<String>::new();
  let mut current_section: Option<String> = None;

  for raw_line in input.lines() {
    let trimmed = raw_line.trim();
    if trimmed.is_empty() {
      if lines.last().is_some_and(|line| !line.is_empty()) {
        lines.push(String::new());
      }
      continue;
    }

    if strategy_is_record_section_header(trimmed) {
      current_section = Some(trimmed.to_string());
      if seen.insert(trimmed.to_string()) {
        if lines.last().is_some_and(|line| !line.is_empty()) {
          lines.push(String::new());
        }
        lines.push(trimmed.to_string());
      } else {
        current_section = None;
      }
      continue;
    }

    if strategy_is_record_tail_noise(trimmed) {
      continue;
    }

    if current_section.is_some() {
      let normalized = strategy_sanitize_text(trimmed);
      if !normalized.is_empty() {
        lines.push(normalized);
      }
    }
  }

  while lines.last().is_some_and(|line| line.is_empty()) {
    lines.pop();
  }

  if lines.is_empty() {
    input.trim().to_string()
  } else {
    lines.join("\n")
  }
}

fn finalize_strategy_record_answer(
  answer: &str,
  evidence_packet: &StrategyEvidencePacket,
  message: &str,
) -> String {
  let cleaned = strategy_strip_prompt_echo(answer.trim());
  let cleaned = strategy_trim_repeated_record_halves(&cleaned);
  let cleaned = strategy_dedupe_record_sections(&cleaned);
  let cleaned = strategy_trim(cleaned.trim(), 5200);
  if strategy_record_answer_has_required_sections(&cleaned) {
    cleaned
  } else {
    build_strategy_record_fallback_answer(evidence_packet, message)
  }
}

#[derive(Debug, Clone)]
struct StrategyRecordReviewResult {
  answer: String,
  metrics: Option<StrategyStagePerf>,
  renderer_stats: Option<StrategyRecordRendererStats>,
}

#[derive(Debug, Clone)]
struct StrategyRecordRendererStats {
  synthetic_cache_requested: bool,
  synthetic_cache_supported: bool,
  synthetic_cache_applied: bool,
  draft_provider: String,
  proposed_tokens: usize,
  accepted_tokens: usize,
  rejected_tokens: usize,
  verification_batches: usize,
  fallback_decode_tokens: usize,
  renderer_inserted_tokens: usize,
  llm_generated_tokens: usize,
  output_token_reduction_ratio: f32,
}

#[derive(Debug, Clone, Default)]
struct StrategyVerifiedDraftPrefix {
  accepted_text: String,
  proposed_tokens: usize,
  accepted_tokens: usize,
  rejected_tokens: usize,
  verification_batches: usize,
  accepted_tokens_per_verify: f32,
  synthetic_applied: bool,
  bypass_reason: String,
}

fn strategy_build_draft_provider_state(
  stage: HybridStage,
  model_id: &str,
  structured_json: bool,
  prompt_hash: Option<String>,
) -> DraftProviderState {
  DraftProviderState {
    stage: stage.as_str().to_string(),
    model_id: model_id.to_string(),
    structured_json,
    prompt_hash,
  }
}

fn strategy_build_draft_proposal(
  stage: HybridStage,
  provider_name: &str,
  model_id: &str,
  structured_json: bool,
  prompt_hash: Option<String>,
  max_draft_tokens: usize,
) -> DraftProposal {
  let state = strategy_build_draft_provider_state(stage, model_id, structured_json, prompt_hash);
  let normalized_provider = provider_name.trim().to_ascii_lowercase();
  if normalized_provider == "template" {
    return TemplateDraftProvider::proposal_for_stage(stage, model_id, max_draft_tokens.max(1));
  }
  let provider = NoopDraftProvider;
  DraftProposal {
    provider_name: provider.name().to_string(),
    token_ids: provider.propose(&state, max_draft_tokens),
    rendered_fragments: provider.rendered_fragments(&state),
  }
}

fn strategy_draft_token_ids_to_text(token_ids: &[u32]) -> String {
  token_ids
    .iter()
    .filter_map(|id| char::from_u32(*id))
    .collect::<String>()
}

fn strategy_verify_draft_prefix_via_llama_server(
  app: Option<&AppHandle>,
  backend_runtime: &StrategyBackendRuntime,
  model_id: &str,
  stage: HybridStage,
  system_prompt: &str,
  user_prompt: &str,
  n_ctx: u32,
  threads: u32,
  max_tokens: u32,
  draft_proposal: &DraftProposal,
) -> StrategyVerifiedDraftPrefix {
  let mut result = StrategyVerifiedDraftPrefix {
    proposed_tokens: draft_proposal.token_ids.len(),
    ..StrategyVerifiedDraftPrefix::default()
  };
  if draft_proposal.token_ids.is_empty()
    || !backend_runtime.capabilities.supports_token_verification
  {
    result.rejected_tokens = result.proposed_tokens;
    return result;
  }
  if !matches!(
    backend_runtime.capabilities.backend_kind,
    BackendKind::LlamaServer | BackendKind::Native
  ) {
    result.rejected_tokens = result.proposed_tokens;
    return result;
  }
  let model_path = match resolve_strategy_model_path(app, model_id) {
    Ok(path) => path,
    Err(_) => {
      result.rejected_tokens = result.proposed_tokens;
      return result;
    }
  };
  let use_native_verifier = matches!(backend_runtime.capabilities.backend_kind, BackendKind::Native)
    && backend_runtime.capabilities.supports_token_verification;
  let (endpoint, slot, cache_prompt, request_timeout_ms) = if matches!(backend_runtime.capabilities.backend_kind, BackendKind::LlamaServer) {
    let Some(llama_server) = backend_runtime.llama_server.as_ref() else {
      result.rejected_tokens = result.proposed_tokens;
      return result;
    };
    let (endpoint, base_slot) = effective_llama_server_endpoint_for_model(llama_server, model_id);
    (
      endpoint,
      llama_server_slot_for_stage(base_slot, stage),
      llama_server.cache_prompt,
      llama_server.request_timeout_ms.min(12_000),
    )
  } else {
    (String::new(), None, false, 12_000)
  };
  let prompt = format!("{}\n\n{}", system_prompt.trim(), user_prompt.trim());
  let max_batch = draft_proposal.token_ids.len().clamp(8, 16);
  let mut offset = 0usize;
  let mut accepted_text = String::new();
  let session_backend = if use_native_verifier {
    EitherVerifyBackend::Native(NativeSessionBackend)
  } else {
    EitherVerifyBackend::Llama(LlamaServerSessionBackend)
  };
  let base_options = GenerationSessionOptions {
    model_id: model_id.to_string(),
    endpoint: endpoint.clone(),
    model_path: Some(model_path.display().to_string()),
    slot,
    cache_prompt,
    assistant_prefix: None,
    n_ctx: Some(n_ctx.max(3584)),
    threads: Some(threads),
    max_tokens: max_tokens.max(1),
    temperature: 0.0,
    top_p: 1.0,
    repeat_penalty: 1.0,
    request_timeout_ms,
  };

  match session_backend {
    EitherVerifyBackend::Native(backend) => {
      let session = match backend.open_session(&prompt, base_options) {
        Ok(value) => value,
        Err(_) => {
          result.rejected_tokens = result.proposed_tokens;
          return result;
        }
      };
      while offset < draft_proposal.token_ids.len() {
        let batch_end = (offset + max_batch).min(draft_proposal.token_ids.len());
        let batch = &draft_proposal.token_ids[offset..batch_end];
        result.verification_batches += 1;
        let verify = backend.verify_draft(&session, batch);
        let Ok(verify) = verify else {
          break;
        };
        result.accepted_tokens += verify.accepted_len;
        accepted_text.push_str(&verify.accepted_text);
        if verify.accepted_len < batch.len() {
          result.rejected_tokens += batch.len() - verify.accepted_len;
          break;
        }
        offset = batch_end;
      }
      let _ = backend.close_session(&session);
    }
    EitherVerifyBackend::Llama(backend) => {
      while offset < draft_proposal.token_ids.len() {
        let batch_end = (offset + max_batch).min(draft_proposal.token_ids.len());
        let batch = &draft_proposal.token_ids[offset..batch_end];
        let mut options = base_options.clone();
        if !accepted_text.is_empty() {
          options.assistant_prefix = Some(accepted_text.clone());
        }
        let session = match backend.open_session(&prompt, options) {
          Ok(value) => value,
          Err(_) => break,
        };
        result.verification_batches += 1;
        let verify = backend.verify_draft(&session, batch);
        let _ = backend.close_session(&session);
        let Ok(verify) = verify else {
          break;
        };
        result.accepted_tokens += verify.accepted_len;
        accepted_text.push_str(&verify.accepted_text);
        if verify.accepted_len < batch.len() {
          result.rejected_tokens += batch.len() - verify.accepted_len;
          break;
        }
        offset = batch_end;
      }
    }
  }

  if offset < draft_proposal.token_ids.len() {
    result.rejected_tokens = draft_proposal.token_ids.len().saturating_sub(result.accepted_tokens);
  }
  result.accepted_tokens_per_verify = if result.verification_batches > 0 {
    result.accepted_tokens as f32 / result.verification_batches as f32
  } else {
    0.0
  };
  if result.verification_batches > 0 && result.accepted_tokens_per_verify < 2.0 {
    result.synthetic_applied = false;
    result.accepted_text = String::new();
    result.bypass_reason = format!(
      "synthetic_low_acceptance:{:.2}_tokens_per_verify; fallback=PrefixKV",
      result.accepted_tokens_per_verify
    );
  } else {
    result.synthetic_applied = result.accepted_tokens > 0;
    result.accepted_text = accepted_text.clone();
  }
  if result.synthetic_applied {
    emit_strategy_progress(
      app,
      "점검",
      format!(
        "DRaCE verification draft가 {}토큰 정도 먼저 맞아 들어가 resident 생성 앞단에 붙였어요.",
        result.accepted_tokens
      ),
    );
  } else if result.proposed_tokens > 0 && result.verification_batches > 0 && !result.bypass_reason.is_empty() {
    emit_strategy_progress(app, "점검", format!("Synthetic draft 수용률이 낮아 PrefixKV로 안전하게 우회할게요. {}", result.bypass_reason));
  }
  result
}

enum EitherVerifyBackend {
  Native(NativeSessionBackend),
  Llama(LlamaServerSessionBackend),
}

fn apply_strategy_record_renderer_stats(stages: &mut [StrategyStagePerf], stats: &StrategyRecordRendererStats) {
  let Some(last_stage) = stages.last_mut() else {
    return;
  };
  last_stage.cache.synthetic_cache_requested = stats.synthetic_cache_requested;
  last_stage.cache.synthetic_cache_supported = stats.synthetic_cache_supported;
  last_stage.cache.synthetic_cache_applied = stats.synthetic_cache_applied;
  last_stage.cache.draft_provider = stats.draft_provider.clone();
  last_stage.cache.proposed_tokens = stats.proposed_tokens;
  last_stage.cache.accepted_tokens = stats.accepted_tokens;
  last_stage.cache.rejected_tokens = stats.rejected_tokens;
  last_stage.cache.verify_batches = stats.verification_batches;
  last_stage.cache.fallback_decode_tokens = stats.fallback_decode_tokens;
  last_stage.cache.acceptance_ratio = if stats.proposed_tokens > 0 {
    stats.accepted_tokens as f32 / stats.proposed_tokens as f32
  } else {
    0.0
  };
  last_stage.cache.avg_proposed_batch_size = stats.proposed_tokens as f32;
  last_stage.cache.avg_accepted_batch_size = stats.accepted_tokens as f32;
  last_stage.cache.accepted_tokens_per_verify = if stats.verification_batches > 0 {
    stats.accepted_tokens as f32 / stats.verification_batches as f32
  } else {
    0.0
  };
  if stats.synthetic_cache_applied {
    last_stage.cache.cache_applied = true;
    last_stage.cache.cache_mode_applied = "FullDRACE".to_string();
    last_stage.cache.cache_loaded = true;
  } else if last_stage.cache.prefix_kv_applied {
    last_stage.cache.cache_applied = true;
    last_stage.cache.cache_mode_applied = "PrefixKV+TemplateRenderer".to_string();
    last_stage.cache.cache_loaded = true;
  }
  last_stage.cache.renderer_inserted_tokens = stats.renderer_inserted_tokens;
  last_stage.cache.llm_generated_tokens = stats.llm_generated_tokens.max(last_stage.output_tokens);
  last_stage.cache.output_token_reduction_ratio = stats.output_token_reduction_ratio;
}

fn finalize_strategy_record_stage_metrics(
  mut stages: Vec<StrategyStagePerf>,
  review: StrategyRecordReviewResult,
) -> Vec<StrategyStagePerf> {
  if let Some(renderer_stats) = &review.renderer_stats {
    apply_strategy_record_renderer_stats(&mut stages, renderer_stats);
  }
  if let Some(review_stage) = review.metrics {
    stages.push(review_stage);
  }
  stages
}

fn strategy_try_render_record_without_llm(
  app: Option<&AppHandle>,
  evidence_packet: &StrategyEvidencePacket,
  message: &str,
  draft_answer: &str,
  synthetic_cache_enabled: bool,
  progress_stage: &str,
) -> Option<StrategyRecordReviewResult> {
  let review_stage_output_target = strategy_approx_output_tokens(draft_answer).max(160);
  let review_plan = DraceCacheManager::global().plan_stage(
    synthetic_cache_enabled,
    HybridStage::RecordReview,
    strategy_approx_output_tokens(draft_answer).max(160),
    review_stage_output_target,
  );
  if !review_plan.use_template_renderer {
    return None;
  }
  let renderer_draft_provider = if review_plan.draft_provider.trim().is_empty() {
    "noop".to_string()
  } else {
    review_plan.draft_provider.clone()
  };
  let draft_proposal = strategy_build_draft_proposal(
    HybridStage::RecordReview,
    &renderer_draft_provider,
    STRATEGY_MODEL_DEFAULT_ID,
    true,
    Some(strategy_fast_hash_hex(draft_answer)),
    review_plan.max_candidate_tokens.max(8),
  );
  let structured =
    strategy_build_structured_report_from_record_sections(evidence_packet, message, draft_answer)?;
  let llm_generated_tokens = strategy_estimate_structured_report_tokens(&structured).max(1);
  let (rendered, renderer_inserted_tokens, renderer_generated_tokens) =
    strategy_render_record_structured_answer(&structured, evidence_packet, message, draft_answer);
  let output_token_reduction_ratio = if renderer_inserted_tokens + renderer_generated_tokens > 0 {
    renderer_inserted_tokens as f32 / (renderer_inserted_tokens + renderer_generated_tokens) as f32
  } else {
    0.0
  };
  emit_strategy_progress(
    app,
    progress_stage,
    format!(
      "고정 섹션 약 {}토큰은 앱에서 바로 렌더링할 수 있어 Roosy/합성 단계를 줄였어요.",
      renderer_inserted_tokens
    ),
  );
  Some(StrategyRecordReviewResult {
    answer: rendered,
    metrics: None,
    renderer_stats: Some(StrategyRecordRendererStats {
      synthetic_cache_requested: review_plan.synthetic_cache_requested,
      synthetic_cache_supported: review_plan.synthetic_cache_supported,
      synthetic_cache_applied: false,
      draft_provider: renderer_draft_provider,
      proposed_tokens: draft_proposal.token_ids.len(),
      accepted_tokens: 0,
      rejected_tokens: draft_proposal.token_ids.len(),
      verification_batches: 0,
      fallback_decode_tokens: 0,
      renderer_inserted_tokens,
      llm_generated_tokens: renderer_generated_tokens.max(llm_generated_tokens),
      output_token_reduction_ratio,
    }),
  })
}

fn maybe_review_strategy_record_answer(
  app: Option<&AppHandle>,
  backend_runtime: &StrategyBackendRuntime,
  evidence_packet: &StrategyEvidencePacket,
  case_item: Option<&CaseItem>,
  message: &str,
  strategy_note: Option<&str>,
  draft_answer: &str,
  n_ctx: u32,
  max_tokens: u32,
  threads: u32,
  synthetic_cache_enabled: bool,
) -> StrategyRecordReviewResult {
  if let Some(rendered) = strategy_try_render_record_without_llm(
    app,
    evidence_packet,
    message,
    draft_answer,
    synthetic_cache_enabled,
    "점검",
  ) {
    return rendered;
  }
  let review_stage_output_target = strategy_approx_output_tokens(draft_answer).max(160);
  let review_plan = DraceCacheManager::global().plan_stage(
    synthetic_cache_enabled,
    HybridStage::RecordReview,
    strategy_approx_output_tokens(draft_answer).max(160),
    review_stage_output_target,
  );
  let renderer_draft_provider = if review_plan.draft_provider.trim().is_empty() {
    "noop".to_string()
  } else {
    review_plan.draft_provider.clone()
  };
  let draft_proposal = strategy_build_draft_proposal(
    HybridStage::RecordReview,
    &renderer_draft_provider,
    STRATEGY_MODEL_DEFAULT_ID,
    true,
    Some(strategy_fast_hash_hex(draft_answer)),
    review_plan.max_candidate_tokens.max(8),
  );
  let renderer_system_prompt = build_strategy_record_renderer_system_prompt();
  if review_plan.use_template_renderer {
    emit_strategy_progress(
      app,
      "점검",
      "HyperCLOVA-X가 구조화된 JSON만 짧게 만들고, 고정 섹션은 앱이 안전하게 렌더링할게요.",
    );
    let renderer_user_prompt =
      build_strategy_record_renderer_user_prompt(evidence_packet, case_item, message, strategy_note, draft_answer);
    let verified_draft = strategy_verify_draft_prefix_via_llama_server(
      app,
      backend_runtime,
      STRATEGY_MODEL_DEFAULT_ID,
      HybridStage::RecordReview,
      &renderer_system_prompt,
      &renderer_user_prompt,
      n_ctx,
      threads,
      max_tokens.max(260).clamp(260, 360),
      &draft_proposal,
    );
    let (execute_review, verified_prefix_used) = if verified_draft.synthetic_applied {
      match execute_strategy_model_with_prefix(
        app,
        backend_runtime,
        STRATEGY_MODEL_DEFAULT_ID,
        HybridStage::RecordReview,
        &renderer_system_prompt,
        &renderer_user_prompt,
        evidence_packet,
        n_ctx.max(3584),
        max_tokens.max(260).clamp(260, 360),
        threads,
        "점검",
        synthetic_cache_enabled,
        Some(verified_draft.accepted_text.as_str()),
      ) {
        Ok(reviewed) => (Ok(reviewed), true),
        Err(err) => {
          emit_strategy_progress(
            app,
            "점검",
            format!(
              "검증 prefix 경로가 잠시 흔들려 PrefixKV 경로로 이어갈게요. {}",
              strategy_trim(&err, 220)
            ),
          );
          (
            execute_strategy_model(
              app,
              backend_runtime,
              STRATEGY_MODEL_DEFAULT_ID,
              HybridStage::RecordReview,
              &renderer_system_prompt,
              &renderer_user_prompt,
              evidence_packet,
              n_ctx.max(3584),
              max_tokens.max(260).clamp(260, 360),
              threads,
              "점검",
              synthetic_cache_enabled,
            ),
            false,
          )
        }
      }
    } else {
      (
        execute_strategy_model(
          app,
          backend_runtime,
          STRATEGY_MODEL_DEFAULT_ID,
          HybridStage::RecordReview,
          &renderer_system_prompt,
          &renderer_user_prompt,
          evidence_packet,
          n_ctx.max(3584),
          max_tokens.max(260).clamp(260, 360),
          threads,
          "점검",
          synthetic_cache_enabled,
        ),
        false,
      )
    };
    match execute_review {
      Ok(mut reviewed) => {
        if let Some(structured) = strategy_parse_structured_report_draft(&reviewed.answer) {
          let synthetic_applied = verified_prefix_used && verified_draft.synthetic_applied;
          let accepted_tokens = verified_draft.accepted_tokens;
          let rejected_tokens = if verified_draft.proposed_tokens > 0 {
            verified_draft.rejected_tokens
          } else {
            draft_proposal.token_ids.len()
          };
          let verification_batches = verified_draft.verification_batches;
          let llm_generated_tokens = reviewed.metrics.output_tokens.max(strategy_approx_output_tokens(&reviewed.answer));
          let (rendered, renderer_inserted_tokens, renderer_generated_tokens) =
            strategy_render_record_structured_answer(&structured, evidence_packet, message, draft_answer);
          let output_token_reduction_ratio = if renderer_inserted_tokens + renderer_generated_tokens > 0 {
            renderer_inserted_tokens as f32 / (renderer_inserted_tokens + renderer_generated_tokens) as f32
          } else {
            0.0
          };
          reviewed.answer = rendered.clone();
          reviewed.metrics.cache.synthetic_cache_requested = review_plan.synthetic_cache_requested;
          reviewed.metrics.cache.synthetic_cache_supported = review_plan.synthetic_cache_supported;
          reviewed.metrics.cache.synthetic_cache_applied = synthetic_applied;
          reviewed.metrics.cache.cache_loaded = review_plan.cache_loaded;
          reviewed.metrics.cache.cache_applied = review_plan.cache_applied;
          reviewed.metrics.cache.cache_mode_requested = review_plan.cache_mode_requested.clone();
          reviewed.metrics.cache.cache_mode_applied = if synthetic_applied {
            "FullDRACE".to_string()
          } else if reviewed.metrics.cache.prefix_kv_applied {
            "PrefixKV+TemplateRenderer".to_string()
          } else {
            "Off".to_string()
          };
          reviewed.metrics.cache.bypass_reason = if synthetic_applied {
            String::new()
          } else if !verified_draft.bypass_reason.trim().is_empty() {
            verified_draft.bypass_reason.clone()
          } else if reviewed.metrics.cache.prefix_kv_applied {
            "synthetic_token_cache_verification_disabled_by_default; fallback=PrefixKV+TemplateRenderer".to_string()
          } else {
            review_plan.bypass_reason.clone()
          };
          reviewed.metrics.cache.draft_provider = renderer_draft_provider.clone();
          reviewed.metrics.cache.token_cache_applied = synthetic_applied;
          reviewed.metrics.cache.proposed_tokens = draft_proposal.token_ids.len();
          reviewed.metrics.cache.accepted_tokens = accepted_tokens;
          reviewed.metrics.cache.rejected_tokens = rejected_tokens;
          reviewed.metrics.cache.acceptance_ratio = if draft_proposal.token_ids.is_empty() {
            0.0
          } else {
            accepted_tokens as f32 / draft_proposal.token_ids.len() as f32
          };
          reviewed.metrics.cache.verify_batches = verification_batches;
          reviewed.metrics.cache.rejected_batches = if rejected_tokens > 0 { 1 } else { 0 };
          reviewed.metrics.cache.avg_proposed_batch_size = draft_proposal.token_ids.len() as f32;
          reviewed.metrics.cache.avg_accepted_batch_size = if verification_batches > 0 {
            accepted_tokens as f32 / verification_batches as f32
          } else {
            0.0
          };
          reviewed.metrics.cache.accepted_tokens_per_verify = verified_draft.accepted_tokens_per_verify;
          reviewed.metrics.cache.fallback_tokens = llm_generated_tokens;
          reviewed.metrics.cache.fallback_decode_tokens = llm_generated_tokens.saturating_sub(accepted_tokens);
          reviewed.metrics.cache.renderer_inserted_tokens = renderer_inserted_tokens;
          reviewed.metrics.cache.llm_generated_tokens = renderer_generated_tokens.max(llm_generated_tokens);
          reviewed.metrics.cache.output_token_reduction_ratio = output_token_reduction_ratio;
          let reviewed_llm_generated_tokens = reviewed.metrics.cache.llm_generated_tokens;
          emit_strategy_progress(
            app,
            "점검",
            format!(
              "템플릿 렌더러로 고정 섹션 {}토큰 정도를 앱에서 채우고, 모델은 핵심 JSON만 생성했어요.",
              renderer_inserted_tokens
            ),
          );
          return StrategyRecordReviewResult {
            answer: reviewed.answer,
            metrics: Some(reviewed.metrics),
            renderer_stats: Some(StrategyRecordRendererStats {
              synthetic_cache_requested: review_plan.synthetic_cache_requested,
              synthetic_cache_supported: review_plan.synthetic_cache_supported,
              synthetic_cache_applied: synthetic_applied,
              draft_provider: renderer_draft_provider.clone(),
              proposed_tokens: draft_proposal.token_ids.len(),
              accepted_tokens,
              rejected_tokens,
              verification_batches,
              fallback_decode_tokens: reviewed_llm_generated_tokens.saturating_sub(accepted_tokens),
              renderer_inserted_tokens,
              llm_generated_tokens: reviewed_llm_generated_tokens,
              output_token_reduction_ratio,
            }),
          };
        }
        emit_strategy_progress(
          app,
          "점검",
          "구조화 응답이 충분하지 않아 기존 검토 경로로 한 번 더 다듬을게요.",
        );
      }
      Err(err) => {
        emit_strategy_progress(
          app,
          "점검",
          format!(
            "템플릿 렌더러 단계가 잠시 흔들려 기존 검토 경로로 이어갈게요. {}",
            strategy_trim(&err, 220)
          ),
        );
      }
    }
  }
  emit_strategy_progress(
    app,
    "점검",
    "HyperCLOVA-X가 평가 매트릭 기준으로 기록 초안을 한 번 더 점검하고 있어요.",
  );
  let review_prompt =
    build_strategy_record_review_user_prompt(evidence_packet, case_item, message, strategy_note, draft_answer);
  match execute_strategy_model(
    app,
    backend_runtime,
    STRATEGY_MODEL_DEFAULT_ID,
    HybridStage::RecordReview,
    &build_strategy_record_review_system_prompt(),
    &review_prompt,
    evidence_packet,
    n_ctx.max(3584),
    max_tokens.max(680).clamp(680, 768),
    threads,
    "점검",
    synthetic_cache_enabled,
  ) {
    Ok(reviewed) => {
      emit_strategy_progress(
        app,
        "점검",
        format!(
          "평가 매트릭 점검 후 기록 초안을 {}자로 다시 정리했어요.",
          reviewed.answer.chars().count()
        ),
      );
      StrategyRecordReviewResult {
        answer: reviewed.answer,
        metrics: Some(reviewed.metrics),
        renderer_stats: None,
      }
    }
    Err(err) => {
      emit_strategy_progress(
        app,
        "점검",
        format!(
          "기록 품질 점검 단계가 잠시 흔들려 기존 초안을 유지할게요. {}",
          strategy_trim(&err, 220)
        ),
      );
      StrategyRecordReviewResult {
        answer: draft_answer.to_string(),
        metrics: None,
        renderer_stats: None,
      }
    }
  }
}

fn emit_strategy_progress(app: Option<&AppHandle>, stage: &str, message: impl Into<String>) {
  let safe_stage = stage.trim();
  let safe_message = message.into().trim().to_string();
  if safe_message.is_empty() {
    return;
  }
  eprintln!("[strategy-chat:{}] {}", safe_stage, safe_message);
  if let Some(app) = app {
    let _ = app.emit(
      STRATEGY_PROGRESS_EVENT,
      StrategyProgressPayload {
        stage: safe_stage.to_string(),
        message: safe_message,
      },
    );
  }
}

fn push_unique_path(out: &mut Vec<PathBuf>, path: PathBuf) {
  if !out.iter().any(|p| p == &path) {
    out.push(path);
  }
}

fn strategy_runner_filenames() -> Vec<&'static str> {
  let mut out = vec![STRATEGY_SIDECAR_FILENAME];
  if STRATEGY_SIDECAR_GENERIC_FILENAME != STRATEGY_SIDECAR_FILENAME {
    out.push(STRATEGY_SIDECAR_GENERIC_FILENAME);
  }
  out
}

fn strategy_llama_server_filenames() -> Vec<&'static str> {
  let mut out = vec![STRATEGY_LLAMA_SERVER_FILENAME];
  if STRATEGY_LLAMA_SERVER_GENERIC_FILENAME != STRATEGY_LLAMA_SERVER_FILENAME {
    out.push(STRATEGY_LLAMA_SERVER_GENERIC_FILENAME);
  }
  out
}

fn strategy_runner_hint_text() -> String {
  if STRATEGY_SIDECAR_GENERIC_FILENAME == STRATEGY_SIDECAR_FILENAME {
    return format!("{} 파일", STRATEGY_SIDECAR_FILENAME);
  }
  format!(
    "{} 또는 {} 파일",
    STRATEGY_SIDECAR_FILENAME,
    STRATEGY_SIDECAR_GENERIC_FILENAME
  )
}

fn strategy_llama_server_candidates(app: Option<&AppHandle>) -> Vec<PathBuf> {
  let mut out = Vec::<PathBuf>::new();

  if let Ok(path) = std::env::var("ROOSYCOZY_LLAMA_SERVER_BIN") {
    let trimmed = path.trim();
    if !trimmed.is_empty() {
      push_unique_path(&mut out, PathBuf::from(trimmed));
    }
  }

  if let Some(dir) = strategy_sidecar_storage_dir(app) {
    for file_name in strategy_llama_server_filenames() {
      push_unique_path(&mut out, dir.join(file_name));
    }
  }

  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      for file_name in strategy_llama_server_filenames() {
        push_unique_path(&mut out, dir.join("RoosyCozy").join("sidecar").join(file_name));
      }
      for file_name in strategy_llama_server_filenames() {
        push_unique_path(&mut out, dir.join("sidecar").join(file_name));
      }
    }
  }

  #[cfg(debug_assertions)]
  {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for file_name in strategy_llama_server_filenames() {
      push_unique_path(&mut out, manifest.join("binaries").join(file_name));
      push_unique_path(&mut out, manifest.join("resources").join("sidecar").join(file_name));
    }
  }

  if let Some(paths) = std::env::var_os("PATH") {
    for dir in std::env::split_paths(&paths) {
      for file_name in strategy_llama_server_filenames() {
        push_unique_path(&mut out, dir.join(file_name));
      }
    }
  }

  out
}

fn resolve_llama_server_runner_path(app: Option<&AppHandle>) -> Result<PathBuf, String> {
  for candidate in strategy_llama_server_candidates(app) {
    if candidate.exists() {
      let _ = ensure_executable(&candidate);
      return Ok(candidate);
    }
  }
  Err(format!(
    "llama_server_binary_missing: {} 또는 PATH의 llama-server 실행 파일이 필요해요.",
    strategy_llama_server_filenames().join(", ")
  ))
}

fn normalize_strategy_model_id(raw: Option<&str>) -> &'static str {
  match raw.unwrap_or("").trim().to_ascii_lowercase().as_str() {
    STRATEGY_MODEL_HYBRID_ID => STRATEGY_MODEL_HYBRID_ID,
    STRATEGY_MODEL_ROOSY_ID => STRATEGY_MODEL_ROOSY_ID,
    _ => STRATEGY_MODEL_DEFAULT_ID,
  }
}

fn strategy_model_filename_for_id(model_id: &str) -> &'static str {
  match normalize_strategy_model_id(Some(model_id)) {
    STRATEGY_MODEL_ROOSY_ID => STRATEGY_MODEL_ROOSY_FILENAME,
    _ => STRATEGY_MODEL_FILENAME,
  }
}

fn strategy_model_resource_path_for_id(model_id: &str) -> &'static str {
  match normalize_strategy_model_id(Some(model_id)) {
    STRATEGY_MODEL_ROOSY_ID => STRATEGY_MODEL_ROOSY_RESOURCE_PATH,
    _ => STRATEGY_MODEL_RESOURCE_PATH,
  }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyModelAvailability {
  pub id: String,
  pub label: String,
  pub filename: String,
  pub available: bool,
  pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyModelStatus {
  pub windows_download_mode: bool,
  pub download_supported: bool,
  pub all_ready: bool,
  pub storage_dir: String,
  pub models: Vec<StrategyModelAvailability>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StrategyModelDownloadProgress {
  stage: String,
  model_id: String,
  label: String,
  message: String,
  completed: usize,
  total: usize,
  downloaded_bytes: u64,
  total_bytes: u64,
  percent: u8,
  indeterminate: bool,
}

fn strategy_model_label_for_id(model_id: &str) -> &'static str {
  match normalize_strategy_model_id(Some(model_id)) {
    STRATEGY_MODEL_HYBRID_ID => "ROOSY-Hybrid",
    STRATEGY_MODEL_ROOSY_ID => "Roosy-X",
    _ => "HyperCLOVA-X",
  }
}

#[cfg(target_os = "windows")]
fn configure_strategy_child_process(command: &mut Command) {
  command.creation_flags(STRATEGY_CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn configure_strategy_child_process(_command: &mut Command) {}

fn looks_like_windows_path_line(s: &str) -> bool {
  let bytes = s.as_bytes();
  bytes.len() > 3 && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/')
}

fn is_strategy_runtime_noise(trimmed: &str) -> bool {
  let lower = trimmed.to_ascii_lowercase();
  looks_like_windows_path_line(trimmed)
    || lower.contains("using custom system prompt")
    || lower.contains("llama-sidecar")
    || lower.contains("llama-cli")
    || lower.contains("llama-server")
    || lower.contains(".gguf")
    || lower.starts_with("main: ")
    || lower.starts_with("system info")
    || lower.starts_with("sampler ")
    || lower.starts_with("generate: ")
    || lower.starts_with("n_ctx")
    || lower.starts_with("n_batch")
    || lower.starts_with("build info")
    || lower.starts_with("load_tensors")
    || lower.starts_with("load_backend")
    || lower.starts_with("common params")
    || lower.starts_with("print_info")
    || lower.starts_with("encode ")
    || lower.starts_with("decode ")
    || lower.starts_with("slot ")
    || lower.starts_with("srv ")
}

fn should_emit_strategy_runtime_log(trimmed: &str) -> bool {
  let lower = trimmed.to_ascii_lowercase();
  !is_strategy_runtime_noise(trimmed)
    && (lower.contains("error")
      || lower.contains("failed")
      || lower.contains("cannot")
      || lower.contains("invalid")
      || lower.contains("exception"))
}

#[cfg(unix)]
fn ensure_executable(path: &Path) -> Result<(), String> {
  let meta = fs::metadata(path).map_err(|e| format!("cannot inspect sidecar permissions: {e}"))?;
  let mut perms = meta.permissions();
  let mode = perms.mode();
  if mode & 0o111 == 0 {
    perms.set_mode(mode | 0o755);
    fs::set_permissions(path, perms).map_err(|e| format!("cannot mark sidecar executable: {e}"))?;
  }
  Ok(())
}

#[cfg(not(unix))]
fn ensure_executable(_path: &Path) -> Result<(), String> {
  Ok(())
}

#[cfg(target_os = "windows")]
fn strategy_windows_shared_root() -> PathBuf {
  let public_root = std::env::var_os("PUBLIC")
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
  public_root
    .join("Documents")
    .join("RoosyCozy")
    .join("co.roosycozy.app")
}

#[cfg(target_os = "windows")]
fn strategy_sidecar_storage_dir(app: Option<&AppHandle>) -> Option<PathBuf> {
  let _ = app;
  Some(strategy_windows_shared_root().join("sidecar"))
}

#[cfg(not(target_os = "windows"))]
fn strategy_sidecar_storage_dir(_app: Option<&AppHandle>) -> Option<PathBuf> {
  None
}

#[cfg(target_os = "windows")]
fn copy_strategy_runtime_tree(source: &Path, target: &Path) -> Result<(), String> {
  if !source.exists() {
    return Ok(());
  }
  fs::create_dir_all(target).map_err(|e| format!("AI 런타임 폴더를 만들지 못했어요: {}", e))?;
  for entry in fs::read_dir(source).map_err(|e| format!("AI 런타임 폴더를 읽지 못했어요: {}", e))? {
    let entry = entry.map_err(|e| format!("AI 런타임 폴더 항목을 읽지 못했어요: {}", e))?;
    let source_path = entry.path();
    let target_path = target.join(entry.file_name());
    if source_path.is_dir() {
      copy_strategy_runtime_tree(&source_path, &target_path)?;
    } else {
      if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("AI 런타임 하위 폴더를 만들지 못했어요: {}", e))?;
      }
      fs::copy(&source_path, &target_path).map_err(|e| format!("AI 런타임 파일 복사에 실패했어요: {}", e))?;
    }
  }
  Ok(())
}

#[cfg(target_os = "windows")]
fn hydrate_strategy_runtime_to_appdata(app: Option<&AppHandle>) {
  let Some(target_dir) = strategy_sidecar_storage_dir(app) else {
    return;
  };

  let has_all_required = strategy_runner_filenames()
    .iter()
    .any(|name| target_dir.join(name).exists())
    && target_dir.join("llama.dll").exists()
    && target_dir.join("mtmd.dll").exists();
  if has_all_required {
    return;
  }

  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      let bootstrap_candidates = [
        dir.join("RoosyCozy").join("sidecar"),
        dir.join("sidecar"),
      ];
      for source in bootstrap_candidates {
        if source.exists() {
          let _ = copy_strategy_runtime_tree(&source, &target_dir);
          break;
        }
      }
    }
  }
}

#[cfg(not(target_os = "windows"))]
fn hydrate_strategy_runtime_to_appdata(_app: Option<&AppHandle>) {}

fn strategy_runner_candidates(_app: Option<&AppHandle>) -> Vec<PathBuf> {
  let mut out = Vec::<PathBuf>::new();
  hydrate_strategy_runtime_to_appdata(_app);

  if let Some(dir) = strategy_sidecar_storage_dir(_app) {
    for file_name in strategy_runner_filenames() {
      push_unique_path(&mut out, dir.join(file_name));
    }
  }

  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      for file_name in strategy_runner_filenames() {
        push_unique_path(&mut out, dir.join("RoosyCozy").join("sidecar").join(file_name));
      }
      for file_name in strategy_runner_filenames() {
        push_unique_path(&mut out, dir.join("sidecar").join(file_name));
      }
    }
  }

  #[cfg(debug_assertions)]
  {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for file_name in strategy_runner_filenames() {
      push_unique_path(&mut out, manifest.join("binaries").join(file_name));
      push_unique_path(&mut out, manifest.join("resources").join("sidecar").join(file_name));
    }
  }

  out
}

fn resolve_strategy_runner_path(app: Option<&AppHandle>) -> Result<PathBuf, String> {
  for candidate in strategy_runner_candidates(app) {
    if candidate.exists() {
      let _ = ensure_executable(&candidate);
      return Ok(candidate);
    }
  }
  #[cfg(target_os = "windows")]
  {
    return Err(format!(
      "전략자문 추론기 파일을 찾지 못했어요. 먼저 AI 모델 다운로드를 완료했는지 확인해주세요. 실행 파일은 공용 RoosyCozy sidecar 폴더에서 찾고 있어요. 필요한 파일: {}",
      strategy_runner_hint_text()
    ));
  }

  #[cfg(not(target_os = "windows"))]
  {
    Err(format!(
      "전략자문 추론기 파일을 찾지 못했어요. 앱 번들의 sidecar 안에 {}이(가) 함께 포함되어야 해요.",
      strategy_runner_hint_text()
    ))
  }
}

struct StrategyModelDownloadSpec {
  model_id: &'static str,
  label: &'static str,
  filename: &'static str,
  default_url: &'static str,
}

fn strategy_model_download_specs() -> [StrategyModelDownloadSpec; 2] {
  [
    StrategyModelDownloadSpec {
      model_id: STRATEGY_MODEL_DEFAULT_ID,
      label: strategy_model_label_for_id(STRATEGY_MODEL_DEFAULT_ID),
      filename: STRATEGY_MODEL_FILENAME,
      default_url: STRATEGY_MODEL_DEFAULT_URL,
    },
    StrategyModelDownloadSpec {
      model_id: STRATEGY_MODEL_ROOSY_ID,
      label: strategy_model_label_for_id(STRATEGY_MODEL_ROOSY_ID),
      filename: STRATEGY_MODEL_ROOSY_FILENAME,
      default_url: STRATEGY_MODEL_ROOSY_DEFAULT_URL,
    },
  ]
}

fn emit_strategy_model_download_progress(
  app: &AppHandle,
  stage: &str,
  model_id: &str,
  label: &str,
  message: impl Into<String>,
  completed: usize,
  total: usize,
  downloaded_bytes: u64,
  total_bytes: u64,
  percent: u8,
  indeterminate: bool,
) {
  let event_key = (stage.to_string(), model_id.to_string(), completed, total);
  let last_event = STRATEGY_MODEL_DOWNLOAD_LAST_EVENT.get_or_init(|| Mutex::new(None));
  if let Ok(mut guard) = last_event.lock() {
    if stage == "downloading" {
      if let Some(previous) = guard.as_ref() {
        if previous == &event_key {
          return;
        }
      }
    }
    *guard = Some(event_key);
  }

  let payload = StrategyModelDownloadProgress {
    stage: stage.to_string(),
    model_id: model_id.to_string(),
    label: label.to_string(),
    message: message.into(),
    completed,
    total,
    downloaded_bytes,
    total_bytes,
    percent,
    indeterminate,
  };
  let _ = app.emit("strategy-model-download-progress", payload);
}

fn strategy_model_status_inner(app: Option<&AppHandle>) -> StrategyModelStatus {
  let specs = strategy_model_download_specs();
  let storage_dir = strategy_model_storage_dir(app)
    .unwrap_or_else(|| PathBuf::from("."))
    .display()
    .to_string();
  let models = specs
    .iter()
    .map(|spec| {
      let path = strategy_existing_model_path(app, spec.model_id);
      StrategyModelAvailability {
        id: spec.model_id.to_string(),
        label: spec.label.to_string(),
        filename: spec.filename.to_string(),
        available: path.is_some(),
        path: path.map(|item| item.display().to_string()).unwrap_or_default(),
      }
    })
    .collect::<Vec<_>>();
  let all_ready = models.iter().all(|item| item.available);
  StrategyModelStatus {
    windows_download_mode: cfg!(target_os = "windows"),
    download_supported: cfg!(target_os = "windows"),
    all_ready,
    storage_dir,
    models,
  }
}

pub fn strategy_model_status(app: Option<&AppHandle>) -> Result<StrategyModelStatus, String> {
  Ok(strategy_model_status_inner(app))
}

#[cfg(target_os = "windows")]
fn download_strategy_model_file<F>(url: &str, target: &Path, mut on_progress: F) -> Result<(), String>
where
  F: FnMut(u64, u64, u8),
{
  let client = reqwest::blocking::Client::builder()
    .user_agent("roosycozy/1.0 (windows-model-downloader)")
    .build()
    .map_err(|err| format!("다운로드 클라이언트를 준비하지 못했어요: {err}"))?;
  let mut response = client
    .get(url)
    .send()
    .map_err(|err| format!("모델 다운로드를 시작하지 못했어요: {err}"))?;
  if !response.status().is_success() {
    return Err(format!("모델 다운로드 응답이 올바르지 않아요: HTTP {}", response.status()));
  }
  let total_bytes = response.content_length().unwrap_or(0);
  let tmp_path = target.with_extension("part");
  if tmp_path.exists() {
    let _ = std::fs::remove_file(&tmp_path);
  }
  if let Some(parent) = tmp_path.parent() {
    std::fs::create_dir_all(parent).map_err(|err| format!("모델 저장 폴더를 만들지 못했어요: {err}"))?;
  }
  let mut file = std::fs::File::create(&tmp_path).map_err(|err| format!("임시 모델 파일을 만들지 못했어요: {err}"))?;
  let mut downloaded_bytes = 0u64;
  let mut buffer = vec![0u8; 256 * 1024];
  let mut last_percent = 0u8;
  let mut last_reported_bytes = 0u64;
  on_progress(0, total_bytes, 0);
  loop {
    let read = response
      .read(&mut buffer)
      .map_err(|err| format!("모델 파일을 내려받는 중 읽기 오류가 발생했어요: {err}"))?;
    if read == 0 {
      break;
    }
    file
      .write_all(&buffer[..read])
      .map_err(|err| format!("모델 파일을 저장하지 못했어요: {err}"))?;
    downloaded_bytes += read as u64;
    let percent = if total_bytes > 0 {
      (((downloaded_bytes as f64 / total_bytes as f64) * 100.0).round() as i64).clamp(0, 100) as u8
    } else {
      (((downloaded_bytes / (1024 * 1024)) % 90) as u8).clamp(1, 90)
    };
    let advanced_enough = downloaded_bytes >= last_reported_bytes.saturating_add(512 * 1024);
    if total_bytes == 0 || advanced_enough || percent >= last_percent.saturating_add(1) || downloaded_bytes == total_bytes {
      last_percent = percent;
      last_reported_bytes = downloaded_bytes;
      on_progress(downloaded_bytes, total_bytes, percent);
    }
  }
  on_progress(downloaded_bytes, total_bytes, 100);
  file.flush().map_err(|err| format!("모델 파일 저장을 마무리하지 못했어요: {err}"))?;
  std::fs::rename(&tmp_path, target).map_err(|err| format!("모델 파일 저장을 완료하지 못했어요: {err}"))?;
  Ok(())
}

pub fn start_strategy_model_download(app: &AppHandle) -> Result<StrategyModelStatus, String> {
  let current_status = strategy_model_status_inner(Some(app));
  if current_status.all_ready {
    return Ok(current_status);
  }

  let running = STRATEGY_MODEL_DOWNLOAD_RUNNING.get_or_init(|| Mutex::new(false));
  {
    let mut guard = running
      .lock()
      .map_err(|_| "모델 다운로드 상태를 확인하지 못했어요.".to_string())?;
    if *guard {
      return Ok(current_status);
    }
    *guard = true;
  }

  emit_strategy_model_download_progress(
    app,
    "starting",
    "all",
    "AI 모델",
    "최초 1회 모델 다운로드를 준비하고 있어요.",
    0,
    2,
    0,
    0,
    0,
    true,
  );

  let app_handle = app.clone();
  thread::spawn(move || {
    let result = download_strategy_models(&app_handle);
    match result {
      Ok(status) => {
        emit_strategy_model_download_progress(
          &app_handle,
          "done",
          "all",
          "AI 모델",
          "모델 다운로드가 끝났어요. 이제 바로 채팅할 수 있어요.",
          status.models.iter().filter(|model| model.available).count(),
          status.models.len(),
          0,
          0,
          100,
          false,
        );
      }
      Err(error) => {
        emit_strategy_model_download_progress(
          &app_handle,
          "error",
          "all",
          "AI 모델",
          error,
          0,
          2,
          0,
          0,
          0,
          true,
        );
      }
    }

    if let Ok(mut guard) = STRATEGY_MODEL_DOWNLOAD_RUNNING
      .get_or_init(|| Mutex::new(false))
      .lock()
    {
      *guard = false;
    }
  });

  Ok(current_status)
}

pub fn download_strategy_models(app: &AppHandle) -> Result<StrategyModelStatus, String> {
  #[cfg(not(target_os = "windows"))]
  {
    return Ok(strategy_model_status_inner(Some(app)));
  }

  #[cfg(target_os = "windows")]
  {
    let _guard = STRATEGY_MODEL_DOWNLOAD_LOCK
      .get_or_init(|| Mutex::new(()))
      .lock()
      .map_err(|_| "모델 다운로드 잠금을 잡지 못했어요.".to_string())?;
    let storage_dir = strategy_model_storage_dir(Some(app))
      .ok_or_else(|| "모델 저장 폴더를 찾지 못했어요.".to_string())?;
    std::fs::create_dir_all(&storage_dir).map_err(|err| format!("모델 저장 폴더를 준비하지 못했어요: {err}"))?;

    let specs = strategy_model_download_specs();
    let total = specs.len();
    let mut completed = 0usize;
    let mut handles = Vec::new();

    for spec in specs.iter() {
      let target = storage_dir.join(spec.filename);
      if target.exists() {
        completed += 1;
        emit_strategy_model_download_progress(
          app,
          "skip",
          spec.model_id,
          spec.label,
          format!("{} 모델이 이미 준비되어 있어요.", spec.label),
          completed,
          total,
          0,
          0,
          100,
          false,
        );
        continue;
      }

      let app_handle = app.clone();
      let model_id = spec.model_id.to_string();
      let label = spec.label.to_string();
      let url = spec.default_url.to_string();
      handles.push(thread::spawn(move || -> Result<(String, String), String> {
        emit_strategy_model_download_progress(
          &app_handle,
          "start",
          &model_id,
          &label,
          format!("{} 모델을 내려받는 중이에요.", label),
          0,
          total,
          0,
          0,
          0,
          true,
        );
        download_strategy_model_file(&url, &target, |downloaded_bytes, total_bytes, percent| {
          let detail = if total_bytes > 0 {
            format!(
              "{} 모델 다운로드 중 · {}% · {:.1}MB / {:.1}MB",
              label,
              percent,
              downloaded_bytes as f64 / (1024.0 * 1024.0),
              total_bytes as f64 / (1024.0 * 1024.0)
            )
          } else {
            format!(
              "{} 모델 다운로드 중 · {:.1}MB 수신",
              label,
              downloaded_bytes as f64 / (1024.0 * 1024.0)
            )
          };
          emit_strategy_model_download_progress(
            &app_handle,
            "progress",
            &model_id,
            &label,
            detail,
            0,
            total,
            downloaded_bytes,
            total_bytes,
            percent,
            total_bytes == 0,
          );
        })?;
        Ok((model_id, label))
      }));
    }

    for handle in handles {
      let (model_id, label) = handle
        .join()
        .map_err(|_| "모델 다운로드 스레드가 비정상 종료됐어요.".to_string())??;
      completed += 1;
      emit_strategy_model_download_progress(
        app,
        "done",
        &model_id,
        &label,
        format!("{} 모델 다운로드가 끝났어요.", label),
        completed,
        total,
        0,
        0,
        100,
        false,
      );
    }

    let status = strategy_model_status_inner(Some(app));
    emit_strategy_model_download_progress(
      app,
      "complete",
      STRATEGY_MODEL_HYBRID_ID,
      "ROOSY-Hybrid",
      "두 모델 다운로드가 모두 끝났어요.",
      total,
      total,
      0,
      0,
      100,
      false,
    );
    Ok(status)
  }
}

fn strategy_model_candidates(app: Option<&AppHandle>, model_id: &str) -> Vec<PathBuf> {
  let mut out = Vec::<PathBuf>::new();
  let resource_path = strategy_model_resource_path_for_id(model_id);
  let filename = strategy_model_filename_for_id(model_id);

  if let Some(path) = strategy_downloaded_model_path(app, model_id) {
    push_unique_path(&mut out, path);
  }

  if let Some(app) = app {
    if let Ok(path) = app.path().resolve(resource_path, BaseDirectory::Resource) {
      push_unique_path(&mut out, path);
    }
  }

  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      if let Some(contents) = dir.parent() {
        push_unique_path(&mut out, contents.join("Resources").join("models").join(filename));
      }
      push_unique_path(&mut out, dir.join("RoosyCozy").join("resources").join("models").join(filename));
      push_unique_path(&mut out, dir.join("resources").join("models").join(filename));
    }
  }

  #[cfg(debug_assertions)]
  {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    push_unique_path(&mut out, manifest.join("resources").join("models").join(filename));
    push_unique_path(&mut out, manifest.join("src").join("engine").join(filename));
    push_unique_path(&mut out, manifest.join(filename));
    if let Some(parent) = manifest.parent() {
      push_unique_path(&mut out, parent.join("src-tauri").join("resources").join("models").join(filename));
    }
  }

  out
}

fn strategy_model_storage_dir(app: Option<&AppHandle>) -> Option<PathBuf> {
  #[cfg(target_os = "windows")]
  {
    let _ = app;
    return Some(strategy_windows_shared_root().join("models"));
  }

  #[cfg(not(target_os = "windows"))]
  {
    if let Some(app) = app {
      if let Ok(path) = app.path().resolve("models", BaseDirectory::AppData) {
        return Some(path);
      }
    }
    None
  }
}

fn strategy_downloaded_model_path(app: Option<&AppHandle>, model_id: &str) -> Option<PathBuf> {
  strategy_model_storage_dir(app).map(|dir| dir.join(strategy_model_filename_for_id(model_id)))
}

fn strategy_existing_model_path(app: Option<&AppHandle>, model_id: &str) -> Option<PathBuf> {
  for candidate in strategy_model_candidates(app, model_id) {
    if candidate.exists() {
      return Some(candidate);
    }
  }
  None
}

fn resolve_strategy_model_path(app: Option<&AppHandle>, model_id: &str) -> Result<PathBuf, String> {
  let filename = strategy_model_filename_for_id(model_id);
  if let Some(path) = strategy_existing_model_path(app, model_id) {
    return Ok(path);
  }
  #[cfg(target_os = "windows")]
  let message = format!(
    "{} 모델 파일을 찾지 못했어요. 채팅 화면에서 먼저 AI 모델 다운로드를 실행한 뒤 다시 시도해주세요. 필요한 파일: {}",
    strategy_model_label_for_id(model_id),
    filename
  );
  #[cfg(not(target_os = "windows"))]
  let message = format!(
    "{} 모델 파일을 찾지 못했어요. App 번들의 Resources/models 안에 {} 파일을 포함해주세요.",
    strategy_model_label_for_id(model_id),
    filename
  );
  Err(message)
}

fn format_actor_short(actor: &ActorRef) -> String {
  let kind = actor.r#type.trim();
  let name = actor.name.trim();
  if kind.is_empty() {
    return name.to_string();
  }
  if name.is_empty() {
    return kind.to_string();
  }
  format!("{} {}", kind, name)
}

fn strategy_store_label(record: &RecordItem) -> String {
  let store = record.store_type.trim();
  let other = record.store_other.trim();
  if store.is_empty() || store == "기타" {
    if other.is_empty() {
      "기록유형 미상".to_string()
    } else {
      other.to_string()
    }
  } else {
    store.to_string()
  }
}

fn strategy_place_label(record: &RecordItem) -> String {
  let place = record.place.trim();
  let other = record.place_other.trim();
  if place.is_empty() || place == "기타" {
    if other.is_empty() {
      "장소 미상".to_string()
    } else {
      other.to_string()
    }
  } else {
    place.to_string()
  }
}

fn strategy_main_actor_label(record: &RecordItem) -> String {
  let actor_names = record_main_actor_names(record);
  actor_names
    .get(0)
    .cloned()
    .filter(|x| !x.trim().is_empty())
    .unwrap_or_else(|| {
      let label = format_actor_short(&record.actor);
      if label.trim().is_empty() {
        "당사자 미상".to_string()
      } else {
        label
      }
    })
}

fn summarize_case_context(case_item: Option<&CaseItem>) -> String {
  if let Some(case_item) = case_item {
    let title = case_item.title.trim();
    let actors = case_item
      .actors
      .iter()
      .take(4)
      .map(format_actor_short)
      .collect::<Vec<_>>()
      .join(", ");
    let query = strategy_trim(case_item.query.trim(), 280);
    let mut lines = vec![format!("- 사건 제목: {}", if title.is_empty() { "제목 없음" } else { title })];
    if !actors.is_empty() {
      lines.push(format!("- 핵심 인물: {}", actors));
    }
    if !query.is_empty() {
      lines.push(format!("- 사건 설명: {}", query));
    }
    if !case_item.time_from.trim().is_empty() || !case_item.time_to.trim().is_empty() {
      lines.push(format!("- 기간 필터: {} ~ {}", case_item.time_from.trim(), case_item.time_to.trim()));
    }
    return lines.join("\n");
  }
  "- 사건 연결 없이 증거만으로 분석 중".to_string()
}

fn strategy_legal_dataset() -> &'static StrategyLegalDataset {
  STRATEGY_LEGAL_DATASET.get_or_init(|| {
    serde_json::from_str::<StrategyLegalDataset>(STRATEGY_LEGAL_RAG_JSON)
      .unwrap_or_else(|err| panic!("failed to load legal rag dataset: {err}"))
  })
}

fn strategy_legal_flat_chunks() -> &'static Vec<StrategyLegalFlatChunk> {
  STRATEGY_LEGAL_FLAT_CHUNKS.get_or_init(|| {
    STRATEGY_LEGAL_RAG_JSONL
      .lines()
      .filter_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
          return None;
        }
        serde_json::from_str::<StrategyLegalFlatChunk>(trimmed).ok()
      })
      .collect::<Vec<_>>()
  })
}

fn strategy_record_template() -> &'static StrategyRecordModeTemplate {
  &STRATEGY_RECORD_TEMPLATE_DATASET
    .get_or_init(|| {
      serde_json::from_str::<StrategyRecordTemplateDataset>(STRATEGY_RECORD_TEMPLATE_JSON)
        .unwrap_or_else(|err| panic!("failed to load record mode template dataset: {err}"))
    })
    .record_mode
}

fn strategy_record_generation_profile(
  evidence_packet: &StrategyEvidencePacket,
  message: &str,
) -> StrategyRecordGenerationProfile {
  let compact = strategy_compact_text(message);
  let submission_intent = [
    "제출",
    "보고",
    "교육청",
    "민원",
    "변호사",
    "위원회",
    "법률",
    "공문",
    "상담용",
    "정식",
  ]
  .iter()
  .any(|keyword| compact.contains(keyword));
  let parent_or_external_actor_present = [
    "학부모",
    "보호자",
    "교육청",
    "변호사",
    "경찰",
    "관리자",
    "교감",
    "교장",
    "외부",
  ]
  .iter()
  .any(|keyword| compact.contains(keyword));
  let critical_signal = ["성", "아동학대", "신체", "안전", "폭행", "유출", "온라인", "녹취", "신고"]
    .iter()
    .any(|keyword| compact.contains(keyword));
  let high_signal = ["민원", "외부", "교육청", "학부모", "보호자", "반복", "압박", "비난", "갈등"]
    .iter()
    .any(|keyword| compact.contains(keyword));
  let sensitivity_key = if critical_signal {
    "lv5_critical"
  } else if submission_intent || (high_signal && parent_or_external_actor_present) {
    "lv4_high"
  } else if high_signal || parent_or_external_actor_present || !evidence_packet.legal_references.is_empty() {
    "lv3_warning"
  } else if evidence_packet.evidence_records.len() >= 2 || !evidence_packet.gaps.is_empty() {
    "lv2_attention"
  } else {
    "lv1_general"
  };

  let mut hyper_focus = Vec::new();
  if !evidence_packet.gaps.is_empty() {
    hyper_focus.push("누락 정보와 추가 확인 필요 부분을 엄격히 분리".to_string());
  }
  if evidence_packet.legal_references.len() >= 2 || submission_intent {
    hyper_focus.push("제출용으로 재사용 가능한 중립 구조와 사실 흐름 우선".to_string());
  }
  if evidence_packet.evidence_records.len() >= 3 {
    hyper_focus.push("기존 기록과 현재 입력을 엮어 시간 흐름과 배경을 선명하게 구조화".to_string());
  }
  if hyper_focus.is_empty() {
    hyper_focus.push("사실·시간·조치 순서를 안정적으로 먼저 세우기".to_string());
  }

  let mut roosy_focus = Vec::new();
  if compact.chars().count() <= 60 {
    roosy_focus.push("짧은 입력을 빠른캡쳐형 실무 문장으로 자연스럽게 확장".to_string());
  }
  if evidence_packet.evidence_records.len() >= 1 {
    roosy_focus.push("각 섹션 안의 문장 연결과 현장형 표현 보강".to_string());
  }
  if roosy_focus.is_empty() {
    roosy_focus.push("구조를 건드리지 않는 범위에서 세부 문장을 촘촘하게 채우기".to_string());
  }

  StrategyRecordGenerationProfile {
    sensitivity_key: sensitivity_key.to_string(),
    hyper_focus,
    roosy_focus,
    guard_required: critical_signal
      || submission_intent
      || evidence_packet.gaps.len() >= 2
      || evidence_packet.legal_references.len() >= 2,
    submission_intent,
  }
}

fn build_strategy_record_rubric_summary(template: &StrategyRecordModeTemplate) -> String {
  let mut rubric = template
    .evaluation_rubric
    .iter()
    .map(|(key, value)| (key.as_str(), value.weight, value.description.trim()))
    .collect::<Vec<_>>();
  rubric.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
  if rubric.is_empty() {
    "- 별도 평가 기준 없음".to_string()
  } else {
    rubric
      .iter()
      .map(|(key, weight, description)| format!("- {} (가중치 {:.2}): {}", key, weight, description))
      .collect::<Vec<_>>()
      .join("\n")
  }
}

fn build_strategy_record_template_prompt_block(
  evidence_packet: &StrategyEvidencePacket,
  message: &str,
) -> String {
  let template = strategy_record_template();
  let profile = strategy_record_generation_profile(evidence_packet, message);
  let principles = if template.principles.is_empty() {
    "- 별도 원칙 없음".to_string()
  } else {
    template
      .principles
      .iter()
      .take(5)
      .map(|line| format!("- {}", line.trim()))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let grounding_rules = if template.input_grounding_rules.is_empty() {
    "- 별도 grounding 규칙 없음".to_string()
  } else {
    template
      .input_grounding_rules
      .iter()
      .take(5)
      .map(|line| format!("- {}", line.trim()))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let sections = if template.section_guides.is_empty() {
    "- 별도 섹션 가이드 없음".to_string()
  } else {
    template
      .section_guides
      .iter()
      .map(|guide| {
        let points = if guide.include_points.is_empty() {
          String::new()
        } else {
          format!(
            " | 포함: {}",
            guide
              .include_points
              .iter()
              .take(3)
              .cloned()
              .collect::<Vec<_>>()
              .join(", ")
          )
        };
        let writing = if guide.writing_rules.is_empty() {
          String::new()
        } else {
          format!(
            " | 작성: {}",
            guide
              .writing_rules
              .iter()
              .take(2)
              .cloned()
              .collect::<Vec<_>>()
              .join(", ")
          )
        };
        let missing = if guide.missing_value_policy.is_empty() {
          String::new()
        } else {
          format!(
            " | 누락값 처리: {}",
            guide
              .missing_value_policy
              .iter()
              .take(2)
              .cloned()
              .collect::<Vec<_>>()
              .join(", ")
          )
        };
        let example = if guide.example_phrases.is_empty() {
          String::new()
        } else {
          format!(" | 예시: {}", strategy_trim(guide.example_phrases[0].trim(), 50))
        };
        format!(
          "- {}: {}{}{}{}{}",
          guide.section.trim(),
          guide.objective.trim(),
          points,
          writing,
          missing,
          example
        )
      })
      .collect::<Vec<_>>()
      .join("\n")
  };
  let output_contract = [
    format!("- 형식: {}", template.output_contract.default_format.trim()),
    format!("- 문체: {}", template.output_contract.tone.trim()),
    format!("- 참조 규칙: {}", template.output_contract.citation_policy.trim()),
    format!("- 법령 규칙: {}", template.output_contract.legal_policy.trim()),
    format!(
      "- 교사 확인 원칙: {}",
      template.output_contract.teacher_confirmation_policy.trim()
    ),
  ]
  .into_iter()
  .filter(|line| !line.ends_with(": "))
  .collect::<Vec<_>>()
  .join("\n");
  let checklist = if template.quality_checklist.is_empty() {
    "- 별도 점검 항목 없음".to_string()
  } else {
    template
      .quality_checklist
      .iter()
      .take(6)
      .map(|line| format!("- {}", line.trim()))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let forbidden = if template.forbidden.is_empty() {
    "- 별도 금지 항목 없음".to_string()
  } else {
    template
      .forbidden
      .iter()
      .take(6)
      .map(|line| format!("- {}", line.trim()))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let completion_questions = if template.completion_questions.is_empty() {
    "- 별도 보강 질문 없음".to_string()
  } else {
    template
      .completion_questions
      .iter()
      .take(6)
      .map(|line| format!("- {}", line.trim()))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let risk_rules = template
    .risk_sensitive_rules
    .get(&profile.sensitivity_key)
    .map(|rules| {
      rules
        .iter()
        .take(3)
        .map(|line| format!("- {}", line.trim()))
        .collect::<Vec<_>>()
        .join("\n")
    })
    .unwrap_or_else(|| "- 별도 민감도 규칙 없음".to_string());
  let routing_note = format!(
    "- 현재 민감도 추정: {}\n- 제출/대응 문서 의도 감지: {}\n- HyperCLOVA-X 우선 포인트: {}\n- Roosy-X 보강 포인트: {}\n- 추가 guard 필요 여부: {}",
    profile.sensitivity_key,
    if profile.submission_intent { "예" } else { "아니오" },
    profile.hyper_focus.join(", "),
    profile.roosy_focus.join(", "),
    if profile.guard_required { "예" } else { "아니오" }
  );
  let rubric = build_strategy_record_rubric_summary(template);

  format!(
    "기록 작성 기준\n- 목적\n- {}\n- 핵심 원칙\n{}\n- 입력 grounding 규칙\n{}\n- 출력 계약\n{}\n- 섹션 작성 메모\n{}\n- 민감도/역할 힌트\n{}\n- 현재 민감도 규칙\n{}\n- 품질 점검\n{}\n- 보강 질문\n{}\n- 평가 매트릭\n{}\n- 금지 사항\n{}",
    template.purpose.trim(),
    principles,
    grounding_rules,
    output_contract,
    sections,
    routing_note,
    risk_rules,
    checklist,
    completion_questions,
    rubric,
    forbidden
  )
}

fn build_strategy_record_legal_prompt_block(evidence_packet: &StrategyEvidencePacket) -> String {
  if evidence_packet.legal_references.is_empty() {
    return "- 이번 상황에서 직접 연결된 참고 법령 없음".to_string();
  }
  evidence_packet
    .legal_references
    .iter()
    .take(3)
    .map(|item| {
      let law_label = if item.short_name.trim().is_empty() {
        item.law_name.trim()
      } else {
        item.short_name.trim()
      };
      let article = if item.article_ref.trim().is_empty() {
        item.article_title.trim().to_string()
      } else {
        format!("{} {}", item.article_ref.trim(), item.article_title.trim())
      };
      format!(
        "- {} {}: {}",
        law_label,
        article.trim(),
        strategy_trim(item.teacher_use_case.trim(), 70)
      )
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn strategy_record_answer_has_required_sections(answer: &str) -> bool {
  let template = strategy_record_template();
  let required = if template.output_contract.section_order.is_empty() {
    STRATEGY_RECORD_SECTION_HEADERS
      .iter()
      .map(|header| header.to_string())
      .collect::<Vec<_>>()
  } else {
    template.output_contract.section_order.clone()
  };
  required.iter().all(|header| answer.contains(header))
}

fn strategy_guess_record_people(message: &str) -> Vec<String> {
  let replaced = strategy_sanitize_text(message)
    .replace("이랑", "|")
    .replace("랑", "|")
    .replace("하고", "|")
    .replace(" 및 ", "|")
    .replace("와 ", "|")
    .replace("과 ", "|");
  let mut people = Vec::<String>::new();
  let mut seen = HashSet::<String>::new();
  for chunk in replaced.split('|') {
    let candidate = chunk
      .split_whitespace()
      .next()
      .unwrap_or("")
      .trim_matches(|ch: char| !ch.is_alphanumeric() && !('가'..='힣').contains(&ch))
      .trim();
    if candidate.is_empty() {
      continue;
    }
    let compact = strategy_compact_text(candidate);
    if compact.is_empty()
      || matches!(
        compact.as_str(),
        "오늘" | "점심시간" | "쉬는시간" | "싸웠어" | "싸움" | "다툼" | "문제" | "상황"
      )
    {
      continue;
    }
    if seen.insert(compact) {
      people.push(candidate.to_string());
    }
    if people.len() >= 2 {
      break;
    }
  }
  people
}

fn strategy_guess_record_time_hint(message: &str) -> String {
  let message = strategy_sanitize_text(message);
  if message.contains("오늘") && message.contains("점심시간") {
    "오늘 점심시간".to_string()
  } else if message.contains("오늘") && message.contains("쉬는시간") {
    "오늘 쉬는시간".to_string()
  } else if message.contains("오늘") {
    "오늘".to_string()
  } else if message.contains("점심시간") {
    "점심시간".to_string()
  } else if message.contains("쉬는시간") {
    "쉬는시간".to_string()
  } else {
    "시점 미상".to_string()
  }
}

fn strategy_guess_record_place_hint(message: &str) -> String {
  let message = strategy_sanitize_text(message);
  for place in ["교실", "복도", "운동장", "급식실", "강당", "화장실", "교문", "버스", "메신저", "온라인"] {
    if message.contains(place) {
      return place.to_string();
    }
  }
  if message.contains("점심시간") || message.contains("쉬는시간") {
    "교내 장소 미상".to_string()
  } else {
    "장소 미상".to_string()
  }
}

fn build_strategy_record_fallback_answer(
  evidence_packet: &StrategyEvidencePacket,
  message: &str,
) -> String {
  let people = strategy_guess_record_people(message);
  let actor = people.get(0).cloned().unwrap_or_else(|| "당사자 미상".to_string());
  let counterpart = people.get(1).cloned().unwrap_or_else(|| "상대방 미상".to_string());
  let time_hint = strategy_guess_record_time_hint(message);
  let place_hint = strategy_guess_record_place_hint(message);
  let now = Local::now().format("%Y.%m.%d %H:%M").to_string();
  let summary_input = strategy_trim(&strategy_sanitize_text(message.trim()), 220);
  let evidence_hint = if evidence_packet.evidence_records.is_empty() {
    "현재 참고 기록이 연결되지 않아 직접 입력 내용을 기준으로 1차 기록을 정리함.".to_string()
  } else {
    format!(
      "현재 참고 기록 {}건을 함께 보며 1차 기록을 정리함.",
      evidence_packet.evidence_records.len()
    )
  };
  let legal_hint = if evidence_packet.legal_references.is_empty() {
    "상담 및 사실 확인 흐름을 중심으로 추가 기록을 보완할 필요가 있음.".to_string()
  } else {
    let joined = evidence_packet
      .legal_references
      .iter()
      .take(2)
      .map(|item| {
        let law_label = if item.short_name.trim().is_empty() {
          item.law_name.trim().to_string()
        } else {
          item.short_name.trim().to_string()
        };
        let article = if item.article_ref.trim().is_empty() {
          item.article_title.trim().to_string()
        } else {
          item.article_ref.trim().to_string()
        };
        format!("{} {}", law_label, article)
      })
      .collect::<Vec<_>>()
      .join(", ");
    format!(
      "{} 기준으로 사실 확인, 상담 경위, 훈육 여부를 분리해 남길 필요가 있음.",
      joined
    )
  };

  format!(
    "[기록 기본정보]\n- 기록 시각: {}\n- 주체: {}\n- 상대방: {}\n- 위치/채널: {}\n- 자료 형태: 직접 입력\n- 민감도: 보통\n\n[상황 요약]\n사용자 입력에 따르면 {} {}에 {}와 {} 사이에 다툼이 있었던 것으로 접수되었다. 현재 입력만으로는 다툼의 직접 원인, 언쟁과 신체 접촉 여부, 교사 개입 시점, 직후 분리 조치 여부는 확인되지 않았다. 따라서 우선 두 학생의 진술과 직후 상황을 분리해서 확인하고, 현재 상태까지 이어서 기록할 필요가 있다.\n\n[배경 흐름]\n현재 확보된 정보는 '{}'라는 직접 입력과 기본 상황 설명이 전부이며, 선행 갈등이나 반복 패턴 여부는 추가 확인이 필요하다. {} 점심시간 전후 수업 맥락, 주변 학생 반응, 해당 시점의 교사 관찰 내용이 있으면 사건 흐름을 더 정확하게 재구성할 수 있다.\n\n[핵심 포인트]\n첫째, 실제 다툼의 형태가 말다툼 수준인지 신체 접촉까지 있었는지 즉시 확인할 필요가 있다. 둘째, 직후 두 학생의 감정 상태와 분리 또는 중재 조치가 있었는지 확인해야 한다. 셋째, 이후 관계 회복이 가능한 상황인지, 추가 충돌 가능성이 있는지도 함께 관찰할 필요가 있다.\n\n[관련 자료]\n현재 연결된 참고 기록은 {}건이며, {} 추가로 당사자 진술, 주변 학생 또는 교사의 관찰 메모, 필요 시 당시 상황과 연결되는 자료를 확보하면 기록의 신뢰도를 높일 수 있다.\n\n[내 대응 메모]\n현재는 사용자 입력을 바탕으로 1차 기록 초안을 정리한 상태이며, 저장 전 사실관계와 직후 조치 내용을 더 구체적으로 보완하는 것이 필요하다. 특히 누가 먼저 어떤 표현이나 행동을 했는지, 즉시 어떤 말을 했고 어떤 중재를 했는지를 분리해 남기면 이후 설명과 상담에 도움이 된다.\n\n[추가 메모]\n{} 현재 상태로는 단정적 판단보다 사실 확인 중심 기록이 우선이며, 필요한 경우 상담 또는 생활지도 경위를 별도 메모로 이어서 남기는 것이 적절하다.",
    now,
    actor,
    counterpart,
    place_hint,
    time_hint,
    place_hint,
    actor,
    counterpart,
    summary_input,
    evidence_hint,
    evidence_packet.evidence_records.len(),
    if evidence_packet.evidence_records.is_empty() { "없음" } else { "있음" },
    legal_hint,
  )
}

fn strategy_push_unique_term(out: &mut Vec<String>, seen: &mut HashSet<String>, raw: &str) {
  let trimmed = raw.trim();
  if trimmed.is_empty() {
    return;
  }
  let normalized = norm(trimmed);
  if normalized.chars().count() < 2 {
    return;
  }
  if seen.insert(normalized) {
    out.push(trimmed.to_string());
  }
}

fn strategy_push_unique_reason(out: &mut Vec<String>, reason: String) {
  let trimmed = reason.trim();
  if trimmed.is_empty() {
    return;
  }
  if !out.iter().any(|item| item == trimmed) {
    out.push(trimmed.to_string());
  }
}

fn strategy_collect_legal_source_text(
  case_item: Option<&CaseItem>,
  selected_records: &[&RecordItem],
  retrieval_query: &str,
  message: &str,
  strategy_note: Option<&str>,
) -> String {
  let mut parts = Vec::<String>::new();
  if let Some(case_item) = case_item {
    if !case_item.title.trim().is_empty() {
      parts.push(case_item.title.trim().to_string());
    }
    if !case_item.query.trim().is_empty() {
      parts.push(case_item.query.trim().to_string());
    }
    for actor in case_item.actors.iter().take(6) {
      let label = format_actor_short(actor);
      if !label.trim().is_empty() {
        parts.push(label);
      }
    }
  }
  if !retrieval_query.trim().is_empty() {
    parts.push(retrieval_query.trim().to_string());
  }
  if !message.trim().is_empty() {
    parts.push(message.trim().to_string());
  }
  if let Some(note) = strategy_note.map(str::trim).filter(|note| !note.is_empty()) {
    parts.push(note.to_string());
  }
  for record in selected_records {
    if !record.summary.trim().is_empty() {
      parts.push(record.summary.trim().to_string());
    }
    let actor = strategy_main_actor_label(record);
    if !actor.trim().is_empty() {
      parts.push(actor);
    }
    let place = strategy_place_label(record);
    if !place.trim().is_empty() && place != "장소 미상" {
      parts.push(place);
    }
    if let Some(parts_summary) = record.summary_parts.as_ref() {
      for field in [
        parts_summary.background.trim(),
        parts_summary.teacher_actions.trim(),
        parts_summary.issues.trim(),
        parts_summary.evidence_list.trim(),
        parts_summary.other.trim(),
      ] {
        if !field.is_empty() {
          parts.push(field.to_string());
        }
      }
    }
    for related in &record.related {
      let label = format_actor_short(related);
      if !label.trim().is_empty() {
        parts.push(label);
      }
    }
  }
  strategy_trim(&parts.join(" "), 2400)
}

fn strategy_build_legal_query_terms(source_text: &str, concept_map: &HashMap<String, Vec<String>>) -> Vec<String> {
  let source_norm = norm(source_text);
  let mut out = Vec::<String>::new();
  let mut seen = HashSet::<String>::new();

  for token in tokenize(&source_norm) {
    strategy_push_unique_term(&mut out, &mut seen, &token);
  }

  for (concept, expansions) in concept_map {
    let concept_norm = norm(concept);
    if concept_norm.is_empty() || !source_norm.contains(&concept_norm) {
      continue;
    }
    strategy_push_unique_term(&mut out, &mut seen, concept);
    for expansion in expansions {
      strategy_push_unique_term(&mut out, &mut seen, expansion);
    }
  }

  out.truncate(80);
  out
}

fn strategy_phrase_score(
  source_norm: &str,
  phrases: &[String],
  weight: f32,
  reason_prefix: &str,
  reasons: &mut Vec<String>,
) -> f32 {
  let mut score = 0.0;
  for phrase in phrases {
    let trimmed = phrase.trim();
    let normalized = norm(trimmed);
    if normalized.chars().count() < 2 || !source_norm.contains(&normalized) {
      continue;
    }
    score += weight;
    strategy_push_unique_reason(reasons, format!("{} {}", reason_prefix, trimmed));
  }
  score
}

fn strategy_overlap_score(source_norm: &str, query_terms: &[String], reasons: &mut Vec<String>) -> f32 {
  let mut matched = Vec::<String>::new();
  for term in query_terms {
    let normalized = norm(term);
    if normalized.chars().count() < 2 || !source_norm.contains(&normalized) {
      continue;
    }
    if !matched.iter().any(|item| item == term) {
      matched.push(term.clone());
    }
  }
  for term in matched.iter().take(4) {
    strategy_push_unique_reason(reasons, format!("질의 일치 {}", term));
  }
  (matched.len().min(6) as f32) * 0.22
}

fn build_strategy_legal_line_for_prompt(reference: &StrategyLegalReference) -> String {
  let law_name = if reference.short_name.trim().is_empty() {
    reference.law_name.trim().to_string()
  } else {
    format!("{} ({})", reference.short_name.trim(), reference.law_name.trim())
  };
  let mut line = format!(
    "[{}] {} {} {}",
    reference.ref_id,
    strategy_trim(&law_name, 52),
    strategy_trim(reference.article_ref.trim(), 18),
    strategy_trim(reference.article_title.trim(), 44)
  );
  if !reference.legal_point.trim().is_empty() {
    line.push_str(&format!(" | 취지: {}", strategy_trim(reference.legal_point.trim(), 120)));
  }
  if !reference.teacher_use_case.trim().is_empty() {
    line.push_str(&format!(" | 현장 적용: {}", strategy_trim(reference.teacher_use_case.trim(), 110)));
  }
  if !reference.relevance_reasons.is_empty() {
    line.push_str(&format!(" | 연결 이유: {}", strategy_trim(&reference.relevance_reasons.join(", "), 96)));
  }
  line
}

fn build_strategy_legal_references(
  case_item: Option<&CaseItem>,
  selected_records: &[&RecordItem],
  retrieval_query: &str,
  message: &str,
  strategy_note: Option<&str>,
) -> Vec<StrategyLegalReference> {
  #[derive(Clone)]
  struct Candidate<'a> {
    score: f32,
    reasons: Vec<String>,
    chunk: &'a StrategyLegalFlatChunk,
    law: Option<&'a StrategyLegalLawRecord>,
  }

  let dataset = strategy_legal_dataset();
  let flat_chunks = strategy_legal_flat_chunks();
  if dataset.records.is_empty() && flat_chunks.is_empty() {
    return Vec::new();
  }

  let source_text = strategy_collect_legal_source_text(case_item, selected_records, retrieval_query, message, strategy_note);
  let source_norm = norm(&source_text);
  let query_terms = strategy_build_legal_query_terms(&source_text, &dataset.retrieval_boosters.concept_map);
  let law_by_id = dataset
    .records
    .iter()
    .filter_map(|law| {
      let id = law.record_id.trim().to_string();
      if id.is_empty() { None } else { Some((id, law)) }
    })
    .collect::<HashMap<_, _>>();
  let mut candidates = Vec::<Candidate>::new();

  for chunk in flat_chunks.iter() {
    if !chunk.chunk_type.trim().is_empty() && !chunk.chunk_type.trim().eq_ignore_ascii_case("article") {
      continue;
    }
    let law = law_by_id.get(chunk.record_id.trim()).copied();

    let mut law_names = Vec::<String>::new();
    let mut law_names_seen = HashSet::<String>::new();
    for raw in [
      chunk.official_name.as_str(),
      chunk.short_name.as_str(),
      law.map(|item| item.official_name.as_str()).unwrap_or(""),
      law.map(|item| item.short_name.as_str()).unwrap_or(""),
    ] {
      strategy_push_unique_term(&mut law_names, &mut law_names_seen, raw);
    }

    let mut aliases = Vec::<String>::new();
    let mut aliases_seen = HashSet::<String>::new();
    for raw in &chunk.aliases {
      strategy_push_unique_term(&mut aliases, &mut aliases_seen, raw);
    }
    if let Some(law_item) = law {
      for raw in &law_item.rag.aliases {
        strategy_push_unique_term(&mut aliases, &mut aliases_seen, raw);
      }
    }

    let mut topical_tags = Vec::<String>::new();
    let mut topical_seen = HashSet::<String>::new();
    for raw in &chunk.topical_tags {
      strategy_push_unique_term(&mut topical_tags, &mut topical_seen, raw);
    }
    if let Some(law_item) = law {
      for raw in &law_item.rag.topical_tags {
        strategy_push_unique_term(&mut topical_tags, &mut topical_seen, raw);
      }
    }

    let article_titles = vec![
      chunk.article_title.clone(),
      format!("{} {}", chunk.article_no.trim(), chunk.article_title.trim()).trim().to_string(),
    ];
    let article_keywords = chunk.keywords.clone();
    let article_text = norm(&format!(
      "{} {} {} {} {} {} {} {} {}",
      chunk.official_name,
      chunk.short_name,
      chunk.school_relevance,
      topical_tags.join(" "),
      chunk.article_no,
      chunk.article_title,
      chunk.legal_point,
      chunk.teacher_use_case,
      chunk.retrieval_text
    ));

    let mut reasons = Vec::<String>::new();
    let mut score = 0.0;
    score += strategy_phrase_score(&source_norm, &law_names, 3.1, "법령명 일치", &mut reasons);
    score += strategy_phrase_score(&source_norm, &aliases, 2.7, "법령 별칭 일치", &mut reasons);
    score += strategy_phrase_score(&source_norm, &topical_tags, 2.3, "주제 일치", &mut reasons);
    score += strategy_phrase_score(&source_norm, &article_titles, 1.8, "조문 주제 일치", &mut reasons);
    score += strategy_phrase_score(&source_norm, &article_keywords, 1.6, "키워드 일치", &mut reasons);
    score += strategy_overlap_score(&article_text, &query_terms, &mut reasons);

    if !chunk.school_relevance.trim().is_empty() {
      score += strategy_overlap_score(&norm(chunk.school_relevance.trim()), &query_terms, &mut reasons) * 0.7;
    }
    if source_norm.contains(&norm(chunk.article_no.trim())) && !chunk.article_no.trim().is_empty() {
      score += 1.0;
      strategy_push_unique_reason(&mut reasons, format!("조문 번호 일치 {}", chunk.article_no.trim()));
    }
    if !chunk.retrieval_text.trim().is_empty() {
      score += strategy_overlap_score(&norm(chunk.retrieval_text.trim()), &query_terms, &mut reasons) * 1.15;
    }

    if score >= 2.2 || reasons.len() >= 2 {
      candidates.push(Candidate { score, reasons, chunk, law });
    }
  }

  candidates.sort_by(|a, b| {
    b.score
      .partial_cmp(&a.score)
      .unwrap_or(Ordering::Equal)
      .then_with(|| a.chunk.official_name.cmp(&b.chunk.official_name))
      .then_with(|| a.chunk.article_no.cmp(&b.chunk.article_no))
  });

  let mut per_law = HashMap::<String, usize>::new();
  let mut out = Vec::<StrategyLegalReference>::new();
  for candidate in candidates {
    let law_id = candidate.chunk.record_id.trim().to_string();
    if law_id.is_empty() {
      continue;
    }
    let used = per_law.entry(law_id.clone()).or_insert(0);
    if *used >= 2 {
      continue;
    }
    *used += 1;
    let index = out.len() + 1;
    out.push(StrategyLegalReference {
      ref_id: format!("L{}", index),
      law_id,
      law_name: candidate
        .law
        .map(|item| item.official_name.trim().to_string())
        .unwrap_or_else(|| candidate.chunk.official_name.trim().to_string()),
      short_name: candidate
        .law
        .map(|item| item.short_name.trim().to_string())
        .unwrap_or_else(|| candidate.chunk.short_name.trim().to_string()),
      article_ref: candidate.chunk.article_no.trim().to_string(),
      article_title: candidate.chunk.article_title.trim().to_string(),
      legal_point: candidate.chunk.legal_point.trim().to_string(),
      teacher_use_case: candidate.chunk.teacher_use_case.trim().to_string(),
      source_url: candidate
        .law
        .map(|item| item.source_url.trim().to_string())
        .unwrap_or_else(|| candidate.chunk.source_url.trim().to_string()),
      status_label: candidate
        .law
        .map(|item| item.current_status_label.trim().to_string())
        .unwrap_or_else(|| candidate.chunk.current_status_label.trim().to_string()),
      relevance_reasons: candidate.reasons.into_iter().take(4).collect(),
    });
    if out.len() >= 4 {
      break;
    }
  }

  out
}

fn build_strategy_retrieval_query(
  case_item: Option<&CaseItem>,
  message: &str,
  strategy_note: Option<&str>,
) -> String {
  let mut parts = Vec::<String>::new();
  let question = message.trim();
  if !question.is_empty() {
    parts.push(question.to_string());
  }
  if let Some(case_item) = case_item {
    let title = case_item.title.trim();
    if !title.is_empty() {
      parts.push(title.to_string());
    }
    let query = case_item.query.trim();
    if !query.is_empty() {
      parts.push(query.to_string());
    }
    let actor_block = case_item
      .actors
      .iter()
      .take(4)
      .map(format_actor_short)
      .filter(|x| !x.trim().is_empty())
      .collect::<Vec<_>>()
      .join(" ");
    if !actor_block.is_empty() {
      parts.push(actor_block);
    }
  }
  let note = strategy_trim(strategy_note.unwrap_or("").trim(), 180);
  if !note.is_empty() && note != "없음" {
    parts.push(note);
  }
  strategy_trim(&parts.join(" "), 340)
}

fn build_strategy_retrieval_case(
  case_item: Option<&CaseItem>,
  retrieval_query: &str,
  max_results: usize,
) -> CaseItem {
  let mut built = case_item.cloned().unwrap_or(CaseItem {
    id: "strategy".to_string(),
    title: "직접 분석".to_string(),
    query: String::new(),
    time_from: String::new(),
    time_to: String::new(),
    max_results: Some(max_results as u32),
    actors: Vec::new(),
  });
  built.query = retrieval_query.to_string();
  built.max_results = Some(max_results as u32);
  built
}

fn summarize_strategy_record_parts(record: &RecordItem) -> String {
  let mut extras = Vec::<String>::new();
  if let Some(parts) = record.summary_parts.as_ref() {
    let issues = parts.issues.trim();
    let evidence = parts.evidence_list.trim();
    let actions = parts.teacher_actions.trim();
    if !issues.is_empty() {
      extras.push(format!("핵심포인트: {}", strategy_trim(issues, 90)));
    }
    if !evidence.is_empty() {
      extras.push(format!("자료: {}", strategy_trim(evidence, 72)));
    }
    if !actions.is_empty() {
      extras.push(format!("내대응: {}", strategy_trim(actions, 72)));
    }
  }
  extras.join(" | ")
}

fn build_strategy_record_line_for_prompt(evidence: &StrategyEvidenceRecord) -> String {
  let mut line = format!(
    "[{}] {} | {} | {} | {} | {}",
    evidence.ref_id,
    strategy_trim(evidence.ts.trim(), 32),
    strategy_trim(evidence.actor.trim(), 28),
    strategy_trim(evidence.store.trim(), 18),
    strategy_trim(evidence.place.trim(), 18),
    strategy_trim(evidence.summary.trim(), 180)
  );
  if !evidence.risk_label.trim().is_empty() {
    line.push_str(&format!(" | 위험: {}", strategy_trim(evidence.risk_label.trim(), 18)));
  }
  if !evidence.reasons.is_empty() {
    line.push_str(&format!(" | 선택 이유: {}", strategy_trim(&evidence.reasons.join(", "), 96)));
  }
  line
}

const STRATEGY_CHAT_MODE_ANALYSIS: &str = "analysis";
const STRATEGY_CHAT_MODE_RECORD: &str = "record";

fn normalize_strategy_chat_mode(mode: Option<&str>) -> &'static str {
  match mode.unwrap_or("").trim().to_ascii_lowercase().as_str() {
    "record" | "write" | "capture" => STRATEGY_CHAT_MODE_RECORD,
    _ => STRATEGY_CHAT_MODE_ANALYSIS,
  }
}

fn strategy_parse_prompt_blocks(raw: &str) -> Vec<(String, String)> {
  let mut blocks = Vec::<(String, String)>::new();
  let mut current_title = String::new();
  let mut current_lines = Vec::<String>::new();
  for line in raw.lines() {
    let trimmed = line.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() > 2 {
      if !current_title.is_empty() {
        blocks.push((current_title.clone(), current_lines.join("\n").trim().to_string()));
      }
      current_title = trimmed.trim_matches(&['[', ']'][..]).to_string();
      current_lines.clear();
      continue;
    }
    current_lines.push(line.to_string());
  }
  if !current_title.is_empty() {
    blocks.push((current_title, current_lines.join("\n").trim().to_string()));
  }
  blocks.retain(|(_, body)| !body.trim().is_empty());
  blocks
}

fn strategy_system_line_is_dynamic(trimmed: &str) -> bool {
  if trimmed.is_empty() {
    return false;
  }
  let normalized = trimmed.trim();
  let dynamic_markers = [
    "[사용자 입력 상황]",
    "[사건 맥락]",
    "[참고 기록]",
    "[증거]",
    "[직전 대화]",
    "[이번 요청]",
    "[질문]",
    "[대화]",
    "[메시지]",
  ];
  dynamic_markers.iter().any(|marker| normalized.starts_with(marker))
    || normalized.contains("현재 시각")
    || normalized.contains("오늘 날짜")
}

fn strategy_extract_dynamic_system_leakage(system_prompt_raw: &str) -> Vec<PromptSegment> {
  let leaked_lines = system_prompt_raw
    .lines()
    .map(str::trim)
    .filter(|line| strategy_system_line_is_dynamic(line))
    .map(|line| line.to_string())
    .collect::<Vec<_>>();
  if leaked_lines.is_empty() {
    Vec::new()
  } else {
    vec![PromptSegment::dynamic_segment(
      PromptSegmentKind::DynamicCaseContext,
      leaked_lines.join("\n"),
    )]
  }
}

fn strategy_system_prompt_segments(system_prompt_raw: &str, stage: HybridStage) -> Vec<PromptSegment> {
  let mut system_prefix_lines = Vec::<String>::new();
  let mut mode_lines = Vec::<String>::new();
  let mut stage_lines = Vec::<String>::new();
  let mut output_lines = Vec::<String>::new();

  for line in system_prompt_raw.lines() {
    let trimmed = line.trim();
    if trimmed.is_empty() {
      continue;
    }
    if strategy_system_line_is_dynamic(trimmed) {
      continue;
    }
    if trimmed.starts_with("- AI 언어모델") || trimmed.starts_with("- 반드시 한국어") {
      system_prefix_lines.push(trimmed.to_string());
    } else if trimmed.starts_with("- 역할:") {
      mode_lines.push(trimmed.to_string());
    } else if trimmed.contains("출력")
      || trimmed.contains("섹션")
      || trimmed.starts_with('[')
      || trimmed.contains("지정된")
      || trimmed.contains("제목")
    {
      output_lines.push(trimmed.to_string());
    } else {
      stage_lines.push(trimmed.to_string());
    }
  }

  let mut segments = Vec::<PromptSegment>::new();
  if !system_prefix_lines.is_empty() {
    segments.push(PromptSegment::static_segment(
      PromptSegmentKind::StaticSystemPrefix,
      format!("{}.system", stage.as_str()),
      system_prefix_lines.join("\n"),
    ));
  }
  if !mode_lines.is_empty() {
    segments.push(PromptSegment::static_segment(
      PromptSegmentKind::StaticModeTemplate,
      format!("{}.mode", stage.as_str()),
      mode_lines.join("\n"),
    ));
  }
  if !stage_lines.is_empty() {
    segments.push(PromptSegment::static_segment(
      PromptSegmentKind::StaticStageInstruction,
      format!("{}.instruction", stage.as_str()),
      stage_lines.join("\n"),
    ));
  }
  if !output_lines.is_empty() {
    segments.push(PromptSegment::static_segment(
      PromptSegmentKind::StaticOutputFormat,
      format!("{}.format", stage.as_str()),
      output_lines.join("\n"),
    ));
  }
  if segments.is_empty() {
    segments.push(PromptSegment::static_segment(
      PromptSegmentKind::StaticSystemPrefix,
      format!("{}.fallback", stage.as_str()),
      strategy_sanitize_text(system_prompt_raw),
    ));
  }
  segments
}

fn strategy_user_prompt_segments(user_prompt_raw: &str) -> Vec<PromptSegment> {
  let blocks = strategy_parse_prompt_blocks(user_prompt_raw);
  if blocks.is_empty() {
    return vec![PromptSegment::dynamic_segment(
      PromptSegmentKind::DynamicUserMessage,
      strategy_sanitize_text(user_prompt_raw),
    )];
  }
  let mut segments = Vec::<PromptSegment>::new();
  for (title, body) in blocks {
    let text = format!("[{}]\n{}", title.trim(), body.trim());
    let kind = if title.contains("법령") {
      PromptSegmentKind::DynamicLegalRefs
    } else if title.contains("증거") || title.contains("참고 기록") || title.contains("핵심 근거") {
      PromptSegmentKind::DynamicEvidencePacket
    } else if title.contains("직전 대화") {
      PromptSegmentKind::DynamicConversation
    } else if title.contains("초안") || title.contains("골격") {
      PromptSegmentKind::DynamicDraftArtifacts
    } else if title.contains("질문") || title.contains("사용자 입력 상황") || title.contains("이번 요청") {
      PromptSegmentKind::DynamicUserMessage
    } else {
      PromptSegmentKind::DynamicCaseContext
    };
    segments.push(PromptSegment::dynamic_segment(kind, text));
  }
  segments
}

fn strategy_prepare_stage_prompt(
  model_id: &str,
  stage: HybridStage,
  cache_requested: bool,
  system_prompt_raw: &str,
  user_prompt_raw: &str,
  expected_output_tokens: usize,
) -> crate::drace::PreparedStagePrompt {
  let manager = DraceCacheManager::global();
  let mut user_segments = strategy_user_prompt_segments(user_prompt_raw);
  let leaked_dynamic_segments = strategy_extract_dynamic_system_leakage(system_prompt_raw);
  if !leaked_dynamic_segments.is_empty() {
    user_segments.extend(leaked_dynamic_segments);
  }
  manager.prepare_prompt(
    model_id,
    stage,
    cache_requested,
    strategy_system_prompt_segments(system_prompt_raw, stage),
    user_segments,
    expected_output_tokens,
  )
}

fn strategy_short_hash_u64(value: u64) -> String {
  format!("{:016x}", value)
}

fn strategy_hybrid_stage_from_name(value: &str) -> Option<HybridStage> {
  match value.trim().to_ascii_lowercase().as_str() {
    "general_main" => Some(HybridStage::GeneralMain),
    "general_draft" => Some(HybridStage::GeneralDraft),
    "general_synthesis" => Some(HybridStage::GeneralSynthesis),
    "record_main" => Some(HybridStage::RecordMain),
    "record_fill" => Some(HybridStage::RecordFill),
    "record_synthesis" => Some(HybridStage::RecordSynthesis),
    "record_review" => Some(HybridStage::RecordReview),
    "fast_roosy" => Some(HybridStage::FastRoosy),
    "record_recovery" => Some(HybridStage::RecordRecovery),
    _ => None,
  }
}

fn strategy_load_drace_persistent_state(app: Option<&AppHandle>) {
  let Some(app_handle) = app else {
    return;
  };
  if STRATEGY_DRACE_PERSISTENT_STATE_LOADED.load(AtomicOrdering::SeqCst) {
    return;
  }
  let _ = DraceCacheManager::global().load_persistent_state(app_handle);
  STRATEGY_DRACE_PERSISTENT_STATE_LOADED.store(true, AtomicOrdering::SeqCst);
}

fn strategy_persist_drace_persistent_state(app: Option<&AppHandle>) {
  let Some(app_handle) = app else {
    return;
  };
  let _ = DraceCacheManager::global().persist_persistent_state(app_handle);
}

fn strategy_prewarm_known_static_prefixes_for_model(
  model_id: &str,
  app: Option<&AppHandle>,
  config: &LlamaServerConfig,
) {
  let Some(_) = app else {
    return;
  };
  let manager = DraceCacheManager::global();
  let prefixes = manager.static_prefixes_for_model(model_id);
  if prefixes.is_empty() {
    return;
  }
  let (endpoint, base_slot) = effective_llama_server_endpoint_for_model(config, model_id);
  let backend = LlamaServerSessionBackend;
  for prefix in prefixes {
    let Some(stage) = strategy_hybrid_stage_from_name(&prefix.stage_name) else {
      continue;
    };
    let slot = llama_server_slot_for_stage(base_slot, stage);
    if manager.is_prefix_warm(model_id, &prefix.id, slot, Some(prefix.content_hash)) {
      continue;
    }
    let options = GenerationSessionOptions {
      model_id: model_id.to_string(),
      endpoint: endpoint.clone(),
      model_path: None,
      slot,
      cache_prompt: config.cache_prompt,
      assistant_prefix: None,
      n_ctx: Some(4096),
      threads: Some(strategy_default_threads()),
      max_tokens: 0,
      temperature: 0.0,
      top_p: 1.0,
      repeat_penalty: 1.0,
      request_timeout_ms: config.request_timeout_ms.max(10_000),
    };
    if let Ok(session) = backend.open_session(&prefix.text, options) {
      if backend.generate_next(&session).is_ok() {
        manager.mark_prefix_warm(model_id, &prefix.id, slot, Some(prefix.content_hash), prefix.estimated_tokens);
      }
      let _ = backend.close_session(&session);
    }
  }
  strategy_persist_drace_persistent_state(app);
}

fn build_strategy_actor_summary(records: &[&RecordItem]) -> Vec<String> {
  let mut counts = HashMap::<String, usize>::new();
  for record in records {
    let main = strategy_main_actor_label(record);
    if !main.trim().is_empty() {
      *counts.entry(main).or_insert(0) += 1;
    }
    for related in &record.related {
      let label = format!("관련 {}", format_actor_short(related));
      if !label.trim().is_empty() {
        *counts.entry(label).or_insert(0) += 1;
      }
    }
  }
  let mut ranked = counts.into_iter().collect::<Vec<_>>();
  ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
  ranked
    .into_iter()
    .take(6)
    .map(|(label, count)| format!("{} · {}건", label, count))
    .collect()
}

fn build_strategy_gaps(case_item: Option<&CaseItem>, records: &[&RecordItem], total_records: usize) -> Vec<String> {
  let mut gaps = Vec::<String>::new();
  if case_item.is_none() {
    gaps.push("기준 컬렉션 없이 직접 분석 중이라 사건 범위가 넓을 수 있어요.".to_string());
  }
  if records.len() <= 2 {
    gaps.push("현재 연결된 핵심 기록 수가 적어서 흐름 판단이 제한적일 수 있어요.".to_string());
  }
  if total_records > records.len() {
    gaps.push(format!("전체 기록 {}건 중 핵심 근거 {}건만 추려 분석했어요.", total_records, records.len()));
  }
  let missing_actions = records
    .iter()
    .filter(|record| {
      record
        .summary_parts
        .as_ref()
        .map(|parts| parts.teacher_actions.trim().is_empty())
        .unwrap_or(true)
    })
    .count();
  if missing_actions >= records.len().max(1) / 2 && !records.is_empty() {
    gaps.push("내가 실제로 취한 대응 메모가 비어 있는 기록이 많아요.".to_string());
  }
  let missing_evidence = records
    .iter()
    .filter(|record| {
      record
        .summary_parts
        .as_ref()
        .map(|parts| parts.evidence_list.trim().is_empty())
        .unwrap_or(true)
    })
    .count();
  if missing_evidence >= records.len().max(1) / 2 && !records.is_empty() {
    gaps.push("관련 자료·증빙 정리 칸이 비어 있는 기록이 많아요.".to_string());
  }
  gaps.truncate(4);
  gaps
}

fn push_strategy_evidence_candidate(
  out: &mut Vec<(String, Vec<String>, f32)>,
  seen: &mut HashSet<String>,
  id: String,
  reasons: Vec<String>,
  score: f32,
) {
  if !id.trim().is_empty() && seen.insert(id.clone()) {
    out.push((id, reasons, score));
  }
}

fn select_strategy_evidence_records(
  records: &[RecordItem],
  ranked_hits: &[RankedHit],
) -> Vec<(String, Vec<String>, f32)> {
  let mut out = Vec::<(String, Vec<String>, f32)>::new();
  let mut seen = HashSet::<String>::new();

  for hit in ranked_hits.iter().take(5) {
    push_strategy_evidence_candidate(&mut out, &mut seen, hit.id.clone(), hit.reasons.iter().take(3).cloned().collect(), hit.score);
  }

  let mut recent = records.iter().collect::<Vec<_>>();
  recent.sort_by(|a, b| b.ts.cmp(&a.ts));
  for record in recent.into_iter().take(3) {
    push_strategy_evidence_candidate(&mut out, &mut seen, record.id.clone(), vec!["최근 흐름".to_string()], 0.0);
  }

  let should_fill_default = out.is_empty();
  if should_fill_default {
    for record in records.iter().rev().take(4) {
      push_strategy_evidence_candidate(&mut out, &mut seen, record.id.clone(), vec!["기본 선택".to_string()], 0.0);
    }
  }

  out
}

fn build_strategy_evidence_packet(
  case_item: Option<&CaseItem>,
  records: &[RecordItem],
  message: &str,
  strategy_note: Option<&str>,
) -> (StrategyEvidencePacket, String) {
  let retrieval_query = build_strategy_retrieval_query(case_item, message, strategy_note);
  let retrieval_case = build_strategy_retrieval_case(case_item, &retrieval_query, records.len().clamp(4, 8));
  let ranked_hits = rank_records_for_case(
    records,
    &retrieval_case,
    Some(RankOpts {
      max_results: Some(records.len().clamp(4, 8) as u32),
      weights: Some(RankWeights {
        actor: Some(2.8),
        related: Some(1.2),
        text: Some(2.4),
      }),
      min_score: Some(0.0),
      min_text_sim: Some(0.15),
    }),
  );
  let selected = select_strategy_evidence_records(records, &ranked_hits);
  let by_id = records
    .iter()
    .map(|record| (record.id.clone(), record))
    .collect::<HashMap<_, _>>();

  let mut selected_records = selected
    .iter()
    .filter_map(|(id, _, _)| by_id.get(id).copied())
    .collect::<Vec<_>>();
  selected_records.sort_by(|a, b| a.ts.cmp(&b.ts));

  let earliest = selected_records.first().map(|record| record.ts.trim()).unwrap_or("");
  let latest = selected_records.last().map(|record| record.ts.trim()).unwrap_or("");
  let actor_summary = build_strategy_actor_summary(&selected_records);
  let gaps = build_strategy_gaps(case_item, &selected_records, records.len());
  let legal_references = build_strategy_legal_references(case_item, &selected_records, &retrieval_query, message, strategy_note);

  let case_title = case_item
    .map(|item| item.title.trim().to_string())
    .filter(|title| !title.is_empty())
    .unwrap_or_else(|| "직접 분석".to_string());
  let actor_line = actor_summary
    .iter()
    .take(3)
    .cloned()
    .collect::<Vec<_>>()
    .join(", ");
  let focus_summary = if earliest.is_empty() && latest.is_empty() {
    format!("{} 기준으로 핵심 근거 {}건을 묶었어요.", case_title, selected_records.len())
  } else {
    format!(
      "{} 기준. {} ~ {} 흐름에서 핵심 근거 {}건을 골랐고, 주요 인물은 {}예요.",
      case_title,
      if earliest.is_empty() { "시점 미상" } else { earliest },
      if latest.is_empty() { "시점 미상" } else { latest },
      selected_records.len(),
      if actor_line.is_empty() { "정리 중" } else { actor_line.as_str() }
    )
  };

  let overview = if selected_records.is_empty() {
    "핵심 근거를 아직 고르지 못했어요.".to_string()
  } else {
    let lead = selected_records
      .iter()
      .take(3)
      .map(|record| {
        format!(
          "{} / {} / {}",
          strategy_main_actor_label(record),
          strategy_store_label(record),
          strategy_trim(record.summary.trim(), 58)
        )
      })
      .collect::<Vec<_>>()
      .join(" -> ");
    format!(
      "질문과 사건 맥락을 기준으로 기록을 다시 뽑아보니, 흐름의 중심은 {} 입니다.",
      strategy_trim(&lead, 220)
    )
  };

  let timeline_summary = selected
    .iter()
    .filter_map(|(id, reasons, score)| {
      let record = by_id.get(id)?;
      let extra = summarize_strategy_record_parts(record);
      let mut line = format!(
        "{} · {} · {} · {}",
        record.ts.trim(),
        strategy_main_actor_label(record),
        strategy_trim(record.summary.trim(), 72),
        if extra.is_empty() { "핵심 흐름".to_string() } else { extra }
      );
      if !reasons.is_empty() {
        line.push_str(&format!(" · 선택 이유 {}", strategy_trim(&reasons.join(", "), 80)));
      }
      if *score > 0.0 {
        line.push_str(&format!(" · score {:.2}", score));
      }
      Some(strategy_trim(&line, 280))
    })
    .take(6)
    .collect::<Vec<_>>();

  let evidence_records = selected_records
    .iter()
    .enumerate()
    .map(|(idx, record)| {
      let ref_id = format!("E{}", idx + 1);
      let lookup = selected
        .iter()
        .find(|(id, _, _)| id == &record.id)
        .cloned()
        .unwrap_or_else(|| (record.id.clone(), vec!["핵심 근거".to_string()], 0.0));
      let extra = summarize_strategy_record_parts(record);
      let summary = if extra.is_empty() {
        strategy_trim(record.summary.trim(), 220)
      } else {
        strategy_trim(&format!("{} | {}", record.summary.trim(), extra), 220)
      };
      StrategyEvidenceRecord {
        ref_id,
        record_id: record.id.clone(),
        ts: record.ts.trim().to_string(),
        actor: strategy_main_actor_label(record),
        place: strategy_place_label(record),
        store: strategy_store_label(record),
        summary,
        score: lookup.2,
        risk_label: String::new(),
        reasons: lookup.1,
      }
    })
    .collect::<Vec<_>>();

  (
    StrategyEvidencePacket {
      mode: if case_item.is_some() { "case-linked".to_string() } else { "direct".to_string() },
      case_title,
      focus_summary,
      overview,
      actor_summary,
      timeline_summary,
      risk_summary: Vec::new(),
      gaps,
      evidence_records,
      legal_references,
    },
    retrieval_query,
  )
}

fn summarize_conversation(history: &[StrategyChatTurn]) -> String {
  if history.is_empty() {
    return "- 이전 대화 없음".to_string();
  }
  history
    .iter()
    .rev()
    .take(4)
    .collect::<Vec<_>>()
    .into_iter()
    .rev()
    .map(|turn| {
      let role = if turn.role.trim() == "user" { "사용자" } else { "어시스턴트" };
      format!("- {}: {}", role, strategy_trim(&strategy_sanitize_text(turn.content.trim()), 160))
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn build_strategy_system_prompt() -> String {
  [
    "- AI 언어모델의 이름은 \"CLOVA X\" 이며 네이버에서 만들었다.",
    "- 반드시 한국어로만 답하라.",
    "- 영어, 중국어, 일본어, 한자 사용 금지.",
    "- 역할: 학교 현장 분쟁·민원 대응을 돕는 증거 기반 전략자문 에이전트.",
    "- 반드시 제공된 사건 맥락과 증거 참조표만 근거로 답하라.",
    "- 관련 법령 참조표가 함께 주어지면 사건 증거와 연결되는 범위 안에서만 조심스럽게 활용하라.",
    "- 입력으로 주어진 사건 맥락, 증거 참조표, 법령 참조표 문구를 그대로 길게 다시 베끼지 말라.",
    "- 사건에 없는 사실을 추가하지 말고, 확실하지 않으면 모른다고 적어라.",
    "- 첫 문장에서 사용자의 질문에 직접 답하라.",
    "- 근거는 별도 '근거 묶음' 섹션으로 떼어내지 말고, 답변 문장 안에 자연스럽게 녹여라.",
    "- 핵심 판단이나 제안마다 가능하면 [E1], [E2] 형식의 근거 표기를 문장 안에 붙여라.",
    "- 법령을 언급할 때는 가능하면 [L1], [L2]처럼 표시하고, 조문 취지와 현장 적용 포인트만 짧게 연결하라.",
    "- 답변은 교사가 바로 복사해 쓸 수 있게 실무적으로 작성하라.",
    "- 과도한 법률 단정이나 최종 법률판단은 피하고, 기록·증거·말의 톤·다음 행동 중심으로 답하라.",
    "- 응답은 대화형으로 자연스럽게 이어가되, 필요하면 짧은 bullet만 사용하라.",
    "- 응답은 가능하면 1) 상황판단 2) 지금 먼저 할 말 3) 바로 남길 기록 4) 다음 행동 순서를 자연스럽게 따른다.",
  ].join("\n")
}

fn build_strategy_record_system_prompt() -> String {
  [
    "- AI 언어모델의 이름은 \"CLOVA X\" 이며 네이버에서 만들었다.",
    "- 반드시 한국어로만 답하라.",
    "- 역할: 학교 현장 사안을 빠른 기록 양식으로 정리하는 기록작성 에이전트.",
    "- 제공된 사용자 입력과 참고 기록만 근거로 기록 초안을 작성하라.",
    "- 사건에 없는 사실을 덧붙이지 말고, 모르는 값은 '미상'이라고 적어라.",
    "- 문체는 감정적 해석보다 중립적 사실 정리 중심으로 유지하라.",
    "- 빠르게 적는 수준에서 끝내지 말고, 나중에 다시 봐도 맥락이 살아 있도록 실질적이고 촘촘한 기록으로 써라.",
    "- 먼저 사건의 큰 흐름과 섹션 구조를 안정적으로 세우고, 그 안을 구체적인 사실과 조치 내용으로 채우는 방식으로 작성하라.",
    "- [상황 요약]은 단순 한 줄이 아니라, 누가 언제 어디서 무엇을 했는지 한 번에 읽히는 탄탄한 문단으로 작성하라.",
    "- [배경 흐름]에는 사건 직전 맥락, 이전 갈등, 선행 조치가 있으면 빠뜨리지 말고 정리하라.",
    "- [핵심 포인트]에는 위험 신호, 확인이 필요한 점, 즉시 남겨야 할 포인트를 분리해서 적어라.",
    "- [관련 자료]에는 이미 있는 증거, 추가로 확보할 자료, 확인한 사람을 실무적으로 정리하라.",
    "- [내 대응 메모]에는 교사가 실제로 한 말, 즉시 취한 조치, 후속 안내 계획, 상대에게 전달한 설명을 구체적으로 적어라.",
    "- 값이 모호하면 짧게 비워두지 말고 왜 미상인지 또는 무엇을 추가 확인해야 하는지 적어라.",
    "- 출력은 반드시 지정된 섹션만, 지정된 순서대로 작성하라.",
    "- 섹션 제목 앞뒤에 다른 안내 문장이나 인사말을 붙이지 말라.",
    "- 기록 초안 마지막에 설명, 주의사항, 요약 코멘트를 덧붙이지 말라.",
    "- 아래 섹션을 그대로 사용하라:",
    "[기록 기본정보]",
    "[상황 요약]",
    "[배경 흐름]",
    "[핵심 포인트]",
    "[관련 자료]",
    "[내 대응 메모]",
    "[추가 메모]",
    "- [기록 기본정보]에는 다음 항목을 bullet로 적어라:",
    "- 기록 시각:",
    "- 주체:",
    "- 상대방:",
    "- 위치/채널:",
    "- 자료 형태:",
    "- 민감도:",
  ].join("\n")
}

fn build_strategy_record_hybrid_system_prompt() -> String {
  [
    "- AI 언어모델의 이름은 \"CLOVA X\" 이며 네이버에서 만들었다.",
    "- 반드시 한국어로만 답하라.",
    "- 역할: 두 개의 기록 초안을 하나의 빠른캡쳐형 기록 초안으로 합치는 기록 편집 에이전트.",
    "- HyperCLOVA-X 초안은 최종 기록의 기본 뼈대이며, 사실관계·시간 흐름·누락 보완·보수적 표현을 우선 기준으로 삼아라.",
    "- Roosy-X 초안은 읽기 흐름, 현장형 표현, 저장 전 문장 다듬기와 실무 메모 보강용으로만 활용하라.",
    "- 최종 초안은 HyperCLOVA-X 초안을 기준으로 만들고, Roosy-X 초안의 장점은 가독성과 자연스러운 표현 보완에 제한적으로 반영하라.",
    "- 출력은 반드시 지정된 섹션 제목과 순서만 사용하라.",
    "- 입력에 없는 사실을 새로 만들지 말고, 불확실한 값은 '미상' 또는 '추가 확인 필요'로 적어라.",
    "- [상황 요약]은 한 번에 읽히는 1개 문단으로 쓰되, 누가·언제·어디서·무엇을·어떻게·그 직후 어떤 조치가 있었는지 빠뜨리지 말라.",
    "- [배경 흐름], [핵심 포인트], [관련 자료], [내 대응 메모], [추가 메모]는 실무 검토와 사후 재구성에 충분할 정도로 구체적이고 체계적으로 작성하라.",
    "- 지나치게 짧게 줄이거나 문장감을 위해 정보를 덜어내지 말고, 추후 보고·상담·민원 대응에 바로 써먹을 수 있을 정도로 탄탄하게 적어라.",
    "- 기록 초안 마지막에 설명이나 주석을 덧붙이지 말라.",
    "- 가능하면 템플릿 평가 매트릭(충실성, 시간 흐름, 완결성, 자료 근거성, 중립성, 제출 가능성)을 마음속으로 먼저 점검한 뒤, 부족한 부분만 보완하라.",
  ].join("\n")
}

fn build_strategy_record_review_system_prompt() -> String {
  [
    "- AI 언어모델의 이름은 \"CLOVA X\" 이며 네이버에서 만들었다.",
    "- 반드시 한국어로만 답하라.",
    "- 역할: 기록 초안을 평가 매트릭 기준으로 스스로 점검하고, 필요한 부분만 보강하는 기록 품질 검토 에이전트.",
    "- 충실성, 시간 흐름, 완결성, 자료 근거성, 중립성, 제출 가능성을 기준으로 초안을 조용히 점검하라.",
    "- 입력에 없는 사실은 절대 추가하지 말라.",
    "- 초안이 이미 적절하면 구조는 그대로 유지하되, 누락값 표기와 문장 연결만 최소 보완하라.",
    "- 출력은 최종 기록 초안 본문만 하라. 점수, 평가표, 코멘트, 자기설명은 출력하지 말라.",
    "- 지정된 섹션 제목과 순서만 유지하라.",
    "- 같은 문장을 반복하거나 같은 섹션을 두 번 쓰지 말라.",
  ].join("\n")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StrategyStructuredReportDraft {
  summary: String,
  actors: Vec<String>,
  timeline: Vec<String>,
  issues: Vec<String>,
  evidence: Vec<String>,
  recommended_questions: Vec<String>,
}

fn build_strategy_record_renderer_system_prompt() -> String {
  [
    "- AI 언어모델의 이름은 \"CLOVA X\" 이며 네이버에서 만들었다.",
    "- 반드시 한국어로만 답하라.",
    "- 역할: 기록 초안을 구조화 JSON으로 정리하는 resident report planner.",
    "- 최종 문서의 헤더, 섹션 제목, 고정 표기는 앱이 렌더링하므로 너는 사실 내용만 JSON 값으로 작성하라.",
    "- 아래 6개 키만 포함한 단일 JSON object만 출력하라.",
    "- 마크다운, 코드펜스, 설명 문장, 주석, 서문, 후기는 금지한다.",
    "- 키는 정확히 summary, actors, timeline, issues, evidence, recommended_questions만 사용하라.",
    "- summary는 하나의 촘촘한 문단 문자열로 쓴다.",
    "- actors, timeline, issues, evidence, recommended_questions는 문자열 배열로 쓴다.",
    "- 입력에 없는 사실은 추가하지 말고, 불확실하면 '미상' 또는 '추가 확인 필요'처럼 적어라.",
    "- recommended_questions는 후속 확인 질문 또는 추가 메모 포인트만 간결하게 넣어라.",
    "- 절대 섹션 제목([기록 기본정보] 등)을 다시 출력하지 말라.",
  ].join("\n")
}

fn build_strategy_record_renderer_user_prompt(
  evidence_packet: &StrategyEvidencePacket,
  case_item: Option<&CaseItem>,
  message: &str,
  strategy_note: Option<&str>,
  draft_answer: &str,
) -> String {
  let case_block = summarize_case_context(case_item);
  let note_block = strategy_trim(&strategy_sanitize_text(strategy_note.unwrap_or("없음")), 220);
  let question_block = strategy_trim(&strategy_sanitize_text(message.trim()), 320);
  let evidence_block = if evidence_packet.evidence_records.is_empty() {
    "- 참고 기록 없음".to_string()
  } else {
    evidence_packet
      .evidence_records
      .iter()
      .take(3)
      .map(build_strategy_record_line_for_prompt)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let legal_block = evidence_packet
    .legal_references
    .iter()
    .take(2)
    .map(build_strategy_legal_line_for_prompt)
    .map(|line| format!("- {}", line))
    .collect::<Vec<_>>()
    .join("\n");
  let gap_block = if evidence_packet.gaps.is_empty() {
    "- 특별히 비어 있는 정보 없음".to_string()
  } else {
    evidence_packet
      .gaps
      .iter()
      .take(4)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let compact_draft = {
    let mut parts = Vec::<String>::new();
    for header in ["[상황 요약]", "[배경 흐름]", "[핵심 포인트]", "[관련 자료]", "[내 대응 메모]", "[추가 메모]"] {
      let body = strategy_extract_record_section_body(draft_answer, header);
      if !body.trim().is_empty() {
        parts.push(format!("{}\n{}", header, strategy_trim(&body, 320)));
      }
    }
    if parts.is_empty() {
      strategy_trim(draft_answer.trim(), 1600)
    } else {
      strategy_trim(&parts.join("\n\n"), 1600)
    }
  };

  strategy_sanitize_text(&format!(
    "[사용자 입력 상황]\n{}\n\n[사건 맥락]\n{}\n\n[참고 기록]\n{}\n\n[관련 법령/참고 기준]\n{}\n\n[확인 필요]\n{}\n\n[전략 메모]\n{}\n\n[현재 기록 초안]\n{}\n\n[JSON 작성 규칙]\n- summary: 사건 흐름과 직후 조치를 한 문단으로\n- actors: 핵심 인물/역할만 2~6개 문자열 배열\n- timeline: 사건 직전 맥락/배경 흐름/선행 조치 배열\n- issues: 핵심 쟁점/위험 신호/즉시 확인 포인트 배열\n- evidence: 이미 있는 자료 + 추가 확보 자료 배열\n- recommended_questions: 후속 확인 질문 또는 추가 메모 포인트 배열\n- JSON 외 다른 텍스트는 절대 출력하지 말라",
    question_block,
    case_block,
    evidence_block,
    legal_block,
    gap_block,
    note_block,
    compact_draft,
  ))
}

fn strategy_extract_json_object(raw: &str) -> Option<String> {
  let mut start = None;
  let mut depth = 0_i32;
  let mut in_string = false;
  let mut escaped = false;
  for (idx, ch) in raw.char_indices() {
    if in_string {
      if escaped {
        escaped = false;
      } else if ch == '\\' {
        escaped = true;
      } else if ch == '"' {
        in_string = false;
      }
      continue;
    }
    if ch == '"' {
      in_string = true;
      continue;
    }
    if ch == '{' {
      if start.is_none() {
        start = Some(idx);
      }
      depth += 1;
    } else if ch == '}' {
      depth -= 1;
      if depth == 0 {
        let start_idx = start?;
        return Some(raw[start_idx..=idx].to_string());
      }
    }
  }
  None
}

fn strategy_parse_structured_report_draft(raw: &str) -> Option<StrategyStructuredReportDraft> {
  serde_json::from_str::<StrategyStructuredReportDraft>(raw.trim())
    .ok()
    .or_else(|| strategy_extract_json_object(raw).and_then(|json| serde_json::from_str::<StrategyStructuredReportDraft>(&json).ok()))
}

fn strategy_split_renderer_items(input: &str, max_items: usize) -> Vec<String> {
  let normalized = input
    .replace(" / ", "\n")
    .replace("·", "\n")
    .replace("•", "\n")
    .replace("①", "\n")
    .replace("②", "\n")
    .replace("③", "\n")
    .replace("④", "\n");
  normalized
    .lines()
    .map(|line| line.trim().trim_start_matches('-').trim())
    .filter(|line| !line.is_empty())
    .map(|line| strategy_trim(line, 180))
    .filter(|line| !line.is_empty())
    .take(max_items)
    .collect::<Vec<_>>()
}

fn strategy_estimate_structured_report_tokens(structured: &StrategyStructuredReportDraft) -> usize {
  let joined = [
    structured.summary.clone(),
    structured.actors.join(" "),
    structured.timeline.join(" "),
    structured.issues.join(" "),
    structured.evidence.join(" "),
    structured.recommended_questions.join(" "),
  ]
  .join(" ");
  strategy_approx_output_tokens(&joined)
}

fn strategy_build_structured_report_from_record_sections(
  evidence_packet: &StrategyEvidencePacket,
  message: &str,
  draft_answer: &str,
) -> Option<StrategyStructuredReportDraft> {
  let summary = strategy_extract_record_section_body(draft_answer, "[상황 요약]");
  let timeline_body = strategy_extract_record_section_body(draft_answer, "[배경 흐름]");
  let issues_body = strategy_extract_record_section_body(draft_answer, "[핵심 포인트]");
  let evidence_body = strategy_extract_record_section_body(draft_answer, "[관련 자료]");
  let response_body = strategy_extract_record_section_body(draft_answer, "[내 대응 메모]");
  let notes_body = strategy_extract_record_section_body(draft_answer, "[추가 메모]");

  if summary.trim().is_empty()
    && timeline_body.trim().is_empty()
    && issues_body.trim().is_empty()
    && evidence_body.trim().is_empty()
  {
    return None;
  }

  let mut actors = strategy_guess_record_people(message);
  if actors.is_empty() {
    let mut collected = Vec::<String>::new();
    for actor in &evidence_packet.actor_summary {
      for item in actor
        .split(|ch| matches!(ch, ',' | '·' | '/' | '|' | ';'))
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
      {
        if !collected
          .iter()
          .any(|existing| strategy_compact_text(existing) == strategy_compact_text(&item))
        {
          collected.push(item);
        }
      }
    }
    for record in &evidence_packet.evidence_records {
      let item = record.actor.trim().to_string();
      if !item.is_empty()
        && !collected
          .iter()
          .any(|existing| strategy_compact_text(existing) == strategy_compact_text(&item))
      {
        collected.push(item);
      }
    }
    actors = collected;
  }

  let evidence_items = {
    let mut items = strategy_split_renderer_items(&evidence_body, 6);
    if items.is_empty() {
      items = evidence_packet
        .evidence_records
        .iter()
        .take(4)
        .map(build_strategy_record_line_for_prompt)
        .map(|line| strategy_trim(&line, 180))
        .collect::<Vec<_>>();
    }
    items
  };

  let recommended_questions = {
    let mut items = strategy_split_renderer_items(&notes_body, 4);
    let response_items = strategy_split_renderer_items(&response_body, 3);
    if items.is_empty() {
      items = response_items;
    } else {
      for item in response_items {
        if !items.iter().any(|existing| strategy_compact_text(existing) == strategy_compact_text(&item)) {
          items.push(item);
        }
      }
    }
    items
  };

  Some(StrategyStructuredReportDraft {
    summary: if summary.trim().is_empty() {
      strategy_trim(&strategy_sanitize_text(message), 280)
    } else {
      strategy_trim(&summary, 600)
    },
    actors: actors.into_iter().take(6).collect::<Vec<_>>(),
    timeline: strategy_split_renderer_items(&timeline_body, 5),
    issues: strategy_split_renderer_items(&issues_body, 5),
    evidence: evidence_items,
    recommended_questions,
  })
}

fn strategy_extract_record_section_body(input: &str, target_header: &str) -> String {
  let mut current_header = String::new();
  let mut lines = Vec::<String>::new();
  for line in input.lines() {
    let trimmed = line.trim();
    if strategy_is_record_section_header(trimmed) {
      current_header = trimmed.to_string();
      continue;
    }
    if current_header == target_header && !trimmed.is_empty() && !strategy_is_record_tail_noise(trimmed) {
      lines.push(strategy_sanitize_text(trimmed));
    }
  }
  lines.join("\n")
}

fn strategy_extract_action_notes_from_message(message: &str) -> Vec<String> {
  let mut out = Vec::<String>::new();
  let mut buf = String::new();
  let mut capturing = false;
  for ch in message.chars() {
    if ch == '#' {
      if capturing {
        let note = strategy_sanitize_text(buf.trim());
        if !note.is_empty() {
          out.push(note);
        }
        buf.clear();
      }
      capturing = !capturing;
      continue;
    }
    if capturing {
      buf.push(ch);
    }
  }
  out
}

fn strategy_render_record_structured_answer(
  structured: &StrategyStructuredReportDraft,
  evidence_packet: &StrategyEvidencePacket,
  message: &str,
  draft_answer: &str,
) -> (String, usize, usize) {
  let people = strategy_guess_record_people(message);
  let actor = people
    .get(0)
    .cloned()
    .or_else(|| structured.actors.first().cloned())
    .unwrap_or_else(|| "당사자 미상".to_string());
  let counterpart = people
    .get(1)
    .cloned()
    .or_else(|| structured.actors.get(1).cloned())
    .unwrap_or_else(|| "상대방 미상".to_string());
  let place_hint = strategy_guess_record_place_hint(message);
  let action_notes = strategy_extract_action_notes_from_message(message);
  let recommended_questions = structured
    .recommended_questions
    .iter()
    .map(|item| strategy_sanitize_text(item.trim()))
    .filter(|item| !item.is_empty())
    .collect::<Vec<_>>();
  let evidence_lines = structured
    .evidence
    .iter()
    .map(|item| strategy_sanitize_text(item.trim()))
    .filter(|item| !item.is_empty())
    .collect::<Vec<_>>();
  let issue_lines = structured
    .issues
    .iter()
    .map(|item| strategy_sanitize_text(item.trim()))
    .filter(|item| !item.is_empty())
    .collect::<Vec<_>>();
  let timeline_lines = structured
    .timeline
    .iter()
    .map(|item| strategy_sanitize_text(item.trim()))
    .filter(|item| !item.is_empty())
    .collect::<Vec<_>>();
  let summary = strategy_trim(
    if structured.summary.trim().is_empty() {
      strategy_extract_record_section_body(draft_answer, "[상황 요약]")
    } else {
      strategy_sanitize_text(structured.summary.trim())
    }
    .trim(),
    1200,
  );
  let response_memo = if !action_notes.is_empty() {
    action_notes.join(" / ")
  } else {
    let extracted = strategy_extract_record_section_body(draft_answer, "[내 대응 메모]");
    if extracted.trim().is_empty() {
      "직접 입력된 대응 메모는 없으며, 저장 전 실제 중재·분리·안내 내용을 보강할 필요가 있다.".to_string()
    } else {
      extracted
    }
  };
  let additional_memo = if !recommended_questions.is_empty() {
    recommended_questions
      .iter()
      .map(|item| format!("- {}", item))
      .collect::<Vec<_>>()
      .join("\n")
  } else {
    let extracted = strategy_extract_record_section_body(draft_answer, "[추가 메모]");
    if extracted.trim().is_empty() {
      "추가 확인 질문이 없으면 현재 상태와 후속 관찰 필요 사항을 짧게 이어서 보완한다.".to_string()
    } else {
      extracted
    }
  };

  let rendered = format!(
    "[기록 기본정보]\n- 기록 시각: {}\n- 주체: {}\n- 상대방: {}\n- 위치/채널: {}\n- 자료 형태: 직접 입력\n- 민감도: 보통\n\n[상황 요약]\n{}\n\n[배경 흐름]\n{}\n\n[핵심 포인트]\n{}\n\n[관련 자료]\n{}\n\n[내 대응 메모]\n{}\n\n[추가 메모]\n{}",
    Local::now().format("%Y.%m.%d %H:%M"),
    actor,
    counterpart,
    place_hint,
    if summary.trim().is_empty() {
      "상황 요약이 비어 있어 추가 확인이 필요하다.".to_string()
    } else {
      summary
    },
    if timeline_lines.is_empty() {
      let extracted = strategy_extract_record_section_body(draft_answer, "[배경 흐름]");
      if extracted.trim().is_empty() {
        "사건 직전 맥락과 선행 갈등 여부는 추가 확인이 필요하다.".to_string()
      } else {
        extracted
      }
    } else {
      timeline_lines.join("\n")
    },
    if issue_lines.is_empty() {
      let extracted = strategy_extract_record_section_body(draft_answer, "[핵심 포인트]");
      if extracted.trim().is_empty() {
        "즉시 확인할 쟁점과 위험 신호를 추가로 정리할 필요가 있다.".to_string()
      } else {
        extracted
      }
    } else {
      issue_lines.join("\n")
    },
    if evidence_lines.is_empty() {
      let extracted = strategy_extract_record_section_body(draft_answer, "[관련 자료]");
      if extracted.trim().is_empty() {
        if evidence_packet.evidence_records.is_empty() {
          "현재 연결된 참고 기록은 없으며, 당사자 진술과 주변 관찰 자료를 추가 확보할 필요가 있다.".to_string()
        } else {
          format!(
            "현재 연결된 참고 기록 {}건을 우선 근거로 삼고, 필요 시 추가 메모와 주변 관찰 자료를 확보한다.",
            evidence_packet.evidence_records.len()
          )
        }
      } else {
        extracted
      }
    } else {
      evidence_lines.join("\n")
    },
    response_memo,
    additional_memo,
  );
  let draft_proposal = strategy_build_draft_proposal(
    HybridStage::RecordReview,
    "template",
    STRATEGY_MODEL_DEFAULT_ID,
    true,
    None,
    16,
  );
  let renderer_inserted_tokens =
    strategy_approx_output_tokens(&draft_proposal.rendered_fragments.join(" "));
  let llm_generated_tokens = strategy_approx_output_tokens(
    &serde_json::to_string(structured).unwrap_or_else(|_| structured.summary.clone()),
  );
  (rendered, renderer_inserted_tokens, llm_generated_tokens)
}

fn build_strategy_hybrid_system_prompt() -> String {
  [
    "- AI 언어모델의 이름은 \"CLOVA X\" 이며 네이버에서 만들었다.",
    "- 반드시 한국어로만 답하라.",
    "- 역할: 두 개의 전략자문 초안을 하나의 최종 답변으로 합치는 편집 에이전트.",
    "- 최종 문장과 답변 흐름은 Roosy-X 초안을 기본으로 삼고, HyperCLOVA-X 초안은 사실 과장 방지와 근거 정렬용으로 활용하라.",
    "- HyperCLOVA-X 초안의 근거성·균형감은 안전장치로 쓰고, Roosy-X 초안의 직관성·실무 문장은 전면에 세워라.",
    "- 입력 초안을 비교평가하지 말고, 사용자에게 바로 보여줄 최종 답변만 작성하라.",
    "- 사건·증거·법령은 입력에 포함된 범위만 사용하라.",
    "- 사실 단정은 조심하고, 확실하지 않은 부분은 확인 필요로 표현하라.",
    "- 첫 문장에서 사용자의 질문에 직접 답하라.",
    "- 근거는 문장 안에 자연스럽게 [E1], [L1]처럼 녹여라.",
    "- 답변은 가능하면 1) 상황판단 2) 지금 먼저 할 말 3) 바로 남길 기록 4) 다음 행동 순서를 자연스럽게 따른다.",
    "- 초안 문장을 그대로 길게 이어붙이지 말고, 하나의 매끈한 최종 한국어 답변으로 정리하라.",
    "- 증거 목록을 길게 다시 나열하지 말고, 결론을 뒷받침하는 핵심 근거만 짧게 묶어 설명하라.",
  ].join("\n")
}

fn build_strategy_user_prompt(
  evidence_packet: &StrategyEvidencePacket,
  case_item: Option<&CaseItem>,
  message: &str,
  strategy_note: Option<&str>,
  conversation: &[StrategyChatTurn],
) -> String {
  let case_block = summarize_case_context(case_item);
  let records_block = if evidence_packet.evidence_records.is_empty() {
    "- 연결된 증거 없음".to_string()
  } else {
    evidence_packet
      .evidence_records
      .iter()
      .map(build_strategy_record_line_for_prompt)
      .collect::<Vec<_>>()
      .join("\n")
  };
  let note_block = strategy_trim(&strategy_sanitize_text(strategy_note.unwrap_or("없음")), 320);
  let history_block = summarize_conversation(conversation);
  let question_block = strategy_trim(&strategy_sanitize_text(message.trim()), 360);
  let question_focus_block = strategy_question_focus_hint(message)
    .unwrap_or_else(|| "- 이번 질문의 핵심 의도를 첫 문장에서 직접 답하라.".to_string());
  let actor_block = if evidence_packet.actor_summary.is_empty() {
    "- 정리된 인물 없음".to_string()
  } else {
    evidence_packet
      .actor_summary
      .iter()
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let gap_block = if evidence_packet.gaps.is_empty() {
    "- 특별히 비어 있는 정보 없음".to_string()
  } else {
    evidence_packet
      .gaps
      .iter()
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let timeline_block = if evidence_packet.timeline_summary.is_empty() {
    "- 시간 흐름 요약 없음".to_string()
  } else {
    evidence_packet
      .timeline_summary
      .iter()
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let legal_block = if evidence_packet.legal_references.is_empty() {
    "- 바로 연결된 법령 없음".to_string()
  } else {
    evidence_packet
      .legal_references
      .iter()
      .map(build_strategy_legal_line_for_prompt)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };

  strategy_sanitize_text(&format!(
    "[현재 사건 맥락]\n{}\n\n[증거 패킷 요약]\n- {}\n- {}\n\n[핵심 인물]\n{}\n\n[시간 흐름]\n{}\n\n[비어 있는 정보]\n{}\n\n[증거 참조표]\n{}\n\n[관련 법령 참조표]\n{}\n\n[전략 메모]\n{}\n\n[직전 대화]\n{}\n\n[이번 요청]\n{}\n\n[질문 초점]\n{}\n\n[응답 조건]\n- 한국어만 사용\n- 학교 현장에서 바로 쓰는 표현\n- 첫 문장에서 질문에 직접 답할 것\n- 너무 긴 설명보다 핵심 위주\n- 필요한 경우 bullet 사용 가능\n- 사건에 없는 사실은 추정하지 말 것\n- '현재 근거 묶음 보기' 같은 별도 섹션 제목은 만들지 말 것\n- 근거는 답변 문장 안에 [E1]처럼 자연스럽게 섞어 쓸 것\n- 법령을 쓸 때는 [L1]처럼 자연스럽게 섞되, 최종 법률판단처럼 단정하지 말 것\n- 비어 있는 정보나 확인 필요 사항도 별도 큰 섹션보다 문장 말미에 자연스럽게 덧붙일 것\n- 근거가 약한 내용은 '확실하지 않음'이라고 쓸 것",
    case_block,
    evidence_packet.focus_summary,
    evidence_packet.overview,
    actor_block,
    timeline_block,
    gap_block,
    records_block,
    legal_block,
    note_block,
    history_block,
    question_block,
    question_focus_block
  ))
}

fn build_strategy_user_prompt_for_draft(
  evidence_packet: &StrategyEvidencePacket,
  case_item: Option<&CaseItem>,
  message: &str,
  strategy_note: Option<&str>,
) -> String {
  let case_block = summarize_case_context(case_item);
  let note_block = strategy_trim(&strategy_sanitize_text(strategy_note.unwrap_or("없음")), 180);
  let question_block = strategy_trim(&strategy_sanitize_text(message.trim()), 220);
  let question_focus_block = strategy_question_focus_hint(message)
    .unwrap_or_else(|| "- 첫 문장에서 질문에 직접 답하고, 바로 실무 판단으로 이어가라.".to_string());
  let evidence_block = if evidence_packet.evidence_records.is_empty() {
    "- 연결된 핵심 근거 없음".to_string()
  } else {
    evidence_packet
      .evidence_records
      .iter()
      .take(3)
      .map(build_strategy_record_line_for_prompt)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let legal_block = if !strategy_question_needs_legal_refs(message) || evidence_packet.legal_references.is_empty() {
    "- 이번 질문에서 법령 직접 인용은 우선순위가 낮음".to_string()
  } else {
    evidence_packet
      .legal_references
      .iter()
      .take(2)
      .map(build_strategy_legal_line_for_prompt)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };

  strategy_sanitize_text(&format!(
    "[현재 사건 맥락]\n{}\n\n[핵심 근거]\n{}\n\n[관련 법령]\n{}\n\n[전략 메모]\n{}\n\n[질문]\n{}\n\n[질문 초점]\n{}\n\n[응답 조건]\n- 첫 문장에서 질문에 직접 답할 것\n- 증거 목록을 길게 다시 늘어놓지 말 것\n- 학교 현장에서 바로 쓸 수 있는 한국어 문장으로 답할 것\n- 너무 긴 설명보다 결론과 행동 제안을 먼저 줄 것",
    case_block,
    evidence_block,
    legal_block,
    note_block,
    question_block,
    question_focus_block
  ))
}

fn build_strategy_record_user_prompt(
  evidence_packet: &StrategyEvidencePacket,
  case_item: Option<&CaseItem>,
  message: &str,
  strategy_note: Option<&str>,
  conversation: &[StrategyChatTurn],
) -> String {
  let case_block = summarize_case_context(case_item);
  let note_block = strategy_trim(&strategy_sanitize_text(strategy_note.unwrap_or("없음")), 220);
  let question_block = strategy_trim(&strategy_sanitize_text(message.trim()), 520);
  let history_block = summarize_conversation(conversation);
  let actor_block = if evidence_packet.actor_summary.is_empty() {
    "- 정리된 인물 없음".to_string()
  } else {
    evidence_packet
      .actor_summary
      .iter()
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let timeline_block = if evidence_packet.timeline_summary.is_empty() {
    "- 시간 흐름 참고 없음".to_string()
  } else {
    evidence_packet
      .timeline_summary
      .iter()
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let records_block = if evidence_packet.evidence_records.is_empty() {
    "- 참고 기록 없음".to_string()
  } else {
    evidence_packet
      .evidence_records
      .iter()
      .take(5)
      .map(build_strategy_record_line_for_prompt)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let gap_block = if evidence_packet.gaps.is_empty() {
    "- 특별히 비어 있는 정보 없음".to_string()
  } else {
    evidence_packet
      .gaps
      .iter()
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let legal_block = build_strategy_record_legal_prompt_block(evidence_packet);
  let template_block = build_strategy_record_template_prompt_block(evidence_packet, message);

  strategy_sanitize_text(&format!(
    "[사용자 입력 상황]\n{}\n\n[사건 맥락]\n{}\n\n[참고 인물]\n{}\n\n[참고 시간 흐름]\n{}\n\n[참고 기록]\n{}\n\n[관련 법령/참고 기준]\n{}\n\n{}\n\n[확인이 더 필요한 부분]\n{}\n\n[전략 메모]\n{}\n\n[직전 대화]\n{}\n\n[출력 규칙]\n- 반드시 지정된 섹션 제목만 사용할 것\n- 사건에 없는 사실은 만들지 말 것\n- 값이 불확실하면 '미상' 또는 '추가 확인 필요'로 표기할 것\n- [상황 요약]에는 누가, 언제, 어디서, 어떤 흐름으로 상황이 벌어졌는지뿐 아니라, 직후 조치와 현재 상태까지 한 번에 읽히는 핵심 기록 문단을 쓸 것\n- [배경 흐름], [핵심 포인트], [관련 자료], [내 대응 메모], [추가 메모]는 각각 2~5문장 안에서 탄탄하고 실무적으로 정리할 것\n- 가능하면 시간 흐름, 직접 관찰한 사실, 들은 내용, 추가 확인 필요 사항을 헷갈리지 않게 분리해서 적을 것\n- 짧은 메모처럼 끝내지 말고, 나중에 다시 봐도 상황이 재구성될 정도로 정보를 남길 것\n- 참고 법령이 있다면 사실관계와 조치 기준을 보강하는 정도로만 활용하고, 법률 해석처럼 길게 쓰지 말 것",
    question_block,
    case_block,
    actor_block,
    timeline_block,
    records_block,
    legal_block,
    template_block,
    gap_block,
    note_block,
    history_block,
  ))
}

fn build_strategy_record_fill_user_prompt(
  evidence_packet: &StrategyEvidencePacket,
  case_item: Option<&CaseItem>,
  message: &str,
  strategy_note: Option<&str>,
  hyper_answer: &str,
) -> String {
  let case_block = summarize_case_context(case_item);
  let note_block = strategy_trim(&strategy_sanitize_text(strategy_note.unwrap_or("없음")), 220);
  let question_block = strategy_trim(&strategy_sanitize_text(message.trim()), 520);
  let evidence_block = if evidence_packet.evidence_records.is_empty() {
    "- 참고 기록 없음".to_string()
  } else {
    evidence_packet
      .evidence_records
      .iter()
      .take(5)
      .map(build_strategy_record_line_for_prompt)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let legal_block = build_strategy_record_legal_prompt_block(evidence_packet);
  let template_block = build_strategy_record_template_prompt_block(evidence_packet, message);
  let gap_block = if evidence_packet.gaps.is_empty() {
    "- 특별히 비어 있는 정보 없음".to_string()
  } else {
    evidence_packet
      .gaps
      .iter()
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };

  strategy_sanitize_text(&format!(
    "[사용자 입력 상황]\n{}\n\n[사건 맥락]\n{}\n\n[참고 기록]\n{}\n\n[관련 법령/참고 기준]\n{}\n\n{}\n\n[추가 확인 필요]\n{}\n\n[전략 메모]\n{}\n\n[HyperCLOVA-X 기록 골격]\n{}\n\n[작성 지침]\n- HyperCLOVA-X 기록 골격의 섹션 구조와 사실 흐름을 절대 무너뜨리지 말라.\n- 너의 역할은 골격 안을 채우는 것이다. 각 섹션을 더 구체적이고 실무적으로 채우되, 입력에 없는 사실은 새로 만들지 말라.\n- [상황 요약]은 누가, 언제, 어디서, 어떤 상황에서 무엇을 했고 직후 조치가 무엇이었는지 한 번에 읽히도록 촘촘한 문단으로 채워라.\n- [배경 흐름]은 이전 갈등, 직전 맥락, 반복 징후, 선행 안내가 보이면 꼭 보강하라.\n- [핵심 포인트]는 사후 검토에 필요한 쟁점을 번호감 없이 명확히 채워라.\n- [관련 자료]는 이미 있는 자료, 바로 추가 확보할 자료, 확인이 필요한 사람과 채널을 실무적으로 채워라.\n- [내 대응 메모]는 실제로 한 말, 즉시 취한 조치, 설명 내용, 후속 계획을 빠뜨리지 말고 구체적으로 적어라.\n- 문장감은 자연스럽게 다듬되, 사실 밀도와 체계성을 절대 줄이지 말라.\n- 동일한 문장을 반복하거나 같은 섹션을 두 번 쓰지 말라.\n- 지정된 섹션 제목만 사용하고, 마지막에 설명 문장을 붙이지 말라.",
    question_block,
    case_block,
    evidence_block,
    legal_block,
    template_block,
    gap_block,
    note_block,
    strategy_trim(hyper_answer.trim(), 3200),
  ))
}

fn build_strategy_record_hybrid_user_prompt(
  evidence_packet: &StrategyEvidencePacket,
  case_item: Option<&CaseItem>,
  message: &str,
  strategy_note: Option<&str>,
  hyper_answer: &str,
  roosy_answer: &str,
) -> String {
  let case_block = summarize_case_context(case_item);
  let note_block = strategy_trim(&strategy_sanitize_text(strategy_note.unwrap_or("없음")), 220);
  let question_block = strategy_trim(&strategy_sanitize_text(message.trim()), 520);
  let evidence_block = if evidence_packet.evidence_records.is_empty() {
    "- 참고 기록 없음".to_string()
  } else {
    evidence_packet
      .evidence_records
      .iter()
      .take(5)
      .map(build_strategy_record_line_for_prompt)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let gap_block = if evidence_packet.gaps.is_empty() {
    "- 특별히 비어 있는 정보 없음".to_string()
  } else {
    evidence_packet
      .gaps
      .iter()
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let legal_block = build_strategy_record_legal_prompt_block(evidence_packet);
  let template_block = build_strategy_record_template_prompt_block(evidence_packet, message);

  strategy_sanitize_text(&format!(
    "[사용자 입력 상황]\n{}\n\n[사건 맥락]\n{}\n\n[참고 기록]\n{}\n\n[관련 법령/참고 기준]\n{}\n\n{}\n\n[추가 확인 필요]\n{}\n\n[전략 메모]\n{}\n\n[HyperCLOVA-X 초안]\n{}\n\n[Roosy-X 초안]\n{}\n\n[합성 지침]\n- 최종 결과는 빠른캡쳐 저장 전에 그대로 검토할 수 있는 기록 초안이어야 한다.\n- HyperCLOVA-X 초안을 기본 원본이자 최종 구조 기준으로 삼고, 사실 흐름·누락 보완·보수적 표현을 우선 유지한다.\n- Roosy-X 초안은 각 섹션 안의 문장 연결, 실무 표현, 설명 밀도, 메모 구체화를 채워 넣는 보조 초안으로만 활용한다.\n- Roosy-X 때문에 섹션 구조가 바뀌거나 사실 흐름이 느슨해지면 안 된다.\n- 문장감을 위해 사실이나 디테일을 덜어내지 말고, 정보 밀도와 체계성을 우선한다.\n- legal 템플릿과 참고 기준에 어긋나는 모호한 표현은 HyperCLOVA-X 초안 기준으로 바로잡는다.\n- [상황 요약]은 촘촘한 한 문단으로 쓰고, 나머지 섹션도 실무 검토에 충분할 정도로 구체적으로 작성한다.\n- 동일한 문장을 반복하거나 같은 섹션을 두 번 쓰지 않는다.\n- 기록을 나중에 다시 열어도 상황이 재구성될 정도의 정보 밀도를 유지한다.\n- 지정된 섹션 제목만 사용하고, 그 밖의 설명 문장은 덧붙이지 않는다.",
    question_block,
    case_block,
    evidence_block,
    legal_block,
    template_block,
    gap_block,
    note_block,
    strategy_trim(hyper_answer.trim(), 2600),
    strategy_trim(roosy_answer.trim(), 2600),
  ))
}

fn build_strategy_record_review_user_prompt(
  evidence_packet: &StrategyEvidencePacket,
  case_item: Option<&CaseItem>,
  message: &str,
  strategy_note: Option<&str>,
  draft_answer: &str,
) -> String {
  let case_block = summarize_case_context(case_item);
  let note_block = strategy_trim(&strategy_sanitize_text(strategy_note.unwrap_or("없음")), 220);
  let question_block = strategy_trim(&strategy_sanitize_text(message.trim()), 520);
  let evidence_block = if evidence_packet.evidence_records.is_empty() {
    "- 참고 기록 없음".to_string()
  } else {
    evidence_packet
      .evidence_records
      .iter()
      .take(5)
      .map(build_strategy_record_line_for_prompt)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let legal_block = build_strategy_record_legal_prompt_block(evidence_packet);
  let template_block = build_strategy_record_template_prompt_block(evidence_packet, message);
  let gap_block = if evidence_packet.gaps.is_empty() {
    "- 특별히 비어 있는 정보 없음".to_string()
  } else {
    evidence_packet
      .gaps
      .iter()
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };

  strategy_sanitize_text(&format!(
    "[사용자 입력 상황]\n{}\n\n[사건 맥락]\n{}\n\n[참고 기록]\n{}\n\n[관련 법령/참고 기준]\n{}\n\n{}\n\n[추가 확인 필요]\n{}\n\n[전략 메모]\n{}\n\n[현재 기록 초안]\n{}\n\n[검토 지침]\n- 평가 매트릭과 보강 질문을 마음속으로 확인한 뒤, 충실성·시간 흐름·완결성·자료 근거성·중립성·제출 가능성을 높이도록 초안을 조용히 다듬어라.\n- 입력에 없는 사실, 발언, 시각, 장소, 자료를 절대 만들지 말라.\n- 누락 정보는 '미상', '입력 없음', '추가 확인 필요'로 명확히 표시하라.\n- 점수, 체크리스트, 자기평가, 해설 문장은 출력하지 말고 최종 기록 본문만 출력하라.\n- 초안이 이미 적절하면 구조는 유지하되 문장 중복, 모호한 표현, 누락값 처리만 최소 수정하라.",
    question_block,
    case_block,
    evidence_block,
    legal_block,
    template_block,
    gap_block,
    note_block,
    strategy_trim(draft_answer.trim(), 4200),
  ))
}

fn build_strategy_hybrid_user_prompt(
  evidence_packet: &StrategyEvidencePacket,
  case_item: Option<&CaseItem>,
  message: &str,
  strategy_note: Option<&str>,
  hyper_answer: &str,
  roosy_answer: &str,
) -> String {
  let case_block = summarize_case_context(case_item);
  let note_block = strategy_trim(&strategy_sanitize_text(strategy_note.unwrap_or("없음")), 220);
  let question_block = strategy_trim(&strategy_sanitize_text(message.trim()), 240);
  let question_focus_block = strategy_question_focus_hint(message)
    .unwrap_or_else(|| "- 이번 질문에 먼저 직접 답하고, 그다음 이유와 행동 제안을 붙여라.".to_string());
  let evidence_block = if evidence_packet.evidence_records.is_empty() {
    "- 연결된 핵심 근거 없음".to_string()
  } else {
    evidence_packet
      .evidence_records
      .iter()
      .take(4)
      .map(build_strategy_record_line_for_prompt)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let legal_block = if evidence_packet.legal_references.is_empty() {
    "- 관련 법령 없음".to_string()
  } else {
    evidence_packet
      .legal_references
      .iter()
      .take(3)
      .map(build_strategy_legal_line_for_prompt)
      .map(|line| format!("- {}", line))
      .collect::<Vec<_>>()
      .join("\n")
  };

  strategy_sanitize_text(&format!(
    "[현재 사건 맥락]\n{}\n\n[질문]\n{}\n\n[질문 초점]\n{}\n\n[전략 메모]\n{}\n\n[핵심 근거]\n{}\n\n[관련 법령]\n{}\n\n[HyperCLOVA-X 초안]\n{}\n\n[Roosy-X 초안]\n{}\n\n[합성 지침]\n- 최종 답변의 문장 흐름과 말투는 Roosy-X 초안을 기본으로 삼는다.\n- HyperCLOVA-X 초안은 과한 단정, 근거 누락, 법령 연결 오류를 바로잡는 안전 검토용으로 쓴다.\n- 첫 문장에서 사용자의 질문에 바로 답한다.\n- 둘을 비교하거나 '첫 번째 초안/두 번째 초안'이라고 설명하지 않는다.\n- 사용자에게 바로 전달할 하나의 최종 답변만 쓴다.\n- 대답 속에 [합성 지침], [추가 정보], [관련 법령] 같은 입력 헤더를 절대 다시 출력하지 않는다.\n- 증거 목록을 길게 다시 늘어놓지 말고, 결론을 뒷받침하는 핵심 근거만 짧게 묶어 설명한다.\n- 너무 짧게 줄이지 말고, 두 초안의 좋은 내용을 자연스럽게 충분히 녹여 길이감 있게 정리한다.\n- 답변은 상황판단, 지금 먼저 할 말, 바로 남길 기록, 다음 행동이 모두 드러나도록 3~6문단 정도의 완성형 답변으로 쓴다.\n- 답변은 자연스러운 문단형으로 쓰되, 꼭 필요할 때만 짧은 bullet을 사용한다.",
    case_block,
    question_block,
    question_focus_block,
    note_block,
    evidence_block,
    legal_block,
    strategy_trim(hyper_answer.trim(), 2200),
    strategy_trim(roosy_answer.trim(), 2200)
  ))
}

fn cleanup_strategy_output(raw: &str) -> String {
  let mut cleaned = String::with_capacity(raw.len());
  let mut chars = raw.chars().peekable();
  while let Some(ch) = chars.next() {
    if ch == '\u{1b}' {
      if matches!(chars.peek(), Some('[')) {
        chars.next();
        while let Some(next) = chars.next() {
          if ('@'..='~').contains(&next) {
            break;
          }
        }
        continue;
      }
      continue;
    }
    cleaned.push(ch);
  }

  let mut out = cleaned.replace('\r', "");
  if let Some(idx) = out.rfind("<|im_start|>assistant") {
    out = out[(idx + "<|im_start|>assistant".len())..].to_string();
  }
  out = out.replace("<|im_end|>", "");
  out = out.replace("<|endofturn|>", "");
  out = out.replace("<|stop|>", "");
  let mut filtered = Vec::<String>::new();
  let mut skipping_user_echo = false;
  for line in out.lines() {
    let trimmed = line.trim();
    let is_block_art = !trimmed.is_empty() && trimmed.chars().all(|ch| matches!(ch, '▄' | '█' | '▀' | ' '));
    if trimmed.contains('\u{fffd}') {
      continue;
    }
    if skipping_user_echo {
      if trimmed.is_empty() {
        skipping_user_echo = false;
      }
      continue;
    }
    if trimmed.starts_with('>') {
      skipping_user_echo = true;
      continue;
    }
    if trimmed.is_empty() {
      if filtered.last().is_some_and(|last| !last.is_empty()) {
        filtered.push(String::new());
      }
      continue;
    }
    if is_block_art
      || trimmed == ">>>"
      || trimmed == "..."
      || is_strategy_runtime_noise(trimmed)
      || trimmed.contains("(truncated)")
      || trimmed.starts_with("common params")
      || trimmed.starts_with("example-specific params")
      || trimmed.starts_with("Loading model...")
      || trimmed.starts_with("build")
      || trimmed.starts_with("model")
      || trimmed.starts_with("modalities")
      || trimmed.starts_with("available commands:")
      || trimmed.starts_with("/exit")
      || trimmed.starts_with("/regen")
      || trimmed.starts_with("/clear")
      || trimmed.starts_with("/read ")
      || trimmed.starts_with("/glob ")
      || trimmed.starts_with("[ Prompt:")
      || trimmed.starts_with("Exiting...")
      || trimmed.starts_with("llama_memory_breakdown_print:")
      || trimmed.starts_with("<|im_start|>")
      || trimmed.starts_with("<|im_end|>")
      || trimmed.starts_with("[현재 사건 맥락]")
      || trimmed.starts_with("[증거 패킷 요약]")
      || trimmed.starts_with("[핵심 인물]")
      || trimmed.starts_with("[시간 흐름]")
      || trimmed.starts_with("[비어 있는 정보]")
      || trimmed.starts_with("[증거 참조표]")
      || trimmed.starts_with("[전략 메모]")
      || trimmed.starts_with("[직전 대화]")
      || trimmed.starts_with("[이번 요청]")
      || trimmed.starts_with("[응답 조건]")
      || trimmed.starts_with("[질문]")
      || trimmed.starts_with("[핵심 근거]")
      || trimmed.starts_with("[관련 법령]")
      || trimmed.starts_with("[관련 법령/참고 기준]")
      || trimmed.starts_with("[기록 작성 템플릿]")
      || trimmed.starts_with("[핵심 원칙]")
      || trimmed.starts_with("[품질 점검표]")
      || trimmed.starts_with("[금지 사항]")
      || trimmed.starts_with("[섹션별 가이드]")
      || trimmed.starts_with("[HyperCLOVA-X 기록 골격]")
      || trimmed.starts_with("[HyperCLOVA-X 초안]")
      || trimmed.starts_with("[Roosy-X 초안]")
      || trimmed.starts_with("[합성 지침]")
      || trimmed.starts_with("[추가 정보]")
      || trimmed.starts_with("현재 목표:")
      || trimmed.starts_with("전략 프리셋:")
      || trimmed.starts_with("AI에게 반영할 메모:")
      || trimmed.starts_with("기준 사건:")
      || trimmed.starts_with("로컬 브리핑 지표:")
      || trimmed.starts_with("추천 톤:")
      || trimmed.starts_with("추천 행동:")
      || trimmed.starts_with("- AI 언어모델의 이름은")
      || trimmed.starts_with("- 반드시 한국어로만")
      || trimmed.starts_with("- 영어, 중국어, 일본어")
      || trimmed.starts_with("- 역할:")
      || trimmed.starts_with("- 반드시 제공된 사건 맥락과 증거 참조표만")
      || trimmed.starts_with("- 사건에 없는 사실을 추가하지 말고")
      || trimmed.starts_with("- 핵심 판단이나 제안마다 가능하면")
      || trimmed.starts_with("- 답변은 교사가 바로")
      || trimmed.starts_with("- 과도한 법률 단정은")
      || trimmed.starts_with("- 응답은 가능하면 1) 상황판단")
      || trimmed.starts_with("- HyperCLOVA-X 초안의")
      || trimmed.starts_with("- Roosy-X 초안의")
      || trimmed.starts_with("- 둘을 비교하거나")
      || trimmed.starts_with("- 사용자에게 바로 전달할")
      || trimmed.starts_with("- 대답 속에 [합성 지침]")
      || trimmed.starts_with("- 너무 짧게 줄이지 말고")
      || trimmed.starts_with("- HyperCLOVA-X 기록 골격의")
      || trimmed.starts_with("- 각 섹션을 더 구체적이고")
      || trimmed.starts_with("- 문장감은 자연스럽게")
      || trimmed.starts_with("- [상황 요약], [배경 흐름]")
      || trimmed.starts_with("- 동일한 문장을 반복하거나")
      || trimmed.starts_with("- 지정된 섹션 제목만")
    {
      continue;
    }
    let normalized = trimmed.replace('\u{fffd}', "").trim().to_string();
    if normalized.is_empty() {
      continue;
    }
    filtered.push(normalized);
  }
  let out = filtered.join("\n");
  let out = strategy_strip_prompt_echo(out.trim());
  let out = out.trim();
  strategy_trim(out, 4200)
}

fn strategy_answer_has_evidence_ref(answer: &str) -> bool {
  answer.contains("[E1]") || answer.contains("[E2]") || answer.contains("[E3]") || answer.contains("[E")
}

fn finalize_strategy_answer(answer: &str, evidence_packet: &StrategyEvidencePacket, user_message: &str) -> String {
  let mut out = answer.trim().to_string();
  if out.is_empty() {
    return out;
  }

  if !strategy_answer_has_evidence_ref(&out) && !evidence_packet.evidence_records.is_empty() {
    let lines = evidence_packet
      .evidence_records
      .iter()
      .take(2)
      .map(|item| {
        format!(
          "[{}] {} / {} / {}",
          item.ref_id,
          strategy_trim(item.ts.trim(), 22),
          strategy_trim(item.actor.trim(), 22),
          strategy_trim(item.summary.trim(), 54)
        )
      })
      .collect::<Vec<_>>()
      .join(", ");
    out.push_str("\n\n참고로 지금 판단의 중심 근거는 ");
    out.push_str(&lines);
    out.push_str(" 정도예요.");
  }

  if !out.contains("[L")
    && !evidence_packet.legal_references.is_empty()
    && strategy_question_needs_legal_refs(user_message)
  {
    let refs = evidence_packet
      .legal_references
      .iter()
      .take(2)
      .map(|item| {
        let law_label = if item.short_name.trim().is_empty() {
          item.law_name.trim().to_string()
        } else {
          item.short_name.trim().to_string()
        };
        let article = if item.article_ref.trim().is_empty() {
          item.article_title.trim().to_string()
        } else {
          format!("{} {}", item.article_ref.trim(), item.article_title.trim()).trim().to_string()
        };
        format!("[{}] {} {}", item.ref_id, strategy_trim(&law_label, 18), strategy_trim(&article, 28))
      })
      .collect::<Vec<_>>()
      .join(", ");
    if !refs.trim().is_empty() {
      out.push_str("\n\n관련 법령으로는 ");
      out.push_str(&refs);
      out.push_str(" 정도가 함께 연결돼요.");
    }
  }

  let normalized = out.replace(' ', "");
  if !evidence_packet.gaps.is_empty()
    && !normalized.contains("확인필요")
    && !normalized.contains("비어있는정보")
    && !normalized.contains("추가필요")
  {
    let lines = evidence_packet
      .gaps
      .iter()
      .take(2)
      .map(|item| item.to_string())
      .collect::<Vec<_>>()
      .join(", ");
    out.push_str("\n\n추가로 ");
    out.push_str(&lines);
    out.push_str(" 부분은 아직 확실하지 않아 확인이 더 필요해요.");
  }

  strategy_trim(out.trim(), 5600)
}

#[derive(Debug, Default)]
struct LlamaServerTimings {
  prompt_ms: Option<f64>,
  predicted_ms: Option<f64>,
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

fn parse_llama_server_timings(root: &serde_json::Value) -> LlamaServerTimings {
  let timings = root.get("timings");
  let prompt_ms = timings
    .and_then(|value| value.get("prompt_ms"))
    .and_then(|value| value.as_f64());
  let predicted_ms = timings
    .and_then(|value| value.get("predicted_ms"))
    .and_then(|value| value.as_f64());
  LlamaServerTimings {
    prompt_ms,
    predicted_ms,
  }
}

fn extract_llama_server_answer(root: &serde_json::Value) -> Option<String> {
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

fn extract_llama_server_error(root: &serde_json::Value) -> Option<String> {
  if let Some(value) = value_string(root.get("error").and_then(|error| error.get("message"))) {
    return Some(value);
  }
  if let Some(serde_json::Value::String(text)) = root.get("error") {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
      return Some(trimmed.to_string());
    }
  }
  if let Some(value) = value_string(root.get("detail")) {
    return Some(value);
  }
  if let Some(value) = value_string(root.get("message")) {
    return Some(value);
  }
  None
}

fn llama_server_endpoint_for_model(config: &LlamaServerConfig, model_id: &str) -> (String, Option<u32>) {
  if model_id == STRATEGY_MODEL_ROOSY_ID {
    (config.roosy_url.clone(), config.roosy_slot)
  } else {
    (config.hyperclova_url.clone(), config.hyperclova_slot)
  }
}

fn effective_llama_server_endpoint_for_model(config: &LlamaServerConfig, model_id: &str) -> (String, Option<u32>) {
  if model_id == STRATEGY_MODEL_ROOSY_ID {
    let endpoint = if strategy_endpoint_is_managed_candidate(
      &config.roosy_url,
      MANAGED_LLAMA_SERVER_ROOSY_URL,
      LEGACY_LLAMA_SERVER_ROOSY_URL,
    ) {
      strategy_allocate_managed_endpoint(model_id)
    } else {
      normalize_loopback_llama_server_endpoint(
        &config.roosy_url,
        MANAGED_LLAMA_SERVER_ROOSY_URL,
        LEGACY_LLAMA_SERVER_ROOSY_URL,
      )
    };
    (
      endpoint,
      config.roosy_slot,
    )
  } else {
    let endpoint = if strategy_endpoint_is_managed_candidate(
      &config.hyperclova_url,
      MANAGED_LLAMA_SERVER_HYPERCLOVA_URL,
      LEGACY_LLAMA_SERVER_HYPERCLOVA_URL,
    ) {
      strategy_allocate_managed_endpoint(model_id)
    } else {
      normalize_loopback_llama_server_endpoint(
        &config.hyperclova_url,
        MANAGED_LLAMA_SERVER_HYPERCLOVA_URL,
        LEGACY_LLAMA_SERVER_HYPERCLOVA_URL,
      )
    };
    (
      endpoint,
      config.hyperclova_slot,
    )
  }
}

fn llama_server_slot_for_stage(base_slot: Option<u32>, stage: HybridStage) -> Option<u32> {
  if matches!(base_slot, Some(0)) {
    return Some(0);
  }
  let offset = match stage {
    HybridStage::GeneralMain => 0,
    HybridStage::GeneralDraft => 1,
    HybridStage::GeneralSynthesis => 2,
    HybridStage::RecordMain => 10,
    HybridStage::RecordFill => 11,
    HybridStage::RecordSynthesis => 12,
    HybridStage::RecordReview => 13,
    HybridStage::FastRoosy => 20,
    HybridStage::RecordRecovery => 21,
  };
  base_slot.map(|value| value.saturating_add(offset))
}

fn execute_strategy_model_via_llama_server(
  app: Option<&AppHandle>,
  llama_server: &LlamaServerConfig,
  model_id: &str,
  drace_stage: HybridStage,
  stage: &str,
  model_label: &str,
  model_path: &Path,
  system_prompt: &str,
  user_prompt: &str,
  runtime: StrategyRuntimeConfig,
  max_tokens: u32,
  prompt_chars: usize,
  prompt_tokens: usize,
  static_prefix_id: &str,
  static_prefix_hash: u64,
  static_prefix_tokens: usize,
  stage_cache_pref: &StrategyStageCachePerf,
  started: Instant,
  tuning: StrategyGenerationTuning,
  assistant_prefix: Option<&str>,
) -> Result<StrategyModelExecution, String> {
  let (endpoint, base_slot) = effective_llama_server_endpoint_for_model(llama_server, model_id);
  let slot = llama_server_slot_for_stage(base_slot, drace_stage);
  let runner_label = format!("llama-server@{}", endpoint);
  let cache_prompt_enabled = stage_cache_pref.prefix_kv_applied && llama_server.cache_prompt;
  let prefix_was_warm = if cache_prompt_enabled {
    DraceCacheManager::global().is_prefix_warm(model_id, static_prefix_id, slot, Some(static_prefix_hash))
  } else {
    false
  };
  if cache_prompt_enabled {
    emit_strategy_progress(
      app,
      stage,
      if prefix_was_warm {
        format!(
          "DRaCE PrefixKV warm hit · hash {} · reused {} tokens",
          strategy_short_hash_u64(static_prefix_hash),
          static_prefix_tokens
        )
      } else {
        format!(
          "DRaCE PrefixKV cold start · hash {} · warmup {} tokens",
          strategy_short_hash_u64(static_prefix_hash),
          static_prefix_tokens
        )
      },
    );
  }
  let prompt = format!("{}\n\n{}", system_prompt.trim(), user_prompt.trim());
  let assistant_prefix_text = assistant_prefix.unwrap_or("").to_string();
  let session_backend = LlamaServerSessionBackend;
  let build_session_options = |use_cache_prompt: bool| GenerationSessionOptions {
    model_id: model_id.to_string(),
    endpoint: endpoint.clone(),
    model_path: Some(model_path.display().to_string()),
    slot: if use_cache_prompt { slot } else { None },
    cache_prompt: use_cache_prompt,
    assistant_prefix: if assistant_prefix_text.is_empty() {
      None
    } else {
      Some(assistant_prefix_text.clone())
    },
    n_ctx: Some(runtime.n_ctx),
    threads: Some(runtime.threads),
    max_tokens,
    temperature: tuning.temperature,
    top_p: tuning.top_p,
    repeat_penalty: tuning.repeat_penalty,
    request_timeout_ms: llama_server.request_timeout_ms,
  };
  let run_request = |use_cache_prompt: bool| -> Result<(String, LlamaServerTimings, u128), String> {
    let session = session_backend.open_session(&prompt, build_session_options(use_cache_prompt))?;
    let step = session_backend.generate_next(&session)?;
    let _ = session_backend.close_session(&session);
    let answer = cleanup_strategy_output(
      extract_llama_server_answer(&step.raw_response)
        .unwrap_or_default()
        .as_str(),
    );
    if answer.trim().is_empty() {
      let server_reason = extract_llama_server_error(&step.raw_response)
        .unwrap_or_else(|| "llama-server가 비어 있는 응답을 반환했어요.".to_string());
      return Err(format!("llama-server가 비어 있는 응답을 반환했어요: {}", server_reason));
    }
    let final_answer = if assistant_prefix_text.is_empty() {
      answer
    } else {
      format!("{}{}", assistant_prefix_text, answer)
    };
    Ok((final_answer, parse_llama_server_timings(&step.raw_response), step.response_started_ms))
  };

  emit_strategy_progress(app, stage, format!("{}로 resident backend 응답을 생성하고 있어요.", model_label));
  let mut request_used_cache = cache_prompt_enabled;
  let (answer, timings, response_started_ms) = match run_request(cache_prompt_enabled) {
    Ok(value) => value,
    Err(err) if strategy_error_indicates_ctx_exhausted(&err) && cache_prompt_enabled => {
      emit_strategy_progress(
        app,
        stage,
        "resident slot이 포화되어 cache_prompt 없이 한 번 더 시도할게요.".to_string(),
      );
      request_used_cache = false;
      match run_request(false) {
        Ok(value) => value,
        Err(uncached_err) if strategy_error_indicates_ctx_exhausted(&uncached_err) => {
          emit_strategy_progress(
            app,
            stage,
            "resident backend 컨텍스트가 부족해 서버를 다시 준비한 뒤 한 번 더 시도할게요.".to_string(),
          );
          reset_strategy_llama_server_process(model_id);
          ensure_strategy_llama_server_process(
            app,
            &endpoint,
            model_id,
            runtime.n_ctx.max(4096),
            llama_server.startup_timeout_ms,
          )
          .map_err(|restart_err| format!("{err}; uncached_retry_failed: {uncached_err}; restart_failed: {restart_err}"))?;
          run_request(false)?
        }
        Err(uncached_err) => return Err(format!("{err}; uncached_retry_failed: {uncached_err}")),
      }
    }
    Err(err) if strategy_error_indicates_ctx_exhausted(&err) => {
      emit_strategy_progress(
        app,
        stage,
        "resident backend 컨텍스트가 부족해 서버를 다시 준비한 뒤 한 번 더 시도할게요.".to_string(),
      );
      request_used_cache = false;
      reset_strategy_llama_server_process(model_id);
      ensure_strategy_llama_server_process(
        app,
        &endpoint,
        model_id,
        runtime.n_ctx.max(4096),
        llama_server.startup_timeout_ms,
      )
      .map_err(|restart_err| format!("{err}; restart_failed: {restart_err}"))?;
      run_request(false)?
    }
    Err(err) => return Err(err),
  };

  let prompt_eval_ms = timings.prompt_ms.map(|value: f64| value.max(0.0) as u128);
  let decode_ms = timings.predicted_ms.map(|value: f64| value.max(0.0) as u128);
  let e2e_ms = started.elapsed().as_millis().max(1);
  let ttft_ms = Some(prompt_eval_ms.unwrap_or(response_started_ms).max(1));
  let output_tokens = strategy_approx_output_tokens(&answer);
  let decode_tps = if let Some(ms) = decode_ms {
    let seconds = (ms.max(1) as f32) / 1000.0;
    ((output_tokens as f32) / seconds * 100.0).round() / 100.0
  } else {
    0.0
  };
  let e2e_tps = {
    let seconds = (e2e_ms as f32) / 1000.0;
    ((output_tokens as f32) / seconds * 100.0).round() / 100.0
  };
  let mut stage_cache = stage_cache_pref.clone();
  let synthetic_applied = stage_cache.synthetic_cache_applied;
  let can_apply_prefix_kv = stage_cache.prefix_kv_applied && request_used_cache;
  let can_apply_cache = synthetic_applied || can_apply_prefix_kv;
  stage_cache.cache_loaded =
    stage_cache.cache_requested && (llama_server.cache_prompt || stage_cache.synthetic_cache_supported);
  stage_cache.cache_supported = true;
  stage_cache.prefix_kv_supported = true;
  stage_cache.prefix_kv_applied = can_apply_prefix_kv;
  stage_cache.cache_applied = can_apply_cache;
  stage_cache.cache_warm = can_apply_prefix_kv && prefix_was_warm;
  stage_cache.cache_mode_applied = if synthetic_applied {
    "FullDRACE".to_string()
  } else if can_apply_prefix_kv {
    stage_cache_pref.cache_mode_applied.clone()
  } else {
    "Off".to_string()
  };
  stage_cache.prefix_total_tokens = if can_apply_prefix_kv { static_prefix_tokens } else { 0 };
  stage_cache.prefix_reused_tokens = if can_apply_prefix_kv && prefix_was_warm {
    static_prefix_tokens
  } else {
    0
  };
  stage_cache.prefix_reuse_ratio = if stage_cache.prefix_total_tokens > 0 {
    stage_cache.prefix_reused_tokens as f32 / stage_cache.prefix_total_tokens as f32
  } else {
    0.0
  };
  stage_cache.bypass_reason = if can_apply_cache {
    String::new()
  } else if !stage_cache_pref.bypass_reason.trim().is_empty() {
    stage_cache_pref.bypass_reason.clone()
  } else if cache_prompt_enabled && !request_used_cache {
    "prefix_kv_slot_saturated".to_string()
  } else if !llama_server.cache_prompt {
    "cache_prompt_disabled".to_string()
  } else {
    "prefix_kv_not_planned".to_string()
  };
  if can_apply_prefix_kv && !prefix_was_warm {
    DraceCacheManager::global().mark_prefix_warm(
      model_id,
      static_prefix_id,
      slot,
      Some(static_prefix_hash),
      static_prefix_tokens,
    );
    strategy_persist_drace_persistent_state(app);
  }

  Ok(StrategyModelExecution {
    answer,
    model_path: model_path.display().to_string(),
    runner: runner_label.clone(),
    prompt_chars,
    metrics: StrategyStagePerf {
      stage_name: stage.to_string(),
      model_id: model_id.to_string(),
      backend_kind: "llama-server".to_string(),
      runner_path: runner_label,
      model_path: model_path.display().to_string(),
      threads: runtime.threads,
      threads_batch: runtime.threads,
      n_ctx: runtime.n_ctx,
      max_tokens,
      temperature: tuning.temperature,
      top_p: tuning.top_p,
      repeat_penalty: tuning.repeat_penalty,
      e2e_ms,
      ttft_ms,
      prompt_tokens,
      output_tokens,
      prompt_eval_ms,
      decode_ms,
      e2e_tps,
      decode_tps,
      peak_memory_mb: strategy_estimate_model_footprint_mb(model_path.to_string_lossy().as_ref()),
      process_spawn_ms: 0,
      prompt_file_write_ms: 0,
      stdout_read_ms: response_started_ms,
      postprocess_ms: 0,
      cache: stage_cache,
    },
  })
}

fn execute_strategy_model_via_native_backend(
  app: Option<&AppHandle>,
  model_id: &str,
  stage: &str,
  model_label: &str,
  model_path: &Path,
  system_prompt: &str,
  user_prompt: &str,
  runtime: StrategyRuntimeConfig,
  max_tokens: u32,
  prompt_chars: usize,
  prompt_tokens: usize,
  stage_cache_pref: &StrategyStageCachePerf,
  started: Instant,
  tuning: StrategyGenerationTuning,
  assistant_prefix: Option<&str>,
) -> Result<StrategyModelExecution, String> {
  emit_strategy_progress(
    app,
    stage,
    format!("{model_label}로 native backend 응답을 생성하고 있어요. 현재는 deterministic top-1 생성으로 동작해요."),
  );
  let prompt = format!("{}\n\n{}", system_prompt.trim(), user_prompt.trim());
  let session_backend = NativeSessionBackend;
  let session = session_backend.open_session(
    &prompt,
    GenerationSessionOptions {
      model_id: model_id.to_string(),
      endpoint: String::new(),
      model_path: Some(model_path.display().to_string()),
      slot: None,
      cache_prompt: false,
      assistant_prefix: assistant_prefix.map(|value| value.to_string()),
      n_ctx: Some(runtime.n_ctx),
      threads: Some(runtime.threads),
      max_tokens,
      temperature: 0.0,
      top_p: tuning.top_p,
      repeat_penalty: tuning.repeat_penalty,
      request_timeout_ms: 0,
    },
  )?;
  let step = session_backend.generate_next(&session)?;
  let _ = session_backend.close_session(&session);
  let answer = cleanup_strategy_output(
    extract_llama_server_answer(&step.raw_response)
      .unwrap_or_default()
      .as_str(),
  );
  if answer.trim().is_empty() {
    return Err("native backend가 비어 있는 응답을 반환했어요.".to_string());
  }
  let timings = parse_llama_server_timings(&step.raw_response);
  let prompt_eval_ms = timings.prompt_ms.map(|value: f64| value.max(0.0) as u128);
  let decode_ms = timings.predicted_ms.map(|value: f64| value.max(0.0) as u128);
  let e2e_ms = started.elapsed().as_millis().max(1);
  let ttft_ms = Some(step.response_started_ms.max(1));
  let output_tokens = step
    .raw_response
    .get("native")
    .and_then(|value| value.get("output_tokens"))
    .and_then(|value| value.as_u64())
    .map(|value| value as usize)
    .unwrap_or_else(|| strategy_approx_output_tokens(&answer));
  let decode_tps = if let Some(ms) = decode_ms {
    let seconds = (ms.max(1) as f32) / 1000.0;
    ((output_tokens as f32) / seconds * 100.0).round() / 100.0
  } else {
    0.0
  };
  let e2e_tps = {
    let seconds = (e2e_ms as f32) / 1000.0;
    ((output_tokens as f32) / seconds * 100.0).round() / 100.0
  };
  let mut stage_cache = stage_cache_pref.clone();
  let synthetic_applied = stage_cache.synthetic_cache_applied;
  stage_cache.cache_supported = stage_cache.token_verification_supported || stage_cache.token_cache_supported;
  stage_cache.cache_loaded = stage_cache.synthetic_cache_supported;
  stage_cache.cache_applied = synthetic_applied;
  stage_cache.prefix_kv_supported = false;
  stage_cache.prefix_kv_applied = false;
  stage_cache.prefix_reused_tokens = 0;
  stage_cache.prefix_total_tokens = 0;
  stage_cache.prefix_reuse_ratio = 0.0;
  if synthetic_applied {
    stage_cache.cache_mode_applied = "FullDRACE".to_string();
    stage_cache.bypass_reason.clear();
  } else {
    stage_cache.cache_mode_applied = "Off".to_string();
  }
  Ok(StrategyModelExecution {
    answer,
    model_path: model_path.display().to_string(),
    runner: "libllama(native)".to_string(),
    prompt_chars,
    metrics: StrategyStagePerf {
      stage_name: stage.to_string(),
      model_id: model_id.to_string(),
      backend_kind: "native".to_string(),
      runner_path: "libllama(native)".to_string(),
      model_path: model_path.display().to_string(),
      threads: runtime.threads,
      threads_batch: runtime.threads,
      n_ctx: runtime.n_ctx,
      max_tokens,
      temperature: 0.0,
      top_p: tuning.top_p,
      repeat_penalty: tuning.repeat_penalty,
      e2e_ms,
      ttft_ms,
      prompt_tokens,
      output_tokens,
      prompt_eval_ms,
      decode_ms,
      e2e_tps,
      decode_tps,
      peak_memory_mb: strategy_estimate_model_footprint_mb(model_path.to_string_lossy().as_ref()),
      process_spawn_ms: 0,
      prompt_file_write_ms: 0,
      stdout_read_ms: step.response_started_ms,
      postprocess_ms: 0,
      cache: stage_cache,
    },
  })
}

fn execute_strategy_model(
  app: Option<&AppHandle>,
  backend_runtime: &StrategyBackendRuntime,
  requested_model_id: &str,
  drace_stage: HybridStage,
  system_prompt_raw: &str,
  user_prompt_raw: &str,
  evidence_packet: &StrategyEvidencePacket,
  n_ctx: u32,
  max_tokens: u32,
  threads: u32,
  stage_label: &str,
  synthetic_cache_enabled: bool,
) -> Result<StrategyModelExecution, String> {
  execute_strategy_model_with_prefix(
    app,
    backend_runtime,
    requested_model_id,
    drace_stage,
    system_prompt_raw,
    user_prompt_raw,
    evidence_packet,
    n_ctx,
    max_tokens,
    threads,
    stage_label,
    synthetic_cache_enabled,
    None,
  )
}

fn execute_strategy_model_with_prefix(
  app: Option<&AppHandle>,
  backend_runtime: &StrategyBackendRuntime,
  requested_model_id: &str,
  drace_stage: HybridStage,
  system_prompt_raw: &str,
  user_prompt_raw: &str,
  evidence_packet: &StrategyEvidencePacket,
  n_ctx: u32,
  max_tokens: u32,
  threads: u32,
  stage_label: &str,
  synthetic_cache_enabled: bool,
  assistant_prefix: Option<&str>,
) -> Result<StrategyModelExecution, String> {
  match execute_strategy_model_once(
    app,
    backend_runtime,
    requested_model_id,
    drace_stage,
    system_prompt_raw,
    user_prompt_raw,
    evidence_packet,
    n_ctx,
    max_tokens,
    threads,
    stage_label,
    synthetic_cache_enabled,
    assistant_prefix,
  ) {
    Ok(result) => Ok(result),
    Err(err) if synthetic_cache_enabled && assistant_prefix.is_none() => {
      emit_strategy_progress(
        app,
        stage_label,
        format!(
          "DRaCE 캐시 경로가 잠시 흔들려 캐싱 없이 한 번 더 시도할게요. {}",
          strategy_trim(&err, 220)
        ),
      );
      execute_strategy_model_once(
        app,
        backend_runtime,
        requested_model_id,
        drace_stage,
        system_prompt_raw,
        user_prompt_raw,
        evidence_packet,
        n_ctx,
        max_tokens,
        threads,
        stage_label,
        false,
        assistant_prefix,
      )
    }
    Err(err) => Err(err),
  }
}

fn execute_strategy_model_once(
  app: Option<&AppHandle>,
  backend_runtime: &StrategyBackendRuntime,
  requested_model_id: &str,
  drace_stage: HybridStage,
  system_prompt_raw: &str,
  user_prompt_raw: &str,
  evidence_packet: &StrategyEvidencePacket,
  n_ctx: u32,
  max_tokens: u32,
  threads: u32,
  stage_label: &str,
  synthetic_cache_enabled: bool,
  assistant_prefix: Option<&str>,
) -> Result<StrategyModelExecution, String> {
  let model_id = normalize_strategy_model_id(Some(requested_model_id));
  if model_id == STRATEGY_MODEL_HYBRID_ID {
    return Err("하이브리드 모델은 직접 실행할 수 없어요.".to_string());
  }
  let runtime = strategy_runtime_config(Some(n_ctx), Some(threads));
  let mut tuning = strategy_generation_tuning();
  if matches!(drace_stage, HybridStage::RecordReview) {
    tuning.temperature = 0.0;
    tuning.top_p = 1.0;
    tuning.repeat_penalty = 1.05;
  }
  let cache_requested = synthetic_cache_enabled;
  let capabilities = backend_runtime.capabilities.clone();
  let started = Instant::now();
  let stage = stage_label.trim();
  let model_label = strategy_model_label_for_id(model_id);
  let mut static_prefix_log: Option<(String, u64, Option<u64>, usize, String, String)> = None;

  let model_path = resolve_strategy_model_path(app, model_id)?;
  let runner = resolve_strategy_runner_path(app)?;
  let prompt_build_started = Instant::now();
  let (system_prompt, user_prompt, mut stage_plan, static_prefix_id, static_prefix_hash, static_prefix_tokens) = if matches!(capabilities.backend_kind, BackendKind::CliSidecar) {
    (
      strategy_sanitize_text(system_prompt_raw.trim()),
      strategy_fit_prompt_to_budget(&strategy_sanitize_text(user_prompt_raw.trim()), runtime.n_ctx, max_tokens),
      strategy_cli_bypass_plan_with_reason(
        cache_requested,
        if cache_requested && !backend_runtime.llama_server_unavailable_reason.trim().is_empty() {
          backend_runtime.llama_server_unavailable_reason.clone()
        } else if cache_requested {
          "unsupported_backend_cli".to_string()
        } else {
          "cache disabled".to_string()
        },
      ),
      String::new(),
      0_u64,
      0usize,
    )
  } else {
    let prepared_prompt = strategy_prepare_stage_prompt(
      model_id,
      drace_stage,
      synthetic_cache_enabled,
      system_prompt_raw.trim(),
      user_prompt_raw.trim(),
      max_tokens as usize,
    );
    static_prefix_log = Some((
      prepared_prompt.static_prefix.id.clone(),
      prepared_prompt.static_prefix.content_hash,
      prepared_prompt.static_prefix_previous_hash,
      prepared_prompt.static_prefix.estimated_tokens,
      prepared_prompt.system_segment_order.join(" > "),
      prepared_prompt.user_segment_order.join(" > "),
    ));
    (
      render_prompt_segments(&prepared_prompt.system_segments),
      strategy_fit_prompt_to_budget(
        &render_prompt_segments(&prepared_prompt.user_segments),
        runtime.n_ctx,
        max_tokens,
      ),
      prepared_prompt.plan,
      prepared_prompt.static_prefix.id,
      prepared_prompt.static_prefix.content_hash,
      prepared_prompt.static_prefix.estimated_tokens,
    )
  };
  let prompt_build_ms = prompt_build_started.elapsed().as_millis();
  let prompt_chars = system_prompt.chars().count() + user_prompt.chars().count();
  let prompt_tokens = strategy_approx_output_tokens(&(system_prompt.clone() + "\n" + &user_prompt));
  let runner_name = runner.file_name().and_then(|x| x.to_str()).unwrap_or("llama-sidecar");
  let model_name = model_path.file_name().and_then(|x| x.to_str()).unwrap_or(strategy_model_filename_for_id(model_id));
  let mut stage_cache_pref = build_strategy_stage_cache_perf(&capabilities, &stage_plan, prompt_tokens, 0);
  let mut effective_assistant_prefix = assistant_prefix.map(|value| value.to_string());
  let mut stage_overhead = StrategyOverheadMetrics {
    prompt_build_ms,
    cache_capability_ms: 0,
    cache_plan_ms: 0,
    cache_lookup_ms: stage_plan.token_cache_lookup_ms,
    ..StrategyOverheadMetrics::default()
  };

  emit_strategy_progress(
    app,
    stage,
    format!("{} 단계에서 {}({})를 준비했어요.", if stage.is_empty() { "실행" } else { stage }, model_label, model_name),
  );
  emit_strategy_progress(
    app,
    stage,
    format!(
      "{} · 실행기 {} · 프롬프트 {}자 · 컨텍스트 {} · 최대 토큰 {} · 스레드 {} · 장치 {}",
      model_label,
      runner_name,
      prompt_chars,
      runtime.n_ctx,
      max_tokens,
      runtime.threads,
      if runtime.n_gpu_layers > 0 { "metal" } else { "cpu" }
    ),
  );
  if let Some((prefix_id, prefix_hash, previous_hash, estimated_tokens, system_order, user_order)) = static_prefix_log.as_ref() {
    emit_strategy_progress(
      app,
      stage,
      format!(
        "DRaCE static prefix {} · hash {} · {} tokens · system {} · user {}",
        prefix_id,
        strategy_short_hash_u64(*prefix_hash),
        estimated_tokens,
        system_order,
        user_order
      ),
    );
    if let Some(previous_hash) = previous_hash {
      emit_strategy_progress(
        app,
        stage,
        format!(
          "DRaCE static prefix가 바뀌었어요. 이전 {} → 현재 {}",
          strategy_short_hash_u64(*previous_hash),
          strategy_short_hash_u64(*prefix_hash)
        ),
      );
    }
  }

  if cache_requested && !stage_plan.cache_applied {
    emit_strategy_progress(
      app,
      stage,
      format!(
        "이번 단계의 DRaCE 캐시는 자동 우회할게요. {}",
        if stage_plan.bypass_reason.trim().is_empty() {
          "현재 backend에서 이득이 없다고 판단했어요.".to_string()
        } else {
          stage_plan.bypass_reason.clone()
        }
      ),
    );
  }

  if synthetic_cache_enabled
    && assistant_prefix.is_none()
    && stage_plan.use_synthetic_token_cache
    && !matches!(drace_stage, HybridStage::RecordReview)
  {
    let draft_provider = if stage_plan.draft_provider.trim().is_empty() {
      "template"
    } else {
      stage_plan.draft_provider.as_str()
    };
    let draft_proposal = strategy_build_draft_proposal(
      drace_stage,
      draft_provider,
      model_id,
      false,
      None,
      stage_plan.max_candidate_tokens.max(1),
    );
    let verified_draft = strategy_verify_draft_prefix_via_llama_server(
      app,
      backend_runtime,
      model_id,
      drace_stage,
      &system_prompt,
      &user_prompt,
      runtime.n_ctx,
      runtime.threads,
      max_tokens,
      &draft_proposal,
    );
    stage_cache_pref.synthetic_cache_requested = stage_plan.synthetic_cache_requested;
    stage_cache_pref.synthetic_cache_supported = stage_plan.synthetic_cache_supported;
    stage_cache_pref.synthetic_cache_applied = verified_draft.synthetic_applied;
    stage_cache_pref.token_cache_applied = verified_draft.synthetic_applied;
    stage_cache_pref.token_cache_loaded = stage_cache_pref.token_cache_supported;
    stage_cache_pref.draft_provider = draft_provider.to_string();
    stage_cache_pref.proposed_tokens = verified_draft.proposed_tokens;
    stage_cache_pref.accepted_tokens = verified_draft.accepted_tokens;
    stage_cache_pref.rejected_tokens = verified_draft.rejected_tokens;
    stage_cache_pref.verify_batches = verified_draft.verification_batches;
    stage_cache_pref.rejected_batches = if verified_draft.rejected_tokens > 0 { 1 } else { 0 };
    stage_cache_pref.acceptance_ratio = if verified_draft.proposed_tokens > 0 {
      verified_draft.accepted_tokens as f32 / verified_draft.proposed_tokens as f32
    } else {
      0.0
    };
    stage_cache_pref.avg_proposed_batch_size = if verified_draft.verification_batches > 0 {
      verified_draft.proposed_tokens as f32 / verified_draft.verification_batches as f32
    } else {
      0.0
    };
    stage_cache_pref.avg_accepted_batch_size = if verified_draft.verification_batches > 0 {
      verified_draft.accepted_tokens as f32 / verified_draft.verification_batches as f32
    } else {
      0.0
    };
    stage_cache_pref.accepted_tokens_per_verify = verified_draft.accepted_tokens_per_verify;
    if verified_draft.synthetic_applied && !verified_draft.accepted_text.trim().is_empty() {
      effective_assistant_prefix = Some(verified_draft.accepted_text.clone());
      stage_cache_pref.cache_applied = true;
      stage_cache_pref.cache_mode_applied = "FullDRACE".to_string();
      stage_cache_pref.bypass_reason.clear();
    } else if !verified_draft.bypass_reason.trim().is_empty() {
      stage_cache_pref.bypass_reason = verified_draft.bypass_reason.clone();
    }
  }

  if matches!(capabilities.backend_kind, BackendKind::Native) {
    match execute_strategy_model_via_native_backend(
      app,
      model_id,
      stage,
      model_label,
      &model_path,
      &system_prompt,
      &user_prompt,
      runtime,
      max_tokens,
      prompt_chars,
      prompt_tokens,
      &stage_cache_pref,
      started,
      tuning,
      effective_assistant_prefix.as_deref(),
    ) {
      Ok(result) => return Ok(result),
      Err(err) => {
        emit_strategy_progress(
          app,
          stage,
          format!(
            "native backend가 응답하지 않아 CLI baseline으로 안전하게 되돌릴게요. {}",
            strategy_trim(&err, 220)
          ),
        );
        let cli_capabilities = BackendCapabilities {
          backend_kind: BackendKind::CliSidecar,
          supports_resident: false,
          supports_prompt_token_cache: false,
          supports_prompt_cache: false,
          supports_prefix_kv_cache: false,
          supports_token_verification: false,
          supports_batch_verification: false,
          supports_speculative_decode: false,
          supports_synthetic_token_cache: false,
          supports_mmap_cache_pack: false,
          supports_resident_model: false,
        };
        stage_plan = strategy_cli_bypass_plan_with_reason(cache_requested, format!("native_backend_request_failed: {}", strategy_trim(&err, 240)));
        stage_cache_pref = build_strategy_stage_cache_perf(&cli_capabilities, &stage_plan, prompt_tokens, 0);
      }
    }
  }

  if matches!(capabilities.backend_kind, BackendKind::LlamaServer) {
    if !backend_runtime.llama_server_available {
      let reason = if backend_runtime.llama_server_unavailable_reason.trim().is_empty() {
        "llama_server_unavailable".to_string()
      } else {
        backend_runtime.llama_server_unavailable_reason.clone()
      };
      emit_strategy_progress(
        app,
        stage,
        format!(
          "resident backend healthcheck가 실패해서 CLI baseline으로 되돌릴게요. {}",
          strategy_trim(&reason, 220)
        ),
      );
      let cli_capabilities = BackendCapabilities {
        backend_kind: BackendKind::CliSidecar,
        supports_resident: false,
        supports_prompt_token_cache: false,
        supports_prompt_cache: false,
        supports_prefix_kv_cache: false,
        supports_token_verification: false,
        supports_batch_verification: false,
        supports_speculative_decode: false,
        supports_synthetic_token_cache: false,
        supports_mmap_cache_pack: false,
        supports_resident_model: false,
      };
      stage_plan = strategy_cli_bypass_plan_with_reason(cache_requested, reason);
      stage_cache_pref = build_strategy_stage_cache_perf(&cli_capabilities, &stage_plan, prompt_tokens, 0);
    } else if let Some(llama_server) = backend_runtime.llama_server.as_ref() {
      match execute_strategy_model_via_llama_server(
        app,
        llama_server,
        model_id,
        drace_stage,
        stage,
        model_label,
        &model_path,
        &system_prompt,
        &user_prompt,
        runtime,
        max_tokens,
        prompt_chars,
        prompt_tokens,
        &static_prefix_id,
        static_prefix_hash,
        static_prefix_tokens,
          &stage_cache_pref,
          started,
          tuning,
          effective_assistant_prefix.as_deref(),
        ) {
        Ok(result) => return Ok(result),
        Err(err) => {
          emit_strategy_progress(
            app,
            stage,
            format!(
              "resident backend가 응답하지 않아 CLI baseline으로 안전하게 되돌릴게요. {}",
              strategy_trim(&err, 220)
            ),
          );
          let cli_capabilities = BackendCapabilities {
            backend_kind: BackendKind::CliSidecar,
            supports_resident: false,
            supports_prompt_token_cache: false,
            supports_prompt_cache: false,
            supports_prefix_kv_cache: false,
            supports_token_verification: false,
            supports_batch_verification: false,
            supports_speculative_decode: false,
            supports_synthetic_token_cache: false,
            supports_mmap_cache_pack: false,
            supports_resident_model: false,
          };
          stage_plan = strategy_cli_bypass_plan_with_reason(cache_requested, format!("llama_server_request_failed: {}", strategy_trim(&err, 240)));
          stage_cache_pref = build_strategy_stage_cache_perf(&cli_capabilities, &stage_plan, prompt_tokens, 0);
        }
      }
    } else {
      let cli_capabilities = BackendCapabilities {
        backend_kind: BackendKind::CliSidecar,
        supports_resident: false,
        supports_prompt_token_cache: false,
        supports_prompt_cache: false,
        supports_prefix_kv_cache: false,
        supports_token_verification: false,
        supports_batch_verification: false,
        supports_speculative_decode: false,
        supports_synthetic_token_cache: false,
        supports_mmap_cache_pack: false,
        supports_resident_model: false,
      };
      stage_plan = strategy_cli_bypass_plan_with_reason(cache_requested, "llama_server_endpoint_missing".to_string());
      stage_cache_pref = build_strategy_stage_cache_perf(&cli_capabilities, &stage_plan, prompt_tokens, 0);
    }
  }

  let prompt_file_started = Instant::now();
  let system_prompt_file = write_strategy_prompt_file("system_prompt", &system_prompt)?;
  let user_prompt_file = match write_strategy_prompt_file("user_prompt", &user_prompt) {
    Ok(path) => path,
    Err(err) => {
      cleanup_strategy_prompt_file(&system_prompt_file);
      return Err(err);
    }
  };
  stage_overhead.prompt_file_write_ms = prompt_file_started.elapsed().as_millis();

  let mut command = Command::new(&runner);
  command
    .arg("-m")
    .arg(&model_path)
    .arg("-c")
    .arg(runtime.n_ctx.to_string())
    .arg("-n")
    .arg(max_tokens.to_string())
    .arg("-t")
    .arg(runtime.threads.to_string())
    .arg("--threads-batch")
    .arg(runtime.threads.to_string())
    .arg("--temp")
    .arg(tuning.temperature.to_string())
    .arg("--top-p")
    .arg(tuning.top_p.to_string())
    .arg("--repeat-penalty")
    .arg(tuning.repeat_penalty.to_string())
    .arg("--parallel")
    .arg("1")
    .arg("--simple-io")
    .arg("--no-display-prompt")
    .arg("--no-show-timings")
    .arg("--single-turn")
    .arg("--no-warmup")
    .arg("--device")
    .arg(runtime.device)
    .arg("--n-gpu-layers")
    .arg(runtime.n_gpu_layers.to_string())
    .arg("--color")
    .arg("off")
    .arg("--log-colors")
    .arg("off")
    .arg("--system-prompt-file")
    .arg(&system_prompt_file)
    .arg("--file")
    .arg(&user_prompt_file)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  configure_strategy_child_process(&mut command);

  let spawn_started = Instant::now();
  let mut child = command.spawn().map_err(|e| {
    cleanup_strategy_prompt_file(&system_prompt_file);
    cleanup_strategy_prompt_file(&user_prompt_file);
    format!(
      "{} 실행에 실패했어요. sidecar 포함 여부와 실행 권한을 확인해주세요. runner={} / 상세: {}",
      model_label,
      runner.display(),
      e
    )
  })?;
  stage_overhead.process_spawn_ms = spawn_started.elapsed().as_millis();
  emit_strategy_progress(app, stage, format!("{}로 응답을 생성하고 있어요.", model_label));

  let stdout = child.stdout.take().ok_or_else(|| format!("{} 표준출력을 연결하지 못했어요.", model_label))?;
  let stderr = child.stderr.take().ok_or_else(|| format!("{} 표준에러를 연결하지 못했어요.", model_label))?;

  let first_stdout_ms = Arc::new(AtomicU64::new(0));
  let first_stdout_ms_worker = Arc::clone(&first_stdout_ms);
  let started_for_stdout = started;
  let stdout_handle = thread::spawn(move || {
    let mut reader = BufReader::new(stdout);
    let mut bytes = Vec::<u8>::new();
    let mut buf = [0_u8; 8192];
    loop {
      match reader.read(&mut buf) {
        Ok(0) => break,
        Ok(n) => {
          if first_stdout_ms_worker.load(AtomicOrdering::Relaxed) == 0 {
            let elapsed = started_for_stdout.elapsed().as_millis() as u64;
            let _ = first_stdout_ms_worker.compare_exchange(0, elapsed.max(1), AtomicOrdering::Relaxed, AtomicOrdering::Relaxed);
          }
          bytes.extend_from_slice(&buf[..n]);
        }
        Err(_) => break,
      }
    }
    bytes
  });

  let app_for_stderr = app.cloned();
  let stderr_handle = thread::spawn(move || {
    let mut reader = BufReader::new(stderr);
    let mut collected = String::new();
    loop {
      let mut line = String::new();
      match reader.read_line(&mut line) {
        Ok(0) => break,
        Ok(_) => {
          collected.push_str(&line);
          let trimmed = line.trim();
          if !trimmed.is_empty() && should_emit_strategy_runtime_log(trimmed) {
            emit_strategy_progress(app_for_stderr.as_ref(), "모델로그", strategy_trim(trimmed, 260));
          }
        }
        Err(err) => {
          let msg = format!("표준에러 읽기 실패: {err}");
          collected.push_str(&msg);
          emit_strategy_progress(app_for_stderr.as_ref(), "모델로그", &msg);
          break;
        }
      }
    }
    collected
  });

  let mut timed_out = false;
  let mut last_heartbeat = 0_u64;
  let status = loop {
    if let Some(status) = child.try_wait().map_err(|e| format!("{} 상태 확인에 실패했어요: {e}", model_label))? {
      break status;
    }

    let elapsed = started.elapsed().as_secs();
    if elapsed >= STRATEGY_CHAT_TIMEOUT_SECS {
      timed_out = true;
      emit_strategy_progress(app, stage, format!("{}가 {}초 동안 끝나지 않아 실행을 중단할게요.", model_label, STRATEGY_CHAT_TIMEOUT_SECS));
      let _ = child.kill();
      let status = child.wait().map_err(|e| format!("중단된 {} 프로세스를 정리하지 못했어요: {e}", model_label))?;
      break status;
    }
    if elapsed >= last_heartbeat + 5 {
      last_heartbeat = elapsed;
      emit_strategy_progress(app, stage, format!("{} 응답을 기다리는 중이에요. {}초 경과했어요.", model_label, elapsed));
    }
    thread::sleep(Duration::from_millis(200));
  };

  let stdout = String::from_utf8_lossy(&stdout_handle.join().unwrap_or_default()).to_string();
  let stderr = stderr_handle.join().unwrap_or_else(|_| "stderr 수집 스레드가 비정상 종료되었어요.".to_string());
  stage_overhead.stdout_read_ms = started.elapsed().as_millis();
  cleanup_strategy_prompt_file(&system_prompt_file);
  cleanup_strategy_prompt_file(&user_prompt_file);
  let postprocess_started = Instant::now();
  let answer = finalize_strategy_answer(&cleanup_strategy_output(&stdout), evidence_packet, user_prompt_raw);
  stage_overhead.postprocess_ms = postprocess_started.elapsed().as_millis();

  if timed_out {
    return Err(format!(
      "{} 응답이 {}초 안에 끝나지 않아 중단했어요. 마지막 로그: {}",
      model_label,
      STRATEGY_CHAT_TIMEOUT_SECS,
      strategy_trim(stderr.trim(), 280)
    ));
  }
  if !status.success() && answer.is_empty() {
    return Err(format!(
      "{} 실행이 완료되지 않았어요. runner={} / stderr: {}",
      model_label,
      runner.display(),
      strategy_trim(stderr.trim(), 600)
    ));
  }
  if answer.is_empty() {
    return Err(format!("{}가 빈 응답을 반환했어요.", model_label));
  }

  let e2e_ms = started.elapsed().as_millis() as u128;
  let ttft_ms = first_stdout_ms.load(AtomicOrdering::Relaxed).max(1) as u128;
  let output_tokens = strategy_approx_output_tokens(&answer);
  let decode_ms = e2e_ms.saturating_sub(ttft_ms);
  let e2e_tps = {
    let seconds = (e2e_ms.max(1) as f32) / 1000.0;
    ((output_tokens as f32) / seconds * 100.0).round() / 100.0
  };
  let decode_tps = {
    let seconds = (decode_ms.max(1) as f32) / 1000.0;
    ((output_tokens as f32) / seconds * 100.0).round() / 100.0
  };
  let model_path_text = model_path.display().to_string();
  let runner_text = runner.display().to_string();
  emit_strategy_progress(app, stage, format!("{} 단계 응답을 {}자로 정리했어요.", model_label, answer.chars().count()));
  let mut stage_cache = stage_cache_pref;
  stage_cache.fallback_tokens = output_tokens;
  Ok(StrategyModelExecution {
    answer,
    model_path: model_path_text.clone(),
    runner: runner_text.clone(),
    prompt_chars,
    metrics: StrategyStagePerf {
      stage_name: stage.to_string(),
      model_id: model_id.to_string(),
      backend_kind: "cli".to_string(),
      runner_path: runner_text.clone(),
      model_path: model_path_text,
      threads: runtime.threads,
      threads_batch: runtime.threads,
      n_ctx: runtime.n_ctx,
      max_tokens,
      temperature: tuning.temperature,
      top_p: tuning.top_p,
      repeat_penalty: tuning.repeat_penalty,
      e2e_ms: e2e_ms.max(1),
      ttft_ms: Some(ttft_ms.max(1)),
      prompt_tokens,
      output_tokens,
      prompt_eval_ms: None,
      decode_ms: Some(decode_ms),
      e2e_tps,
      decode_tps,
      peak_memory_mb: strategy_estimate_model_footprint_mb(model_path.to_string_lossy().as_ref()),
      process_spawn_ms: stage_overhead.process_spawn_ms,
      prompt_file_write_ms: stage_overhead.prompt_file_write_ms,
      stdout_read_ms: stage_overhead.stdout_read_ms,
      postprocess_ms: stage_overhead.postprocess_ms,
      cache: stage_cache,
    },
  })
}

pub fn run_strategy_chat(
  app: Option<&AppHandle>,
  case_item: Option<&CaseItem>,
  records: &[RecordItem],
  message: &str,
  mode: Option<&str>,
  strategy_note: Option<&str>,
  conversation: &[StrategyChatTurn],
  opts: Option<StrategyChatOptions>,
) -> Result<StrategyChatRunResult, String> {
  let chat_mode = normalize_strategy_chat_mode(mode);
  let safe_message_owned = strategy_sanitize_text(message.trim());
  let safe_message = safe_message_owned.trim();
  let run_started = Instant::now();
  let benchmark_run_id = strategy_benchmark_run_id();
  let backend_runtime = strategy_backend_runtime(app, opts.as_ref());
  if safe_message.is_empty() {
    return Err("질문 내용이 비어 있어요.".to_string());
  }
  if chat_mode != STRATEGY_CHAT_MODE_RECORD && records.is_empty() {
    return Err("전략자문에 연결된 증거가 없어요.".to_string());
  }
  emit_strategy_progress(
    app,
    "준비",
    if chat_mode == STRATEGY_CHAT_MODE_RECORD {
      format!("기록모드 요청을 받았어요. 참고 기록 {}개와 입력 상황을 정리 중이에요.", records.len())
    } else {
      format!("전략자문 요청을 받았어요. 연결된 증거 {}개를 확인 중이에요.", records.len())
    },
  );
  let (evidence_packet, retrieval_query) = build_strategy_evidence_packet(case_item, records, safe_message, strategy_note);
  emit_strategy_progress(
    app,
    "근거정리",
    format!(
      "질문 기준으로 핵심 근거 {}건을 골랐어요. 검색 질의는 '{}'예요.",
      evidence_packet.evidence_records.len(),
      strategy_trim(retrieval_query.trim(), 120)
    ),
  );
  if !evidence_packet.legal_references.is_empty() {
    emit_strategy_progress(
      app,
      "법령정리",
      format!(
        "사건과 연결되는 법령·조문 {}건도 함께 골랐어요.",
        evidence_packet.legal_references.len()
      ),
    );
  }

  let requested_model_id = normalize_strategy_model_id(opts.as_ref().and_then(|x| x.model.as_deref()));
  let max_tokens = opts
    .as_ref()
    .and_then(|x| x.max_tokens)
    .unwrap_or(if requested_model_id == STRATEGY_MODEL_HYBRID_ID { 720 } else { 320 })
    .clamp(64, 768);
  let runtime = strategy_runtime_config(
    opts.as_ref().and_then(|x| x.n_ctx),
    opts.as_ref().and_then(|x| x.threads),
  );
  let n_ctx = runtime.n_ctx;
  let threads = runtime.threads;
  let synthetic_cache_enabled = opts
    .as_ref()
    .and_then(|x| x.synthetic_cache_enabled)
    .unwrap_or(true);
  let case_hash = case_item
    .map(strategy_hash_json)
    .unwrap_or_else(|| strategy_fast_hash_hex("no-case"));
  let records_hash = strategy_hash_json(&records);
  let model_config_hash = strategy_hash_json(&serde_json::json!({
    "model": requested_model_id,
    "mode": chat_mode,
    "n_ctx": n_ctx,
    "threads": threads,
    "max_tokens": max_tokens,
  }));
  let prompt_hash = strategy_fast_hash_hex(&format!(
    "{}|{}|{}|{}|{}",
    chat_mode,
    safe_message,
    retrieval_query,
    strategy_note.unwrap_or(""),
    conversation
      .iter()
      .map(|turn| format!("{}:{}", turn.role, strategy_trim(&turn.content, 160)))
      .collect::<Vec<_>>()
      .join("|")
  ));
  let build_perf_metrics = |final_answer: &str, stage_metrics: &[StrategyStagePerf]| {
    aggregate_strategy_perf_metrics(
      &benchmark_run_id,
      &prompt_hash,
      &case_hash,
      &records_hash,
      &model_config_hash,
      chat_mode,
      synthetic_cache_enabled,
      run_started.elapsed().as_millis() as u64,
      final_answer,
      stage_metrics,
    )
  };
  if chat_mode == STRATEGY_CHAT_MODE_RECORD {
    let record_system_prompt = build_strategy_record_system_prompt();
    let record_user_prompt = build_strategy_record_user_prompt(&evidence_packet, case_item, safe_message, strategy_note, conversation);
    emit_strategy_progress(app, "라우팅", "기록모드는 빠른캡쳐 형식을 유지하되, 사건 흐름이 충분히 남도록 기록 전용 경로로 정리할게요.");
    if requested_model_id == STRATEGY_MODEL_HYBRID_ID {
      let draft_n_ctx = strategy_hybrid_draft_n_ctx(n_ctx).max(3328);
      emit_strategy_progress(app, "준비", "HyperCLOVA-X가 사건 흐름과 누락 포인트를 먼저 촘촘히 구조화하고, Roosy-X가 읽기 흐름과 실무 표현만 보조할게요.");
      let mut hyper_draft_error = String::new();
      let hyper_draft = match execute_strategy_model(
        app,
        &backend_runtime,
        STRATEGY_MODEL_DEFAULT_ID,
        HybridStage::RecordMain,
        &record_system_prompt,
        &record_user_prompt,
        &evidence_packet,
        draft_n_ctx,
        max_tokens.min(760).max(640),
        threads,
        "초안1",
        synthetic_cache_enabled,
      ) {
        Ok(result) => Some(result),
        Err(err) => {
          hyper_draft_error = err.clone();
          emit_strategy_progress(app, "초안1", format!("HyperCLOVA-X 기록 초안이 잠시 흔들렸어요. {}", strategy_trim(&err, 220)));
          None
        }
      };

      let mut roosy_draft_error = String::new();
      let roosy_draft = match execute_strategy_model(
        app,
        &backend_runtime,
        STRATEGY_MODEL_ROOSY_ID,
        HybridStage::RecordFill,
        &record_system_prompt,
        &hyper_draft
          .as_ref()
          .map(|hyper| {
            build_strategy_record_fill_user_prompt(
              &evidence_packet,
              case_item,
              safe_message,
              strategy_note,
              &hyper.answer,
            )
          })
          .unwrap_or_else(|| record_user_prompt.clone()),
        &evidence_packet,
        draft_n_ctx,
        max_tokens.min(700).max(560),
        threads,
        "초안2",
        synthetic_cache_enabled,
      ) {
        Ok(result) => Some(result),
        Err(err) => {
          roosy_draft_error = err.clone();
          emit_strategy_progress(app, "초안2", format!("Roosy-X 기록 초안이 잠시 흔들렸어요. {}", strategy_trim(&err, 220)));
          None
        }
      };

      match (hyper_draft, roosy_draft) {
        (Some(hyper), Some(roosy)) => {
          if synthetic_cache_enabled {
            if let Some(reviewed) = strategy_try_render_record_without_llm(
              app,
              &evidence_packet,
              safe_message,
              &hyper.answer,
              true,
              "초안1",
            ) {
              let final_answer = finalize_strategy_record_answer(&reviewed.answer, &evidence_packet, safe_message);
              let perf_metrics = build_perf_metrics(
                &final_answer,
                &finalize_strategy_record_stage_metrics(vec![hyper.metrics.clone()], reviewed),
              );
              emit_strategy_progress(app, "완료", "HyperCLOVA-X 초안만으로 충분해 캐시모드 빠른 경로로 바로 기록 생성을 마쳤어요.");
              return Ok(finalize_strategy_chat_run_result(
                app,
                &benchmark_run_id,
                final_answer,
                "ROOSY-Hybrid (PrefixKV+TemplateRenderer fast path)".to_string(),
                hyper.runner,
                hyper.prompt_chars,
                evidence_packet.evidence_records.len(),
                retrieval_query,
                evidence_packet,
                perf_metrics,
              ));
            }
          }
          emit_strategy_progress(app, "합성", "HyperCLOVA-X 초안을 중심으로 유지하고, Roosy-X는 문장 흐름과 읽기 편한 표현만 보강해 최종 기록으로 묶을게요.");
          let hybrid_prompt = build_strategy_record_hybrid_user_prompt(
            &evidence_packet,
            case_item,
            safe_message,
            strategy_note,
            &hyper.answer,
            &roosy.answer,
          );
          let synthesis = match execute_strategy_model(
            app,
            &backend_runtime,
            STRATEGY_MODEL_DEFAULT_ID,
            HybridStage::RecordSynthesis,
            &build_strategy_record_hybrid_system_prompt(),
            &hybrid_prompt,
            &evidence_packet,
            n_ctx.max(3584),
            (max_tokens + 120).clamp(700, 768),
            threads,
            "합성",
            synthetic_cache_enabled,
          ) {
            Ok(result) => result,
            Err(err) => {
              emit_strategy_progress(app, "합성", format!("최종 기록 합성이 흔들려서 HyperCLOVA-X 초안을 우선 보여드릴게요. {}", strategy_trim(&err, 220)));
              hyper.clone()
            }
          };
          let reviewed = maybe_review_strategy_record_answer(
            app,
            &backend_runtime,
            &evidence_packet,
            case_item,
            safe_message,
            strategy_note,
            &synthesis.answer,
            n_ctx,
            max_tokens,
            threads,
            synthetic_cache_enabled,
          );
          let final_answer = finalize_strategy_record_answer(&reviewed.answer, &evidence_packet, safe_message);
          let perf_metrics = build_perf_metrics(
            &final_answer,
            &finalize_strategy_record_stage_metrics(
              vec![hyper.metrics.clone(), roosy.metrics.clone(), synthesis.metrics.clone()],
              reviewed,
            ),
          );
          emit_strategy_progress(app, "완료", format!("기록 초안 생성을 마쳤어요. 본문 길이 {}자예요.", final_answer.chars().count()));
          return Ok(finalize_strategy_chat_run_result(
            app,
            &benchmark_run_id,
            final_answer,
            "ROOSY-Hybrid (HyperCLOVA-X 중심 + Roosy-X 보조 record draft)".to_string(),
            synthesis.runner,
            synthesis.prompt_chars,
            evidence_packet.evidence_records.len(),
            retrieval_query,
            evidence_packet,
            perf_metrics,
          ));
        }
        (Some(hyper), None) => {
          let reviewed = maybe_review_strategy_record_answer(
            app,
            &backend_runtime,
            &evidence_packet,
            case_item,
            safe_message,
            strategy_note,
            &hyper.answer,
            n_ctx,
            max_tokens,
            threads,
            synthetic_cache_enabled,
          );
          let final_answer = finalize_strategy_record_answer(&reviewed.answer, &evidence_packet, safe_message);
          let perf_metrics = build_perf_metrics(
            &final_answer,
            &finalize_strategy_record_stage_metrics(vec![hyper.metrics.clone()], reviewed),
          );
          emit_strategy_progress(app, "완료", "Roosy-X 초안이 비어 있어 HyperCLOVA-X 기반 기록 초안을 먼저 정리했어요.");
          return Ok(finalize_strategy_chat_run_result(
            app,
            &benchmark_run_id,
            final_answer,
            "ROOSY-Hybrid (HyperCLOVA-X record fallback)".to_string(),
            hyper.runner,
            hyper.prompt_chars,
            evidence_packet.evidence_records.len(),
            retrieval_query,
            evidence_packet,
            perf_metrics,
          ));
        }
        (None, Some(roosy)) => {
          let reviewed = maybe_review_strategy_record_answer(
            app,
            &backend_runtime,
            &evidence_packet,
            case_item,
            safe_message,
            strategy_note,
            &roosy.answer,
            n_ctx,
            max_tokens,
            threads,
            synthetic_cache_enabled,
          );
          let final_answer = finalize_strategy_record_answer(&reviewed.answer, &evidence_packet, safe_message);
          let perf_metrics = build_perf_metrics(
            &final_answer,
            &finalize_strategy_record_stage_metrics(vec![roosy.metrics.clone()], reviewed),
          );
          emit_strategy_progress(app, "완료", "HyperCLOVA-X 초안이 비어 있어 Roosy-X 기반 기록 초안을 먼저 정리했어요.");
          return Ok(finalize_strategy_chat_run_result(
            app,
            &benchmark_run_id,
            final_answer,
            "ROOSY-Hybrid (Roosy-X record fallback)".to_string(),
            roosy.runner,
            roosy.prompt_chars,
            evidence_packet.evidence_records.len(),
            retrieval_query,
            evidence_packet,
            perf_metrics,
          ));
        }
        (None, None) => {
          emit_strategy_progress(app, "복구초안", "하이브리드 초안 두 개가 모두 흔들려서 HyperCLOVA-X 단일 기록 초안으로 한 번 더 복구 시도할게요.");
          match execute_strategy_model(
            app,
            &backend_runtime,
            STRATEGY_MODEL_DEFAULT_ID,
            HybridStage::RecordRecovery,
            &record_system_prompt,
            &record_user_prompt,
            &evidence_packet,
            n_ctx.max(3328),
            max_tokens.max(620),
            threads,
            "복구초안",
            false,
          ) {
            Ok(result) => {
              let reviewed = maybe_review_strategy_record_answer(
                app,
                &backend_runtime,
                &evidence_packet,
                case_item,
                safe_message,
                strategy_note,
                &result.answer,
                n_ctx,
                max_tokens,
                threads,
                false,
              );
              let final_answer = finalize_strategy_record_answer(&reviewed.answer, &evidence_packet, safe_message);
              let perf_metrics = build_perf_metrics(
                &final_answer,
                &finalize_strategy_record_stage_metrics(vec![result.metrics.clone()], reviewed),
              );
              emit_strategy_progress(app, "완료", "하이브리드 초안은 흔들렸지만 HyperCLOVA-X 단일 복구 초안으로 기록 생성을 이어갔어요.");
              return Ok(finalize_strategy_chat_run_result(
                app,
                &benchmark_run_id,
                final_answer,
                "HyperCLOVA-X record recovery fallback".to_string(),
                result.runner,
                result.prompt_chars,
                evidence_packet.evidence_records.len(),
                retrieval_query,
                evidence_packet,
                perf_metrics,
              ));
            }
            Err(recovery_err) => {
              let combined_detail = format!(
                "초안1: {} / 초안2: {} / 복구초안: {}",
                strategy_trim(&hyper_draft_error, 180),
                strategy_trim(&roosy_draft_error, 180),
                strategy_trim(&recovery_err, 220),
              );
              #[cfg(target_os = "windows")]
              return Err(format!(
                "기록모드 초안 두 개를 모두 만들지 못했어요. 먼저 AI 모델 다운로드와 sidecar 상태를 함께 확인해주세요. {}",
                combined_detail
              ));
              #[cfg(not(target_os = "windows"))]
              return Err(format!(
                "기록모드 초안 두 개를 모두 만들지 못했어요. 번들된 모델 파일과 sidecar 상태를 함께 확인해주세요. {}",
                combined_detail
              ));
            }
          }
        }
      }
    }

    let result = execute_strategy_model(
      app,
      &backend_runtime,
      requested_model_id,
      HybridStage::RecordMain,
      &record_system_prompt,
      &record_user_prompt,
      &evidence_packet,
      strategy_hybrid_draft_n_ctx(n_ctx).max(3328),
      max_tokens.max(620),
      threads,
      "기록초안",
      synthetic_cache_enabled,
    )?;
    let reviewed = maybe_review_strategy_record_answer(
      app,
      &backend_runtime,
      &evidence_packet,
      case_item,
      safe_message,
      strategy_note,
      &result.answer,
      n_ctx,
      max_tokens,
      threads,
      synthetic_cache_enabled,
    );
    let final_answer = finalize_strategy_record_answer(&reviewed.answer, &evidence_packet, safe_message);
    let perf_metrics = build_perf_metrics(
      &final_answer,
      &finalize_strategy_record_stage_metrics(vec![result.metrics.clone()], reviewed),
    );
    emit_strategy_progress(app, "완료", format!("기록 초안 생성을 마쳤어요. 본문 길이 {}자예요.", final_answer.chars().count()));
    return Ok(finalize_strategy_chat_run_result(
      app,
      &benchmark_run_id,
      final_answer,
      result.model_path,
      result.runner,
      result.prompt_chars,
      evidence_packet.evidence_records.len(),
      retrieval_query,
      evidence_packet,
      perf_metrics,
    ));
  }
  let system_prompt = build_strategy_system_prompt();
  let user_prompt = build_strategy_user_prompt(&evidence_packet, case_item, safe_message, strategy_note, conversation);
  let draft_user_prompt = build_strategy_user_prompt_for_draft(&evidence_packet, case_item, safe_message, strategy_note);
  let question_route = strategy_question_route(safe_message);

  if requested_model_id == STRATEGY_MODEL_HYBRID_ID {
    if question_route == StrategyQuestionRoute::FastRoosy {
      emit_strategy_progress(app, "라우팅", "이번 질문은 짧은 확인성 대화라 Roosy-X 단일 경로로 빠르게 정리할게요.");
      let fast_result = execute_strategy_model(
        app,
        &backend_runtime,
        STRATEGY_MODEL_ROOSY_ID,
        HybridStage::FastRoosy,
        &system_prompt,
        &user_prompt,
        &evidence_packet,
        strategy_hybrid_draft_n_ctx(n_ctx),
        max_tokens.min(640).max(280),
        threads,
        "빠른실행",
        synthetic_cache_enabled,
      )?;
      let perf_metrics = build_perf_metrics(&fast_result.answer, &[fast_result.metrics.clone()]);
      emit_strategy_progress(app, "완료", format!("빠른 경로 응답 생성을 마쳤어요. 본문 길이 {}자예요.", fast_result.answer.chars().count()));
      return Ok(finalize_strategy_chat_run_result(
        app,
        &benchmark_run_id,
        fast_result.answer,
        "ROOSY-Hybrid (Roosy fast path)".to_string(),
        fast_result.runner,
        fast_result.prompt_chars,
        evidence_packet.evidence_records.len(),
        retrieval_query,
        evidence_packet,
        perf_metrics,
      ));
    }

    emit_strategy_progress(app, "라우팅", "이번 질문은 비교·민원문구·법령 연결 성격이 있어 하이브리드 전체를 돌릴게요.");
    emit_strategy_progress(app, "준비", "Roosy-X 1차 초안에 사건 맥락을 충분히 먹이고, HyperCLOVA-X가 균형 검토한 뒤 최종 정리를 붙일게요.");
    let draft_n_ctx = strategy_hybrid_draft_n_ctx(n_ctx);

    let roosy_draft = match execute_strategy_model(
      app,
      &backend_runtime,
      STRATEGY_MODEL_ROOSY_ID,
      HybridStage::GeneralDraft,
      &system_prompt,
      &user_prompt,
      &evidence_packet,
      draft_n_ctx,
      max_tokens.min(640).max(420),
      threads,
      "초안1",
      synthetic_cache_enabled,
    ) {
      Ok(result) => Some(result),
      Err(err) => {
        emit_strategy_progress(app, "초안1", format!("Roosy-X 초안이 잠시 흔들렸어요. {}", strategy_trim(&err, 220)));
        None
      }
    };
    if let Some(roosy) = roosy_draft.as_ref() {
      emit_strategy_progress(
        app,
        "초안공유",
        format!("1차 초안 포인트: {}", strategy_trim(&roosy.answer.replace('\n', " "), 150)),
      );
    }

    let hyper_draft = match execute_strategy_model(
      app,
      &backend_runtime,
      STRATEGY_MODEL_DEFAULT_ID,
      HybridStage::GeneralMain,
      &system_prompt,
      &draft_user_prompt,
      &evidence_packet,
      draft_n_ctx,
      max_tokens.min(520).max(320),
      threads,
      "초안2",
      synthetic_cache_enabled,
    ) {
      Ok(result) => Some(result),
      Err(err) => {
        emit_strategy_progress(app, "초안2", format!("HyperCLOVA-X 검토 초안이 잠시 흔들렸어요. {}", strategy_trim(&err, 220)));
        None
      }
    };

    match (roosy_draft, hyper_draft) {
      (Some(roosy), Some(hyper)) => {
        emit_strategy_progress(app, "합성", "Roosy-X 초안을 바탕으로 가되, HyperCLOVA-X의 근거·균형 검토를 반영해 최종 답변으로 묶을게요.");
        let hybrid_prompt = build_strategy_hybrid_user_prompt(
          &evidence_packet,
          case_item,
          safe_message,
          strategy_note,
          &hyper.answer,
          &roosy.answer,
        );
        let synthesis = match execute_strategy_model(
          app,
          &backend_runtime,
          STRATEGY_MODEL_ROOSY_ID,
          HybridStage::GeneralSynthesis,
          &build_strategy_hybrid_system_prompt(),
          &hybrid_prompt,
          &evidence_packet,
          n_ctx,
          (max_tokens + 120).clamp(620, 768),
          threads,
          "합성",
          synthetic_cache_enabled,
        ) {
          Ok(result) => result,
          Err(err) => {
            emit_strategy_progress(app, "합성", format!("최종 합성 단계가 흔들려서 Roosy-X 초안을 우선 보여드릴게요. {}", strategy_trim(&err, 220)));
            roosy.clone()
          }
        };

        let perf_metrics = build_perf_metrics(&synthesis.answer, &[roosy.metrics.clone(), hyper.metrics.clone(), synthesis.metrics.clone()]);
        emit_strategy_progress(app, "완료", format!("ROOSY-Hybrid 응답 생성을 마쳤어요. 본문 길이 {}자예요.", synthesis.answer.chars().count()));
        return Ok(finalize_strategy_chat_run_result(
          app,
          &benchmark_run_id,
          synthesis.answer,
          "ROOSY-Hybrid (Roosy-X + HyperCLOVA-X)".to_string(),
          synthesis.runner,
          synthesis.prompt_chars,
          evidence_packet.evidence_records.len(),
          retrieval_query,
          evidence_packet,
          perf_metrics,
        ));
      }
      (Some(roosy), None) => {
        emit_strategy_progress(app, "완료", "HyperCLOVA-X 검토 초안이 비어 있어 Roosy-X 기반으로 먼저 정리했어요.");
        let perf_metrics = build_perf_metrics(&roosy.answer, &[roosy.metrics.clone()]);
        return Ok(finalize_strategy_chat_run_result(
          app,
          &benchmark_run_id,
          roosy.answer,
          "ROOSY-Hybrid (Roosy-X fallback)".to_string(),
          roosy.runner,
          roosy.prompt_chars,
          evidence_packet.evidence_records.len(),
          retrieval_query,
          evidence_packet,
          perf_metrics,
        ));
      }
      (None, Some(hyper)) => {
        emit_strategy_progress(app, "완료", "Roosy-X 초안이 비어 있어 HyperCLOVA-X 기반으로 먼저 정리했어요.");
        let perf_metrics = build_perf_metrics(&hyper.answer, &[hyper.metrics.clone()]);
        return Ok(finalize_strategy_chat_run_result(
          app,
          &benchmark_run_id,
          hyper.answer,
          "ROOSY-Hybrid (HyperCLOVA-X fallback)".to_string(),
          hyper.runner,
          hyper.prompt_chars,
          evidence_packet.evidence_records.len(),
          retrieval_query,
          evidence_packet,
          perf_metrics,
        ));
      }
      (None, None) => {
        #[cfg(target_os = "windows")]
        return Err("ROOSY-Hybrid 초안 두 개를 모두 만들지 못했어요. 먼저 AI 모델 다운로드가 완료됐는지, 그리고 sidecar가 정상인지 함께 확인해주세요.".to_string());
        #[cfg(not(target_os = "windows"))]
        return Err("ROOSY-Hybrid 초안 두 개를 모두 만들지 못했어요. 번들된 모델 파일과 sidecar 상태를 함께 확인해주세요.".to_string());
      }
    }
  }

  let result = execute_strategy_model(
    app,
    &backend_runtime,
    requested_model_id,
    HybridStage::GeneralMain,
    &system_prompt,
    &user_prompt,
    &evidence_packet,
    n_ctx,
    max_tokens,
    threads,
    "실행",
    synthetic_cache_enabled,
  )?;
  let perf_metrics = build_perf_metrics(&result.answer, &[result.metrics.clone()]);
  emit_strategy_progress(app, "완료", format!("응답 생성을 마쳤어요. 본문 길이 {}자예요.", result.answer.chars().count()));

  Ok(finalize_strategy_chat_run_result(
    app,
    &benchmark_run_id,
    result.answer,
    result.model_path,
    result.runner,
    result.prompt_chars,
    evidence_packet.evidence_records.len(),
    retrieval_query,
    evidence_packet,
    perf_metrics,
  ))
}
