//! Actual coordinator + SQLite, with parsed provider fixtures and owned IO seams.
use super::*;
const LIST: &str = "1. 如果会议未确认，不要订机票。\n2. 先保留行程。";
const CHAT: &str = "如果会议未确认，不要订机票。\n先保留行程。";
fn draft(source: &SourceRevision) -> rewrite::RewriteResult {
    rewrite::RewriteResult { resolved_mode:BaseMode::Structure,blocks:vec![OutputBlock { id:"draft".into(),text:LIST.into(),
        source_refs:source.items.iter().map(|i|i.id.clone()).collect() }] }
}
fn setup(app: &str) -> Arc<AppHub> {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);enable_fixture_delivery(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().target_application=Some(app.into());
    checkpoint(&h,"s");h
}
async fn complete(h: &Arc<AppHub>) {
    h.finish_rewrite_with_policy("s".into(),7,|_,_,source,_,_|async move { Ok(draft(&source)) },
        None::<RewriteWaitPolicy>,|hub,_,_| { hub.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true; }).await;
}

#[test]
fn old_profiles_load_with_bounded_local_scene_defaults() {
    let mut legacy=serde_json::to_value(Preferences::default()).unwrap();legacy.as_object_mut().unwrap().remove("appWriting");
    let decoded:Preferences=serde_json::from_value(legacy).unwrap();
    assert!(decoded.app_writing.enabled);assert!(decoded.app_writing.overrides.is_empty());
    let h=hub();let mut prefs=h.db.preferences().unwrap();
    prefs.app_writing.overrides.push(AppWritingOverride {application:"msedge.exe".into(),scene:AppWritingScene::Chat});
    let saved=h.db.save_preferences(prefs,1).unwrap();assert_eq!(h.db.preferences().unwrap(),saved);
    assert!(h.db.save_preferences(saved,1).is_err());
}

#[tokio::test]
async fn app_preferences_roundtrip_conflict_validation_and_reset_use_existing_rpc() {
    let h=hub();
    let patch=json!({"appWriting":{"enabled":true,"overrides":[{"application":"msedge.exe","scene":"chat"}]}});
    command(&h,"preferences.update",json!({"expectedVersion":1,"requestId":"scene-save","patch":patch})).await.unwrap();
    assert_eq!(h.db.preferences().unwrap().app_writing.scene_for("msedge.exe"),AppWritingScene::Chat);
    assert!(command(&h,"preferences.update",json!({"expectedVersion":1,"requestId":"scene-stale","patch":patch})).await.is_err());
    let bad=json!({"appWriting":{"enabled":true,"overrides":[{"application":"C:/private/app.exe","scene":"chat"}]}});
    let error=command(&h,"preferences.update",json!({"expectedVersion":2,"requestId":"scene-invalid","patch":bad})).await.unwrap_err();
    assert_eq!(error.code,"invalid_app_writing");assert_eq!(h.db.preferences().unwrap().version,2);
    command(&h,"preferences.reset",json!({"expectedVersion":2,"requestId":"scene-reset"})).await.unwrap();
    assert_eq!(h.db.preferences().unwrap().app_writing,AppWritingPreferences::default());
}

#[tokio::test]
async fn chat_presentation_is_saved_and_delivered_once_with_immutable_source() {
    let h=setup("WeChat.exe");
    let copies=Arc::new(Mutex::new(Vec::new()));let sink=copies.clone();
    h.finish_rewrite_with_policy("s".into(),7,|_,_,source,_,_|async move {Ok(draft(&source))},None::<RewriteWaitPolicy>,
        move|hub,id,generation|hub.initial_delivery_with(id,generation,|_|true,|permit,target,text,gate|
            platform::delivery::dispatch_with_test_operations(permit,target,text,gate,|text|{sink.lock().unwrap().push(text.to_owned());true},||4))).await;
    let detail=h.db.get_result("s").unwrap();let output=detail.output.unwrap();
    assert_eq!(detail.source,source("s"));assert_eq!(output.text(),CHAT);assert_eq!(output.resolved_mode,BaseMode::Light);
    assert_eq!(output.mode_snapshot.base_mode,BaseMode::Auto);assert_eq!(output.blocks[0].source_refs,draft(&source("s")).blocks[0].source_refs);
    assert_eq!(view(&h).writing_scene,Some(AppWritingScene::Chat));assert_eq!(*copies.lock().unwrap(),vec![CHAT]);
    h.initial_delivery_with("s",7,|_|panic!("already delivered"),|_,_,_,_|panic!("already delivered"));
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),1);
}

#[tokio::test]
async fn explicit_modes_rules_disabled_preferences_and_other_scenes_keep_the_draft() {
    for case in ["structure","light","rules","disabled","general","document","technical"] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);enable_fixture_delivery(&h);
        {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.target_application=Some("wechat.exe".into());
            match case {
                "structure"=>s.mode=h.db.mode_snapshot("structure",None).unwrap(),
                "light"=>s.mode=h.db.mode_snapshot("light",None).unwrap(),
                "rules"=>{
                    let personal=h.db.save_mode(ModeDraft{id:None,name:"保留列表".into(),base_mode:BaseMode::Structure,rules:"保留编号".into(),expected_revision:None}).unwrap();
                    s.mode=h.db.mode_snapshot(&personal.id,Some(personal.revision)).unwrap();
                },
                "disabled"=>s.prefs.app_writing.enabled=false,
                "general"=>s.target_application=Some("chrome.exe".into()),
                "document"=>s.target_application=Some("winword.exe".into()),
                "technical"=>s.target_application=Some("code.exe".into()),_=>unreachable!(),
            }}
        checkpoint(&h,"s");
        // The Light fixture uses an appropriate resolved mode; this seam is
        // after cloud semantic acceptance and does not exercise model prompting.
        h.finish_rewrite_with_policy("s".into(),7,|_,mode,source,_,_|async move {
            let mut value=draft(&source);if mode.base_mode==BaseMode::Light {value.resolved_mode=BaseMode::Light;value.blocks[0].text=CHAT.into();} Ok(value)
        },None::<RewriteWaitPolicy>,|h,_,_|{h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;}).await;
        assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),if case=="light"{CHAT}else{LIST},"{case}");
    }
}

#[tokio::test]
async fn spoken_layout_requests_keep_lists_without_matching_english_word_fragments() {
    for (spoken, list, expected) in [
        ("请分点写：检查预算。确认行程。", "1. 检查预算。\n2. 确认行程。", "1. 检查预算。\n2. 确认行程。"),
        ("请分条写：检查预算。确认行程。", "1. 检查预算。\n2. 确认行程。", "1. 检查预算。\n2. 确认行程。"),
        ("用项目符号写：检查预算。确认行程。", "- 检查预算。\n- 确认行程。", "- 检查预算。\n- 确认行程。"),
        ("Use BULLETS: Check the budget. Confirm the trip.", "- Check the budget.\n- Confirm the trip.", "- Check the budget.\n- Confirm the trip."),
        ("Use a numbered list: Check the budget. Confirm the trip.", "1. Check the budget.\n2. Confirm the trip.", "1. Check the budget.\n2. Confirm the trip."),
        ("Write step-by-step: Check the budget. Confirm the trip.", "1. Check the budget.\n2. Confirm the trip.", "1. Check the budget.\n2. Confirm the trip."),
        ("Keep listening. Confirm the trip.", "1. Keep listening.\n2. Confirm the trip.", "Keep listening.\nConfirm the trip."),
    ] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);enable_fixture_delivery(&h);
        h.inner.lock().unwrap().active.as_mut().unwrap().target_application=Some("wechat.exe".into());
        let mut input=source("s");input.items.truncate(1);input.items[0].text=spoken.into();
        h.checkpoint("s",7,input.clone()).unwrap();
        h.finish_rewrite_with_policy("s".into(),7,|_,_,source,_,_|async move {
            let mut value=draft(&source);value.blocks[0].text=list.into();Ok(value)
        },None::<RewriteWaitPolicy>,|h,_,_|{h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;}).await;
        let detail=h.db.get_result("s").unwrap();
        assert_eq!(detail.output.unwrap().text(),expected,"{spoken}");assert_eq!(detail.source,input);
    }
}

#[tokio::test]
async fn recording_preferences_are_frozen_despite_later_global_changes() {
    let h=hub();let mut prefs=h.db.preferences().unwrap();
    prefs.app_writing.overrides.push(AppWritingOverride{application:"chrome.exe".into(),scene:AppWritingScene::Chat});
    h.db.save_preferences(prefs,1).unwrap();
    install(&h,"s",SessionPhase::FinalizingAsr,true);enable_fixture_delivery(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().target_application=Some("chrome.exe".into());
    let mut changed=h.db.preferences().unwrap();changed.app_writing.enabled=false;h.db.save_preferences(changed,2).unwrap();
    checkpoint(&h,"s");complete(&h).await;
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),CHAT);
}

#[tokio::test]
async fn stop_target_application_replaces_any_recording_time_app_hint() {
    let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().target_application=Some("winword.exe".into());
    assert!(h.begin_stop_target(&identity("s"),Instant::now()).unwrap().is_some());
    h.resolve_stop_target_with_application("s",7,||Ok(Some(delivery_target_fixture())),|_|Some("wechat.exe".into()));
    checkpoint(&h,"s");complete(&h).await;
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),CHAT);
}

#[tokio::test]
async fn late_application_capture_cannot_restyle_a_started_rewrite() {
    let h=setup("wechat.exe");
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.target_capture_pending=true;s.target=None;s.target_application=None;}
    let worker=h.clone();
    h.finish_rewrite_with_policy("s".into(),7,move|_,_,source,_,_|async move {
        worker.resolve_stop_target_with_application("s",7,||Ok(Some(delivery_target_fixture())),|_|Some("wechat.exe".into()));
        Ok(draft(&source))
    },None::<RewriteWaitPolicy>,|h,_,_|{h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;}).await;
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),LIST);
    assert!(view(&h).writing_scene.is_none());
}

#[tokio::test]
async fn changed_bound_target_cannot_receive_previous_scene_policy() {
    let h=setup("wechat.exe");let worker=h.clone();
    h.finish_rewrite_with_policy("s".into(),7,move|_,_,source,_,_|async move {
        worker.inner.lock().unwrap().active.as_mut().unwrap().target.as_mut().unwrap().selection=Some((5,5));
        Ok(draft(&source))
    },None::<RewriteWaitPolicy>,|h,_,_|{h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;}).await;
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),LIST);
}

#[tokio::test]
async fn preview_can_use_stop_scene_without_automatic_delivery() {
    let h=hub();install(&h,"s",SessionPhase::Recording,true);
    h.inner.lock().unwrap().active.as_mut().unwrap().prefs.delivery_mode=DeliveryMode::Preview;
    assert!(h.begin_stop_target(&identity("s"),Instant::now()).unwrap().is_some());
    h.resolve_stop_target_with_application("s",7,||Ok(Some(delivery_target_fixture())),|_|Some("wechat.exe".into()));
    checkpoint(&h,"s");
    h.finish_rewrite_with_policy("s".into(),7,|_,_,source,_,_|async move{Ok(draft(&source))},None::<RewriteWaitPolicy>,
        |h,id,generation|h.initial_delivery_with(id,generation,|_|panic!("preview must not inspect for delivery"),|_,_,_,_|panic!("preview must not paste"))).await;
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),CHAT);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}

#[tokio::test]
async fn service_failure_and_explicit_retry_never_apply_initial_scene_cleanup() {
    let h=setup("wechat.exe");
    h.finish_rewrite_with_policy("s".into(),7,|_,_,_,_,_|async move{Err(CloudError::new("rewrite_timeout","rewrite","fixture"))},
        None::<RewriteWaitPolicy>,|h,_,_|{h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;}).await;
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),source("s").text());
    let h=setup("wechat.exe");h.inner.lock().unwrap().active.as_mut().unwrap().reason=CreationReason::Retry;
    complete(&h).await;assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),LIST);
}

#[tokio::test]
async fn cancellation_after_generation_cannot_commit_scene_output() {
    let h=setup("wechat.exe");let worker=h.clone();
    h.finish_rewrite_with_policy("s".into(),7,move|_,_,source,_,_|async move {
        worker.inner.lock().unwrap().active.as_ref().unwrap().cancel.cancel();Ok(draft(&source))
    },
        None::<RewriteWaitPolicy>,|_,_,_|panic!("cancelled result must not deliver")).await;
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),0);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}
