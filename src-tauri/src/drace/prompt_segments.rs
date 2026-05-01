use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum PromptSegmentKind {
  StaticSystemPrefix,
  StaticModeTemplate,
  StaticStageInstruction,
  StaticOutputFormat,
  DynamicCaseContext,
  DynamicEvidencePacket,
  DynamicLegalRefs,
  DynamicConversation,
  DynamicUserMessage,
  DynamicDraftArtifacts,
}

impl PromptSegmentKind {
  pub fn order_rank(&self) -> u8 {
    match self {
      PromptSegmentKind::StaticSystemPrefix => 0,
      PromptSegmentKind::StaticModeTemplate => 1,
      PromptSegmentKind::StaticStageInstruction => 2,
      PromptSegmentKind::StaticOutputFormat => 3,
      PromptSegmentKind::DynamicCaseContext => 10,
      PromptSegmentKind::DynamicEvidencePacket => 11,
      PromptSegmentKind::DynamicLegalRefs => 12,
      PromptSegmentKind::DynamicConversation => 13,
      PromptSegmentKind::DynamicUserMessage => 14,
      PromptSegmentKind::DynamicDraftArtifacts => 15,
    }
  }

  pub fn label(&self) -> &'static str {
    match self {
      PromptSegmentKind::StaticSystemPrefix => "StaticSystemPrefix",
      PromptSegmentKind::StaticModeTemplate => "StaticModeTemplate",
      PromptSegmentKind::StaticStageInstruction => "StaticStageInstruction",
      PromptSegmentKind::StaticOutputFormat => "StaticOutputFormat",
      PromptSegmentKind::DynamicCaseContext => "DynamicCaseContext",
      PromptSegmentKind::DynamicEvidencePacket => "DynamicEvidencePacket",
      PromptSegmentKind::DynamicLegalRefs => "DynamicLegalRefs",
      PromptSegmentKind::DynamicConversation => "DynamicConversation",
      PromptSegmentKind::DynamicUserMessage => "DynamicUserMessage",
      PromptSegmentKind::DynamicDraftArtifacts => "DynamicDraftArtifacts",
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSegment {
  pub kind: PromptSegmentKind,
  pub stable_id: Option<String>,
  pub content_hash: u64,
  pub text: String,
  pub is_static: bool,
}

pub fn fast_hash64(value: &str) -> u64 {
  let mut hasher = DefaultHasher::new();
  value.hash(&mut hasher);
  hasher.finish()
}

fn normalize_segment_text(value: &str) -> String {
  value
    .replace("\r\n", "\n")
    .replace('\r', "\n")
    .trim()
    .to_string()
}

impl PromptSegment {
  pub fn static_segment(kind: PromptSegmentKind, stable_id: impl Into<String>, text: impl Into<String>) -> Self {
    let stable_id = stable_id.into();
    let text = normalize_segment_text(&text.into());
    let content_hash = fast_hash64(&text);
    Self {
      kind,
      stable_id: Some(stable_id),
      content_hash,
      text,
      is_static: true,
    }
  }

  pub fn dynamic_segment(kind: PromptSegmentKind, text: impl Into<String>) -> Self {
    let text = normalize_segment_text(&text.into());
    let content_hash = fast_hash64(&text);
    Self {
      kind,
      stable_id: None,
      content_hash,
      text,
      is_static: false,
    }
  }
}

pub fn render_prompt_segments(segments: &[PromptSegment]) -> String {
  segments
    .iter()
    .map(|segment| segment.text.trim())
    .filter(|text| !text.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}
