use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH}; // 시간 처리를 위한 표준 라이브러리 추가
use tauri::{path::BaseDirectory, AppHandle, Emitter, Manager};

const RISK_LABEL_TEXT: [&str; 3] = ["평범", "경고", "위험"];
const RISK_MODEL_BYTES: &[u8] = include_bytes!("risk_model_v1.bin");

#[derive(Debug, Clone)]
struct RiskLinearModel {
  version: String,
  dims: usize,
  bias: [f32; 3],
  weights: [Vec<f32>; 3],
}

static RISK_MODEL: OnceLock<RiskLinearModel> = OnceLock::new();

fn read_u32_le(bytes: &[u8], pos: &mut usize) -> Result<u32, String> {
  let end = *pos + 4;
  let chunk = bytes.get(*pos..end).ok_or_else(|| "risk model truncated while reading u32".to_string())?;
  *pos = end;
  Ok(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

fn read_f32_le(bytes: &[u8], pos: &mut usize) -> Result<f32, String> {
  let end = *pos + 4;
  let chunk = bytes.get(*pos..end).ok_or_else(|| "risk model truncated while reading f32".to_string())?;
  *pos = end;
  Ok(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

fn load_risk_model_from_bytes(bytes: &[u8]) -> Result<RiskLinearModel, String> {
  let magic = bytes.get(0..8).ok_or_else(|| "risk model too short".to_string())?;
  if magic != b"RCZRISK1" {
    return Err("invalid risk model magic".to_string());
  }

  let mut pos = 8usize;
  let version_len = read_u32_le(bytes, &mut pos)? as usize;
  let version_bytes = bytes
    .get(pos..pos + version_len)
    .ok_or_else(|| "risk model truncated while reading version".to_string())?;
  let version = String::from_utf8(version_bytes.to_vec()).map_err(|_| "risk model version is not valid utf-8".to_string())?;
  pos += version_len;

  let dims = read_u32_le(bytes, &mut pos)? as usize;
  let class_count = read_u32_le(bytes, &mut pos)? as usize;
  if class_count != 3 {
    return Err(format!("unsupported risk class count: {}", class_count));
  }

  let mut bias = [0.0f32; 3];
  for i in 0..3 {
    bias[i] = read_f32_le(bytes, &mut pos)?;
  }

  let mut weights = [Vec::<f32>::with_capacity(dims), Vec::<f32>::with_capacity(dims), Vec::<f32>::with_capacity(dims)];
  for cls in 0..3 {
    for _ in 0..dims {
      weights[cls].push(read_f32_le(bytes, &mut pos)?);
    }
  }

  Ok(RiskLinearModel { version, dims, bias, weights })
}

fn risk_model() -> &'static RiskLinearModel {
  RISK_MODEL.get_or_init(|| load_risk_model_from_bytes(RISK_MODEL_BYTES).expect("failed to load risk_model_v1.bin"))
}

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

fn has_actor_type(r: &RecordItem, actor_type: &str) -> bool {
  let target = actor_type.trim();
  if !target.is_empty() {
    if r.actor.r#type.trim() == target {
      return true;
    }
    for a in &r.actors {
      if a.r#type.trim() == target {
        return true;
      }
    }
  }
  false
}


/* -------------------- shared types (proto) -------------------- */

pub type Sensitivity = String;
pub type StoreType = String;
pub type PlaceType = String;
pub type CaseSensFilter = String;
pub type CaseStatus = String;

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


/* -------------------- complaint risk classify -------------------- */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskPrediction {
  pub label: u8,
  pub label_text: String,
  pub probs: [f32; 3],
  pub confidence: f32,
  #[serde(default)]
  pub reasons: Vec<String>,
  pub model_version: String,
}

const RISK_COUNT_CAP: f32 = 3.0;

const LEGAL_KWS: &[&str] = &["고소", "고발", "변호사", "손해배상", "법적", "경찰", "신고", "아동학대"];
const AUTHORITY_KWS: &[&str] = &["교육청", "국민신문고", "감사", "장학사", "기관"];
const RECORDING_KWS: &[&str] = &["녹취", "녹음", "캡처", "증거", "증빙"];
const PUBLICATION_KWS: &[&str] = &["언론", "제보", "온라인", "커뮤니티", "맘카페", "공개", "실명", "배포"];
const PRESSURE_KWS: &[&str] = &["압박", "재촉", "즉답", "즉시", "오늘 중", "답변 기한", "책임", "과실", "문제 삼음", "납득하지 못"];
const REPEAT_KWS: &[&str] = &["반복 연락", "여러 번호", "같은 내용", "재차", "계속", "반복"];
const VISIT_KWS: &[&str] = &["직접 찾아오", "학교로 직접", "방문", "면담"];
const ADMIN_KWS: &[&str] = &["교감", "교장", "관리자", "동석"];

fn fnv1a64_mod(s: &str, dims: usize) -> usize {
  let mut hash: u64 = 0xcbf29ce484222325;
  for b in s.as_bytes() {
    hash ^= *b as u64;
    hash = hash.wrapping_mul(0x100000001b3);
  }
  (hash % dims as u64) as usize
}

fn clean_summary_for_ngrams(s: &str) -> Vec<char> {
  norm(s)
    .chars()
    .filter(|ch| ch.is_alphanumeric() || is_word_char(*ch as u32))
    .collect::<Vec<char>>()
}

fn add_feature(feats: &mut HashMap<usize, f32>, feature: &str) {
  let idx = fnv1a64_mod(feature, risk_model().dims);
  let entry = feats.entry(idx).or_insert(0.0);
  *entry = (*entry + 1.0).min(RISK_COUNT_CAP);
}

fn add_category_features(feats: &mut HashMap<usize, f32>, summary: &str, category: &str, kws: &[&str]) -> bool {
  let mut hit = false;
  for kw in kws {
    if summary.contains(kw) {
      hit = true;
      add_feature(feats, &format!("kw={}", kw));
    }
  }
  if hit {
    add_feature(feats, &format!("kwcat={}", category));
  }
  hit
}

fn risk_feature_counts(r: &RecordItem) -> HashMap<usize, f32> {
  let mut feats = HashMap::<usize, f32>::new();
  let summary = norm(&r.summary);

  let related_bucket = match r.related.len() {
    0 => "0",
    1 => "1",
    _ => "2+",
  };

  let summary_len_bucket = if summary.len() < 40 {
    "short"
  } else if summary.len() < 90 {
    "mid"
  } else {
    "long"
  };

  add_feature(&mut feats, &format!("actor={}", r.actor.r#type.trim()));
  for a in &r.actors {
    let actor_type = a.r#type.trim();
    if !actor_type.is_empty() {
      add_feature(&mut feats, &format!("actor={}", actor_type));
    }
  }
  add_feature(&mut feats, &format!("place={}", r.place.trim()));
  add_feature(&mut feats, &format!("store={}", r.store_type.trim()));
  add_feature(&mut feats, &format!("lv={}", r.lv.trim()));
  add_feature(&mut feats, &format!("related_bucket={}", related_bucket));
  add_feature(&mut feats, &format!("place_store={}|{}", r.place.trim(), r.store_type.trim()));
  add_feature(&mut feats, &format!("summary_len={}", summary_len_bucket));

  for tok in tokenize(&summary) {
    add_feature(&mut feats, &format!("tok={}", tok));
  }

  let chars = clean_summary_for_ngrams(&summary);
  for n in 2usize..=4usize {
    if chars.len() >= n {
      for i in 0..=(chars.len() - n) {
        let gram: String = chars[i..i + n].iter().collect();
        add_feature(&mut feats, &format!("cg{}={}", n, gram));
      }
    }
  }

  add_category_features(&mut feats, &summary, "legal", LEGAL_KWS);
  add_category_features(&mut feats, &summary, "authority", AUTHORITY_KWS);
  add_category_features(&mut feats, &summary, "recording", RECORDING_KWS);
  add_category_features(&mut feats, &summary, "publication", PUBLICATION_KWS);
  add_category_features(&mut feats, &summary, "pressure", PRESSURE_KWS);
  add_category_features(&mut feats, &summary, "repeat", REPEAT_KWS);
  add_category_features(&mut feats, &summary, "visit", VISIT_KWS);
  add_category_features(&mut feats, &summary, "admin", ADMIN_KWS);

  feats
}

fn softmax3(logits: &mut [f32; 3]) {
  let max_v = logits
    .iter()
    .copied()
    .fold(f32::NEG_INFINITY, f32::max);
  let mut sum = 0.0f32;
  for x in logits.iter_mut() {
    *x = (*x - max_v).exp();
    sum += *x;
  }
  let denom = if sum <= 0.0 { 1.0 } else { sum };
  for x in logits.iter_mut() {
    *x /= denom;
  }
}

fn push_reason(out: &mut Vec<String>, reason: &str) {
  if !out.iter().any(|x| x == reason) {
    out.push(reason.to_string());
  }
}

fn reason_hits(summary: &str, kws: &[&str]) -> bool {
  kws.iter().any(|kw| summary.contains(kw))
}

fn collect_risk_reasons(r: &RecordItem, label: usize, confidence: f32) -> Vec<String> {
  let summary = norm(&r.summary);
  let mut out = Vec::<String>::new();

  if reason_hits(&summary, AUTHORITY_KWS) {
    push_reason(&mut out, "교육청·외부기관 언급");
  }
  if reason_hits(&summary, LEGAL_KWS) {
    push_reason(&mut out, "법적 조치·신고 표현");
  }
  if reason_hits(&summary, RECORDING_KWS) {
    push_reason(&mut out, "녹취·캡처·증빙 확보 언급");
  }
  if reason_hits(&summary, PUBLICATION_KWS) {
    push_reason(&mut out, "온라인 공개·언론 확산 우려");
  }
  if reason_hits(&summary, PRESSURE_KWS) {
    push_reason(&mut out, "압박성 요구·즉답 촉구");
  }
  if reason_hits(&summary, REPEAT_KWS) {
    push_reason(&mut out, "반복 연락·지속 압박");
  }
  if reason_hits(&summary, VISIT_KWS) {
    push_reason(&mut out, "직접 방문·면담 압박");
  }
  if reason_hits(&summary, ADMIN_KWS) {
    push_reason(&mut out, "관리자 동석·공유 필요");
  }

  if out.is_empty() {
    match label {
      2 => push_reason(&mut out, "즉시 대응이 필요한 고위험 신호"),
      1 => push_reason(&mut out, "민원으로 번질 수 있는 경고 신호"),
      _ => push_reason(&mut out, "일반 안내·공유 수준"),
    }
  }

  if label >= 1 && has_actor_type(r, "학부모") {
    push_reason(&mut out, "학부모 직접 민원 반응");
  }

  if label == 2 && confidence >= 0.80 {
    push_reason(&mut out, "고신뢰 위험 판정");
  }

  out.truncate(4);
  out
}

fn predict_risk(record: &RecordItem) -> RiskPrediction {
  let feats = risk_feature_counts(record);
  let model = risk_model();

  let mut logits = [
    model.bias[0],
    model.bias[1],
    model.bias[2],
  ];

  for (idx, val) in feats.iter() {
    let i = *idx;
    let v = *val;
    logits[0] += model.weights[0][i] * v;
    logits[1] += model.weights[1][i] * v;
    logits[2] += model.weights[2][i] * v;
  }

  softmax3(&mut logits);

  let mut best = 0usize;
  for i in 1..3 {
    if logits[i] > logits[best] {
      best = i;
    }
  }

  let label_text = RISK_LABEL_TEXT[best].to_string();
  let confidence = logits[best];
  let reasons = collect_risk_reasons(record, best, confidence);

  RiskPrediction {
    label: best as u8,
    label_text,
    probs: logits,
    confidence,
    reasons,
    model_version: model.version.clone(),
  }
}

pub fn classify_records_risk(records: &[RecordItem]) -> Vec<RiskPrediction> {
  records.iter().map(predict_risk).collect()
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


const STRATEGY_MODEL_FILENAME: &str = "HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf";
const STRATEGY_MODEL_RESOURCE_PATH: &str = "models/HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf";
const STRATEGY_SIDECAR_STEM: &str = "llama-sidecar";
const STRATEGY_PROGRESS_EVENT: &str = "strategy-chat-progress";
const STRATEGY_CHAT_TIMEOUT_SECS: u64 = 90;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar-aarch64-apple-darwin";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar-x86_64-apple-darwin";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar-x86_64-pc-windows-msvc.exe";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar-x86_64-unknown-linux-gnu";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar-aarch64-unknown-linux-gnu";
#[cfg(not(any(
  all(target_os = "macos", target_arch = "aarch64"),
  all(target_os = "macos", target_arch = "x86_64"),
  all(target_os = "windows", target_arch = "x86_64"),
  all(target_os = "linux", target_arch = "x86_64"),
  all(target_os = "linux", target_arch = "aarch64")
)))]
const STRATEGY_SIDECAR_FILENAME: &str = "llama-sidecar";

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
  pub max_tokens: Option<u32>,
  #[serde(default)]
  pub n_ctx: Option<u32>,
  #[serde(default)]
  pub threads: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyChatRunResult {
  pub answer: String,
  pub model_path: String,
  pub runner: String,
  pub prompt_chars: usize,
  pub records_used: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StrategyProgressPayload {
  stage: String,
  message: String,
}

fn strategy_trim(s: &str, limit: usize) -> String {
  s.chars().take(limit).collect::<String>()
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

fn strategy_runner_candidates(app: Option<&AppHandle>) -> Vec<PathBuf> {
  let mut out = Vec::<PathBuf>::new();
  if let Ok(path) = env::var("ROOSYCOZY_LLAMA_CLI") {
    let trimmed = path.trim();
    if !trimmed.is_empty() {
      push_unique_path(&mut out, PathBuf::from(trimmed));
    }
  }

  if let Some(app) = app {
    if let Ok(path) = app.path().resolve(format!("sidecar/{}", STRATEGY_SIDECAR_FILENAME), BaseDirectory::Resource) {
      push_unique_path(&mut out, path);
    }
    if let Ok(path) = app.path().resolve(STRATEGY_SIDECAR_FILENAME, BaseDirectory::Resource) {
      push_unique_path(&mut out, path);
    }
  }

  let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
  push_unique_path(&mut out, manifest.join("binaries").join(STRATEGY_SIDECAR_FILENAME));
  push_unique_path(&mut out, manifest.join("resources").join("sidecar").join(STRATEGY_SIDECAR_FILENAME));
  push_unique_path(&mut out, manifest.join(STRATEGY_SIDECAR_FILENAME));

  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      push_unique_path(&mut out, dir.join(STRATEGY_SIDECAR_FILENAME));
      push_unique_path(&mut out, dir.join("sidecar").join(STRATEGY_SIDECAR_FILENAME));
      push_unique_path(&mut out, dir.join("resources").join("sidecar").join(STRATEGY_SIDECAR_FILENAME));
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
  Err(format!(
    "전략자문 추론기 파일을 찾지 못했어요. {} 파일을 앱 번들 리소스 sidecar 경로나 src-tauri/binaries 폴더에 넣어주세요.",
    STRATEGY_SIDECAR_FILENAME
  ))
}

fn strategy_model_candidates(app: Option<&AppHandle>) -> Vec<PathBuf> {
  let mut out = Vec::<PathBuf>::new();
  if let Ok(path) = env::var("ROOSYCOZY_HYPERCLOVA_MODEL") {
    let trimmed = path.trim();
    if !trimmed.is_empty() {
      push_unique_path(&mut out, PathBuf::from(trimmed));
    }
  }

  if let Some(app) = app {
    if let Ok(path) = app.path().resolve(STRATEGY_MODEL_RESOURCE_PATH, BaseDirectory::Resource) {
      push_unique_path(&mut out, path);
    }
    if let Ok(path) = app.path().resolve(STRATEGY_MODEL_FILENAME, BaseDirectory::Resource) {
      push_unique_path(&mut out, path);
    }
  }

  let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
  push_unique_path(&mut out, manifest.join("src").join("engine").join(STRATEGY_MODEL_FILENAME));
  push_unique_path(&mut out, manifest.join("resources").join("models").join(STRATEGY_MODEL_FILENAME));
  push_unique_path(&mut out, manifest.join(STRATEGY_MODEL_FILENAME));

  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      push_unique_path(&mut out, dir.join(STRATEGY_MODEL_FILENAME));
      push_unique_path(&mut out, dir.join("models").join(STRATEGY_MODEL_FILENAME));
      push_unique_path(&mut out, dir.join("resources").join("models").join(STRATEGY_MODEL_FILENAME));
    }
  }

  out
}

fn resolve_strategy_model_path(app: Option<&AppHandle>) -> Result<PathBuf, String> {
  for candidate in strategy_model_candidates(app) {
    if candidate.exists() {
      return Ok(candidate);
    }
  }
  Err(format!(
    "전략자문 모델 파일을 찾지 못했어요. {} 파일을 앱 번들 리소스 models 경로나 src-tauri/src/engine 폴더에 넣어주세요.",
    STRATEGY_MODEL_FILENAME
  ))
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

fn summarize_record_for_prompt(record: &RecordItem, index: usize) -> String {
  let actor_names = record_main_actor_names(record);
  let main_actor = actor_names.get(0).cloned().unwrap_or_else(|| record.actor.name.trim().to_string());
  let related = record
    .related
    .iter()
    .take(3)
    .map(format_actor_short)
    .collect::<Vec<_>>()
    .join(", ");
  let mut line = format!(
    "{}. [{}] {} | {} | {} | {}",
    index + 1,
    strategy_trim(record.ts.trim(), 32),
    strategy_trim(main_actor.trim(), 32),
    strategy_trim(record.store_type.trim(), 24),
    strategy_trim(record.place.trim(), 24),
    strategy_trim(record.summary.trim(), 240)
  );
  if !related.is_empty() {
    line.push_str(&format!(" | 관련: {}", strategy_trim(&related, 80)));
  }
  line
}

fn summarize_conversation(history: &[StrategyChatTurn]) -> String {
  if history.is_empty() {
    return "- 이전 대화 없음".to_string();
  }
  history
    .iter()
    .rev()
    .take(6)
    .collect::<Vec<_>>()
    .into_iter()
    .rev()
    .map(|turn| {
      let role = if turn.role.trim() == "user" { "사용자" } else { "어시스턴트" };
      format!("- {}: {}", role, strategy_trim(turn.content.trim(), 240))
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn build_strategy_system_prompt() -> String {
  [
    "- AI 언어모델의 이름은 \"CLOVA X\" 이며 네이버에서 만들었다.",
    "- 반드시 한국어로만 답하라.",
    "- 영어, 중국어, 일본어, 한자 사용 금지.",
    "- 역할: 학교 현장 분쟁·민원 대응을 돕는 전략자문 에이전트.",
    "- 답변은 교사가 바로 복사해 쓸 수 있게 실무적으로 작성하라.",
    "- 과도한 법률 단정은 피하고, 기록·증거·말의 톤·다음 행동 중심으로 답하라.",
    "- 가능하면 1) 상황판단 2) 지금 먼저 할 말 3) 바로 남길 기록 4) 다음 행동 순서로 답하라.",
  ].join("\n")
}

fn build_strategy_user_prompt(
  case_item: Option<&CaseItem>,
  records: &[RecordItem],
  message: &str,
  strategy_note: Option<&str>,
  conversation: &[StrategyChatTurn],
) -> String {
  let case_block = summarize_case_context(case_item);
  let records_block = if records.is_empty() {
    "- 연결된 증거 없음".to_string()
  } else {
    records
      .iter()
      .take(12)
      .enumerate()
      .map(|(idx, record)| summarize_record_for_prompt(record, idx))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let note_block = strategy_trim(strategy_note.unwrap_or("없음"), 800);
  let history_block = summarize_conversation(conversation);
  let question_block = strategy_trim(message.trim(), 600);

  format!(
    "[현재 사건 맥락]\n{}\n\n[전략 메모]\n{}\n\n[연결된 증거 요약]\n{}\n\n[직전 대화]\n{}\n\n[이번 요청]\n{}\n\n[응답 조건]\n- 한국어만 사용\n- 학교 현장에서 바로 쓰는 표현\n- 너무 긴 설명보다 핵심 위주\n- 필요한 경우 bullet 사용 가능\n- 사건에 없는 사실은 추정하지 말 것",
    case_block,
    note_block,
    records_block,
    history_block,
    question_block
  )
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
      || trimmed.starts_with("[전략 메모]")
      || trimmed.starts_with("[연결된 증거 요약]")
      || trimmed.starts_with("[직전 대화]")
      || trimmed.starts_with("[이번 요청]")
      || trimmed.starts_with("[응답 조건]")
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
      || trimmed.starts_with("- 답변은 교사가 바로")
      || trimmed.starts_with("- 과도한 법률 단정은")
      || trimmed.starts_with("- 가능하면 1) 상황판단")
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
  let out = out.trim();
  strategy_trim(out, 2400)
}

pub fn run_strategy_chat(
  app: Option<&AppHandle>,
  case_item: Option<&CaseItem>,
  records: &[RecordItem],
  message: &str,
  strategy_note: Option<&str>,
  conversation: &[StrategyChatTurn],
  opts: Option<StrategyChatOptions>,
) -> Result<StrategyChatRunResult, String> {
  let safe_message = message.trim();
  if safe_message.is_empty() {
    return Err("질문 내용이 비어 있어요.".to_string());
  }
  if records.is_empty() {
    return Err("전략자문에 연결된 증거가 없어요.".to_string());
  }
  emit_strategy_progress(app, "준비", format!("전략자문 요청을 받았어요. 연결된 증거 {}개를 확인 중이에요.", records.len()));

  let model_path = resolve_strategy_model_path(app)?;
  let runner = resolve_strategy_runner_path(app)?;
  let system_prompt = build_strategy_system_prompt();
  let user_prompt = build_strategy_user_prompt(case_item, records, safe_message, strategy_note, conversation);
  let max_tokens = opts.as_ref().and_then(|x| x.max_tokens).unwrap_or(320).clamp(64, 512);
  let n_ctx = opts.as_ref().and_then(|x| x.n_ctx).unwrap_or(2048).clamp(1024, 4096);
  let threads = opts.as_ref().and_then(|x| x.threads).unwrap_or(4).clamp(1, 8);
  let runner_name = runner.file_name().and_then(|x| x.to_str()).unwrap_or("llama-sidecar");
  let model_name = model_path.file_name().and_then(|x| x.to_str()).unwrap_or(STRATEGY_MODEL_FILENAME);
  emit_strategy_progress(app, "준비", format!("실행기 {} 와 모델 {} 를 찾았어요.", runner_name, model_name));
  emit_strategy_progress(app, "준비", format!("시스템 {}자 + 사용자 {}자, 컨텍스트 {}, 최대 토큰 {}로 실행해요.", system_prompt.chars().count(), user_prompt.chars().count(), n_ctx, max_tokens));

  let mut child = Command::new(&runner)
    .arg("-m")
    .arg(&model_path)
    .arg("-c")
    .arg(n_ctx.to_string())
    .arg("-n")
    .arg(max_tokens.to_string())
    .arg("-t")
    .arg(threads.to_string())
    .arg("--temp")
    .arg("0.15")
    .arg("--top-p")
    .arg("0.85")
    .arg("--repeat-penalty")
    .arg("1.12")
    .arg("--simple-io")
    .arg("--no-display-prompt")
    .arg("--no-show-timings")
    .arg("--single-turn")
    .arg("--no-warmup")
    .arg("--device")
    .arg("none")
    .arg("--n-gpu-layers")
    .arg("0")
    .arg("--color")
    .arg("off")
    .arg("--log-colors")
    .arg("off")
    .arg("--system-prompt")
    .arg(&system_prompt)
    .arg("-p")
    .arg(&user_prompt)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|e| format!(
      "내장 추론기 실행에 실패했어요. sidecar 포함 여부와 실행 권한을 확인해주세요. runner={} / 상세: {}",
      runner.display(),
      e
    ))?;
  emit_strategy_progress(app, "실행", "전략자문 추론기를 시작했어요. 콘솔 로그를 함께 흘려보낼게요.");

  let stdout = child.stdout.take().ok_or_else(|| "전략자문 표준출력을 연결하지 못했어요.".to_string())?;
  let stderr = child.stderr.take().ok_or_else(|| "전략자문 표준에러를 연결하지 못했어요.".to_string())?;

  let stdout_handle = thread::spawn(move || {
    let mut reader = BufReader::new(stdout);
    let mut bytes = Vec::<u8>::new();
    let _ = reader.read_to_end(&mut bytes);
    bytes
  });

  let app_for_stderr = app.map(|handle| handle.clone());
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
          if !trimmed.is_empty() {
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

  let started = Instant::now();
  let mut timed_out = false;
  let mut last_heartbeat = 0_u64;
  let status = loop {
    if let Some(status) = child.try_wait().map_err(|e| format!("전략자문 상태 확인에 실패했어요: {e}"))? {
      break status;
    }

    let elapsed = started.elapsed().as_secs();
    if elapsed >= STRATEGY_CHAT_TIMEOUT_SECS {
      timed_out = true;
      emit_strategy_progress(app, "오류", format!("{}초 동안 완료되지 않아 실행을 중단할게요.", STRATEGY_CHAT_TIMEOUT_SECS));
      let _ = child.kill();
      let status = child.wait().map_err(|e| format!("중단된 전략자문 프로세스를 정리하지 못했어요: {e}"))?;
      break status;
    }
    if elapsed >= last_heartbeat + 5 {
      last_heartbeat = elapsed;
      emit_strategy_progress(app, "대기", format!("모델 응답을 기다리는 중이에요. {}초 경과했어요.", elapsed));
    }
    thread::sleep(Duration::from_millis(200));
  };

  let stdout = String::from_utf8_lossy(&stdout_handle.join().unwrap_or_default()).to_string();
  let stderr = stderr_handle.join().unwrap_or_else(|_| "stderr 수집 스레드가 비정상 종료되었어요.".to_string());
  let answer = cleanup_strategy_output(&stdout);

  if timed_out {
    return Err(format!(
      "전략자문 응답이 {}초 안에 끝나지 않아 중단했어요. 콘솔에 찍힌 진행 로그와 마지막 모델 로그를 확인해주세요. 마지막 로그: {}",
      STRATEGY_CHAT_TIMEOUT_SECS,
      strategy_trim(stderr.trim(), 280)
    ));
  }

  if !status.success() && answer.is_empty() {
    return Err(format!(
      "모델 실행이 완료되지 않았어요. runner={} / stderr: {}",
      runner.display(),
      strategy_trim(stderr.trim(), 600)
    ));
  }
  if answer.is_empty() {
    return Err("모델이 빈 응답을 반환했어요. 번들된 모델/sidecar 상태를 확인해주세요.".to_string());
  }
  emit_strategy_progress(app, "완료", format!("응답 생성을 마쳤어요. 본문 길이 {}자예요.", answer.chars().count()));

  Ok(StrategyChatRunResult {
    answer,
    model_path: model_path.display().to_string(),
    runner: runner.display().to_string(),
    prompt_chars: system_prompt.chars().count() + user_prompt.chars().count(),
    records_used: records.len(),
  })
}
