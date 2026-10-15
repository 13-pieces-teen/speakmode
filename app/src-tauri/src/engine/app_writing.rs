//! Freeze purpose and bounded host hints before generation; never restyle an accepted draft.
use super::*;
use crate::core::writing_context::{InputContext,WritingContext,selected_scene,input_scene};

pub(super) struct CapturedInput {
    pub target: platform::delivery::TargetSnapshot,
    pub context: InputContext,
    pub captured_at: Instant,
}

pub(super) fn freeze(session: &ActiveSession) -> Option<WritingContext> {
    if session.cancel.is_cancelled() { return None; }
    let explicit=selected_scene(&session.mode);
    let automatic=session.mode.mode_id=="auto" && session.mode.rules.is_empty();
    let language=session.prefs.app_writing.output_language;
    if explicit.is_none() && !automatic && !session.prefs.app_writing.input_context_enabled && language==DraftLanguage::AsSpoken { return None; }
    let target_ready=!session.target_capture_pending && !session.original_delivery_revoked && session.reason==CreationReason::NormalInitial;
    let application=session.target_application.as_deref().unwrap_or("");
    let input=if target_ready && session.prefs.app_writing.permits_input_context(application) {
        session.input_context.as_ref().filter(|input|
            session.target.as_ref()==Some(&input.target) && input.captured_at.elapsed()<Duration::from_secs(3)
            && input.context.validate().is_ok()).map(|input|input.context.clone())
    } else {None};
    let hinted=input.as_ref().map(input_scene).unwrap_or_default();
    let scene=explicit.unwrap_or_else(|| {
        if !automatic || !session.prefs.app_writing.enabled { return AppWritingScene::General; }
        if hinted!=AppWritingScene::General {hinted}
        else if target_ready {session.prefs.app_writing.scene_for(application)}
        else {AppWritingScene::General}
    });
    Some(WritingContext {scene,language,input})
}
