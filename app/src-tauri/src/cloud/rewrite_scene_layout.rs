//! Local presentation candidate, evaluated separately from the cloud experiment.
//! This produces a candidate and provenance, never an input/delivery permission.
use super::{minimum, scene::{SceneSnapshot, WritingScene}, CloudError, RewriteResult};
use crate::core::{BaseMode, ModeSnapshot, SourceRevision};
use serde::Serialize;

pub const VERSION: &str = "scene-layout-v1";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutCandidate {
    pub draft: RewriteResult,
    pub applied: bool,
    pub reason: &'static str,
    pub source_sha256: String,
    pub mode_sha256: String,
    pub scene_sha256: String,
    pub input_sha256: String,
    pub output_sha256: String,
}

/// A narrowly specified formatting operation on complete, same-source text.
/// Callers must independently admit the input; this never supplies a semantic verdict.
pub fn candidate(mode: &ModeSnapshot, source: &SourceRevision, scene: &SceneSnapshot,
    input: &RewriteResult) -> Result<LayoutCandidate, CloudError> {
    super::validate_source(source)?;
    let fail = || CloudError::new("invalid_scene_layout", "rewrite", "场景排版来源不一致");
    if scene.session_id != source.session_id || scene.generation != source.generation
        || scene.target_id.is_empty() || scene.target_id.len() > 256 || scene.revision == 0
        || input.resolved_mode == BaseMode::Auto || input.blocks.len() != 1
        || input.blocks[0].source_refs != source.items.iter().map(|item|item.id.clone()).collect::<Vec<_>>() {
        return Err(fail());
    }
    let mut draft = input.clone();
    let (applied, reason) = if mode.base_mode != BaseMode::Auto || !mode.rules.trim().is_empty() {
        (false, "explicit_mode_or_rules")
    } else if scene.scene != WritingScene::Chat {
        (false, "other_scene")
    } else if protected_source(&source.text()) {
        (false, "source_requests_or_contains_literal_layout")
    } else if let Some(text) = unwrap_short_flat_list(&input.blocks[0].text) {
        draft.blocks[0].text = text;
        draft.resolved_mode = BaseMode::Light;
        (true, "chat_plain_sentences")
    } else {
        (false, "layout_not_supported")
    };
    Ok(LayoutCandidate { source_sha256: minimum::hash(source), mode_sha256: minimum::hash(mode),
        scene_sha256: minimum::hash(scene), input_sha256: minimum::hash(input), output_sha256: minimum::hash(&draft),
        draft, applied, reason })
}

fn protected_source(source: &str) -> bool {
    source.contains(['`', '"', '“', '”'])
        || ["列点", "列出", "列表", "编号", "清单", "逐字", "原样", "照抄", "第一", "第二", "第1", "第2"]
            .iter().any(|word|source.contains(word))
        || source.lines().any(|line| line.starts_with("1. ") || line.starts_with("- ") || line.starts_with("• "))
}

fn unwrap_short_flat_list(text: &str) -> Option<String> {
    if text.chars().count() > 400 || text.contains(['`', '"', '“', '”']) { return None; }
    let lines: Vec<_> = text.lines().collect();
    if !(2..=6).contains(&lines.len()) { return None; }
    let bullet = if lines[0].starts_with("- ") { Some("- ") } else if lines[0].starts_with("• ") { Some("• ") } else { None };
    let mut parts = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let number = format!("{}. ", index + 1);
        let sentence = line.strip_prefix(bullet.unwrap_or(&number))?;
        if sentence.trim() != sentence || !matches!(sentence.chars().last(), Some('。'|'.'|'！'|'!'|'？'|'?')) {
            return None;
        }
        parts.push(sentence);
    }
    // Preserve every sentence character and line boundary; remove only checked list markers.
    Some(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{OutputBlock, SourceItem};
    fn fixture() -> (ModeSnapshot, SourceRevision, SceneSnapshot, RewriteResult) {
        let mode = ModeSnapshot { mode_id: "auto".into(), name: "自动".into(), base_mode: BaseMode::Auto, rules: String::new(), revision: 1 };
        let source = SourceRevision { id: "source".into(), session_id: "session".into(), asr_attempt_id: "attempt".into(), generation: 1,
            provider: "synthetic".into(), model: "fixed".into(), adapter_version: "test".into(), complete: true, created_at: "now".into(),
            items: vec![SourceItem { id: "item".into(), provider_item_id: "0".into(), capture_order: 0,
                text: "先检查三个接口，再确认预算3500元。不要发送。".into(), is_final: true, start_ms: None, end_ms: None }] };
        let mut scene = SceneSnapshot::speech_only(&source); scene.scene = WritingScene::Chat;
        let input = RewriteResult { resolved_mode: BaseMode::Structure, blocks: vec![OutputBlock { id: "draft".into(),
            text: "1. 先检查三个接口，再确认预算3500元。\n2. 不要发送。".into(), source_refs: vec!["item".into()] }] };
        (mode, source, scene, input)
    }
    #[test]
    fn chat_candidate_preserves_sentences_numbers_negation_order_and_source_refs() {
        let (mode, source, scene, input) = fixture(); let out = candidate(&mode,&source,&scene,&input).unwrap();
        assert!(out.applied); assert_eq!(out.draft.blocks[0].text,"先检查三个接口，再确认预算3500元。\n不要发送。");
        assert_eq!(out.draft.blocks[0].source_refs,input.blocks[0].source_refs);
        assert_ne!(out.input_sha256,out.output_sha256); assert_eq!(input.resolved_mode,BaseMode::Structure);
    }
    #[test]
    fn explicit_mode_and_personal_rules_are_not_reformatted() {
        let (mut mode,source,scene,input)=fixture(); mode.base_mode=BaseMode::Structure;
        assert!(!candidate(&mode,&source,&scene,&input).unwrap().applied);
        mode.base_mode=BaseMode::Auto; mode.rules="保留编号".into();
        assert!(!candidate(&mode,&source,&scene,&input).unwrap().applied);
    }
    #[test]
    fn documents_and_technical_scenes_keep_their_layout() {
        let(mode,source,mut scene,input)=fixture();
        for kind in [WritingScene::Document,WritingScene::Technical,WritingScene::General] {
            scene.scene=kind; assert!(!candidate(&mode,&source,&scene,&input).unwrap().applied);
        }
    }
    #[test]
    fn literal_quotes_code_and_spoken_layout_requirements_are_preserved() {
        let(mode,mut source,scene,input)=fixture();
        for text in ["请逐字写出这些内容。", "请保留编号。", "第一步检查接口，第二步停止。", "命令是`test`。", "他说“不要发送”。", "1. 检查接口。"] {
            source.items[0].text=text.into(); assert!(!candidate(&mode,&source,&scene,&input).unwrap().applied);
        }
    }
    #[test]
    fn nested_incomplete_mixed_nonsequential_or_non_sentence_lists_are_left_intact() {
        for text in ["1. 检查。\n3. 停止。", "1. 标题：\n2. 停止。", "1. 检查。\n - 停止。", "1. 检查。\n- 停止。", "1. 检查。\n\n2. 停止。", "1. `code`.\n2. 停止。"] {
            assert!(unwrap_short_flat_list(text).is_none());
        }
    }
    #[test]
    fn stale_session_and_foreign_source_refs_are_rejected() {
        let(mode,source,mut scene,mut input)=fixture(); scene.generation+=1;
        assert!(candidate(&mode,&source,&scene,&input).is_err()); scene.generation-=1;
        input.blocks[0].source_refs=vec!["foreign".into()]; assert!(candidate(&mode,&source,&scene,&input).is_err());
    }
}
