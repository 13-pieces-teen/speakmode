//! A frozen, local scene for initial dictation. No cloud request or host text.
use super::*;
use crate::cloud::rewrite::{scene::{SceneSnapshot,WritingScene}, scene_layout};

fn hash(value: &impl Serialize) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).expect("typed scene identity serializes")))
}

pub(super) struct FrozenScene {
    target: platform::delivery::TargetSnapshot,
    source_hash: String,
    mode_hash: String,
    scene: SceneSnapshot,
    pub kind: AppWritingScene,
}

pub(super) fn freeze(session: &ActiveSession, source: &SourceRevision) -> Option<FrozenScene> {
    if !session.prefs.app_writing.enabled || session.mode.base_mode != BaseMode::Auto || !session.mode.rules.trim().is_empty()
        || session.reason != CreationReason::NormalInitial || session.target_capture_pending
        || session.cancel.is_cancelled() || session.original_delivery_revoked { return None; }
    let target = session.target.as_ref()?;
    let kind = session.prefs.app_writing.scene_for(session.target_application.as_deref().unwrap_or(""));
    let scene = SceneSnapshot { session_id: session.id.clone(), generation: session.generation,
        target_id: hash(target), revision: session.prefs.version,
        scene: match kind { AppWritingScene::General=>WritingScene::General, AppWritingScene::Chat=>WritingScene::Chat,
            AppWritingScene::Document=>WritingScene::Document, AppWritingScene::Technical=>WritingScene::Technical },
        use_nearby_text:false,nearby_text:None };
    Some(FrozenScene { target:target.clone(), source_hash:hash(source), mode_hash:hash(&session.mode), scene, kind })
}

/// Caller has already received an accepted semantic result. Return that result
/// unchanged if any provenance/format constraint fails; never edit a fallback.
pub(super) fn present(session: &ActiveSession, frozen: &FrozenScene, source: &SourceRevision,
    mode: &ModeSnapshot, draft: rewrite::RewriteResult) -> rewrite::RewriteResult {
    if session.id != source.session_id || session.generation != source.generation
        || session.cancel.is_cancelled() || session.original_delivery_revoked
        || session.target.as_ref() != Some(&frozen.target) || session.source.as_ref() != Some(source)
        || hash(source) != frozen.source_hash || hash(mode) != frozen.mode_hash
        || hash(&session.mode) != frozen.mode_hash { return draft; }
    // Preserve spoken formatting requests beyond the frozen experimental
    // renderer's original vocabulary. A false positive only keeps the draft.
    let text = source.text();
    if ["分点", "分条", "分项", "逐项", "按点", "项目符号"].iter().any(|word| text.contains(word)) {
        return draft;
    }
    let english = text.to_ascii_lowercase();
    let words: Vec<_> = english.split(|c: char| !c.is_ascii_alphabetic()).filter(|s| !s.is_empty()).collect();
    if words.iter().any(|word| matches!(*word, "bullet" | "bullets" | "numbered" | "numbering" | "list"))
        || words.windows(3).any(|phrase| phrase == ["step", "by", "step"]) { return draft; }
    scene_layout::candidate(mode,source,&frozen.scene,&draft).map(|candidate|candidate.draft).unwrap_or(draft)
}
