use super::cache_manager::HybridStage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DraftProviderState {
  pub stage: String,
  pub model_id: String,
  pub structured_json: bool,
  pub prompt_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DraftProposal {
  pub provider_name: String,
  pub token_ids: Vec<u32>,
  pub rendered_fragments: Vec<String>,
}

pub trait DraftProvider {
  fn name(&self) -> &'static str;
  fn propose(&self, state: &DraftProviderState, max_draft_tokens: usize) -> Vec<u32>;
  fn rendered_fragments(&self, _state: &DraftProviderState) -> Vec<String> {
    Vec::new()
  }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopDraftProvider;

impl DraftProvider for NoopDraftProvider {
  fn name(&self) -> &'static str {
    "noop"
  }

  fn propose(&self, _state: &DraftProviderState, _max_draft_tokens: usize) -> Vec<u32> {
    Vec::new()
  }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TemplateDraftProvider;

impl TemplateDraftProvider {
  fn stage_template(stage: &str) -> (String, Vec<String>) {
    match stage {
      "record_review" => (
        "{\"summary\":\"".to_string(),
        vec![
          "{\"summary\":\"".to_string(),
          "\"actors\":[".to_string(),
          "\"timeline\":[".to_string(),
          "\"issues\":[".to_string(),
          "\"evidence\":[".to_string(),
          "\"recommended_questions\":[".to_string(),
        ],
      ),
      "record_synthesis" | "record_main" | "record_fill" => (
        "[기록 기본정보]\n- 기록 시각: ".to_string(),
        vec![
          "[기록 기본정보]".to_string(),
          "- 기록 시각: ".to_string(),
          "- 주체: ".to_string(),
          "- 상대방: ".to_string(),
          "- 위치/채널: ".to_string(),
          "- 자료 형태: ".to_string(),
          "[상황 요약]".to_string(),
          "[배경 흐름]".to_string(),
          "[핵심 포인트]".to_string(),
          "[관련 자료]".to_string(),
          "[내 대응 메모]".to_string(),
          "[추가 메모]".to_string(),
        ],
      ),
      "general_synthesis" => (
        "지금은 ".to_string(),
        vec![
          "지금은 ".to_string(),
          "우선 ".to_string(),
          "먼저 ".to_string(),
          "바로 정리하면 ".to_string(),
          "지금 바로 할 일은 ".to_string(),
        ],
      ),
      _ => (
        "{\"summary\":\"".to_string(),
        vec![
          "{\"summary\":\"".to_string(),
          "\"actors\":[".to_string(),
          "\"timeline\":[".to_string(),
          "\"issues\":[".to_string(),
          "\"evidence\":[".to_string(),
          "\"recommended_questions\":[".to_string(),
        ],
      ),
    }
  }

  fn pseudo_tokenize(fragment: &str, max_draft_tokens: usize) -> Vec<u32> {
    fragment
      .chars()
      .take(max_draft_tokens)
      .map(|ch| ch as u32)
      .collect::<Vec<_>>()
  }

  pub fn proposal_for_stage(stage: HybridStage, model_id: &str, max_draft_tokens: usize) -> DraftProposal {
    let state = DraftProviderState {
      stage: stage.as_str().to_string(),
      model_id: model_id.to_string(),
      structured_json: true,
      prompt_hash: None,
    };
    let provider = TemplateDraftProvider;
    DraftProposal {
      provider_name: provider.name().to_string(),
      token_ids: provider.propose(&state, max_draft_tokens),
      rendered_fragments: provider.rendered_fragments(&state),
    }
  }
}

impl DraftProvider for TemplateDraftProvider {
  fn name(&self) -> &'static str {
    "template"
  }

  fn propose(&self, state: &DraftProviderState, max_draft_tokens: usize) -> Vec<u32> {
    let (proposal_prefix, _) = Self::stage_template(&state.stage);
    Self::pseudo_tokenize(&proposal_prefix, max_draft_tokens)
  }

  fn rendered_fragments(&self, state: &DraftProviderState) -> Vec<String> {
    let (_, rendered_fragments) = Self::stage_template(&state.stage);
    rendered_fragments
  }
}
