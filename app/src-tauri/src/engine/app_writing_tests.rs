//! Real coordinator + SQLite with provider and native context seams.
use super::*;
use crate::core::writing_context::{InputContext,WritingContext};
const PROSE:&str="如果会议未确认，不要订机票。先保留行程。";
fn draft(source:&SourceRevision)->rewrite::RewriteResult {
    rewrite::RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![OutputBlock{id:"draft".into(),text:PROSE.into(),source_refs:source.items.iter().map(|i|i.id.clone()).collect()}]}
}
fn setup(mode:&str,app:&str,label:&str)->Arc<AppHub>{
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);enable_fixture_delivery(&h);
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();
        s.mode=h.db.mode_snapshot(mode,None).unwrap();s.target_application=Some(app.into());
        if !label.is_empty(){s.prefs.app_writing.input_context_enabled=true;
            s.input_context=Some(app_writing::CapturedInput{target:s.target.clone().unwrap(),captured_at:Instant::now(),
                context:InputContext{control_label:label.into(),before_text:"LOCAL_HOST_SENTINEL".into(),..Default::default()}});}
    }
    checkpoint(&h,"s");h
}
async fn complete(h:&Arc<AppHub>,check:impl FnOnce(Option<WritingContext>)){
    h.finish_rewrite_with_context_policy("s".into(),7,|_,_,source,_,_,context|{
        check(context);async move {Ok(draft(&source))}
    },None::<RewriteWaitPolicy>,|h,_,_|{h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;}).await;
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
async fn same_browser_fields_select_distinct_scenes_with_same_source(){
    for (label,scene) in [("Send a message",AppWritingScene::Chat),("Document body",AppWritingScene::Document),("Email body",AppWritingScene::Email)] {
        let h=setup("auto","msedge.exe",label);
        complete(&h,|context|{let context=context.unwrap();assert_eq!(context.scene,scene);assert_eq!(context.input.unwrap().control_label,label);}).await;
        let detail=h.db.get_result("s").unwrap();assert_eq!(detail.source,source("s"));
        let output=detail.output.unwrap();assert_eq!(output.text(),PROSE);
        let receipt=output.writing_receipt.as_ref().unwrap();assert_eq!(receipt.scene,scene);assert!(receipt.context_used);
        assert!(!serde_json::to_string(&output).unwrap().contains("LOCAL_HOST_SENTINEL"));
        assert!(h.inner.lock().unwrap().active.as_ref().unwrap().input_context.is_none());
    }
}
#[tokio::test]
async fn explicit_scene_beats_host_and_language_is_frozen(){
    let h=setup("document","msedge.exe","Send a message");
    h.inner.lock().unwrap().active.as_mut().unwrap().prefs.app_writing.output_language=DraftLanguage::English;
    complete(&h,|context|{let context=context.unwrap();assert_eq!(context.scene,AppWritingScene::Document);assert_eq!(context.language,DraftLanguage::English);}).await;
    assert_eq!(view(&h).writing_language,DraftLanguage::English);
}
#[tokio::test]
async fn unknown_missing_or_disabled_context_keeps_speech_only_scene(){
    for condition in ["none","stale","target","excluded","disabled"] {
        let h=setup("auto","msedge.exe","Document body");
        {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();match condition {
            "none"=>s.input_context=None,
            "stale"=>s.input_context.as_mut().unwrap().captured_at=Instant::now()-Duration::from_secs(4),
            "target"=>s.target.as_mut().unwrap().selection=Some((9,9)),
            "excluded"=>s.prefs.app_writing.excluded_applications.push("MSEdge.exe".into()),
            "disabled"=>s.prefs.app_writing.input_context_enabled=false,_=>unreachable!(),}}
        complete(&h,|context|{let context=context.unwrap();assert!(context.input.is_none(),"{condition}");assert_eq!(context.scene,AppWritingScene::General);}).await;
        assert!(!h.db.get_result("s").unwrap().output.unwrap().writing_receipt.unwrap().context_used);
    }
}
#[tokio::test]
async fn custom_rules_and_legacy_selection_are_preserved(){
    let h=setup("structure","msedge.exe","");
    complete(&h,|context|assert!(context.is_none())).await;
    // The fake Light provider result is rejected by storage for Structure;
    // the ordinary legacy contract remains strict.
    assert!(h.db.get_result("s").unwrap().output.is_none());
    let h=setup("light","msedge.exe","");complete(&h,|context|assert!(context.is_none())).await;
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().mode_snapshot.mode_id,"light");
}
#[test]
fn excluded_unknown_or_disabled_apps_never_read_host_text(){
    for case in ["excluded","unknown","disabled"] {
        let h=hub();install(&h,"s",SessionPhase::Recording,true);
        {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.prefs.app_writing.input_context_enabled=case!="disabled";
            if case=="excluded"{s.prefs.app_writing.excluded_applications.push("msedge.exe".into());}}
        h.begin_stop_target(&identity("s"),Instant::now()).unwrap();
        h.resolve_stop_target_with_context("s",7,||Ok(Some(delivery_target_fixture())),|_|if case=="unknown"{None}else{Some("msedge.exe".into())},|_|panic!("must not read {case}"));
        let i=h.inner.lock().unwrap();assert!(i.active.as_ref().unwrap().input_context.is_none());assert!(i.active.as_ref().unwrap().target.is_some());
    }
}
#[test]
fn optional_context_failure_keeps_valid_delivery_target(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);
    h.inner.lock().unwrap().active.as_mut().unwrap().prefs.app_writing.input_context_enabled=true;
    h.begin_stop_target(&identity("s"),Instant::now()).unwrap();
    h.resolve_stop_target_with_context("s",7,||Ok(Some(delivery_target_fixture())),|_|Some("msedge.exe".into()),|_|Err(platform::PlatformError::TargetCapture(platform::TargetCaptureFailure::TargetInspectionTimeout)));
    let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert!(s.target.is_some());assert!(s.input_context.is_none());assert!(!s.target_capture_pending);
    assert_eq!(s.input_context_status,InputContextStatus::Unavailable);
}
#[tokio::test]
async fn late_capture_cannot_change_already_frozen_cloud_context(){
    let h=setup("auto","msedge.exe","");let worker=h.clone();
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.target_capture_pending=true;s.target=None;s.target_application=None;}
    h.finish_rewrite_with_context_policy("s".into(),7,move|_,_,source,_,_,context|async move {
        assert_eq!(context.unwrap().scene,AppWritingScene::General);
        worker.resolve_stop_target_with_context("s",7,||Ok(Some(delivery_target_fixture())),|_|Some("wechat.exe".into()),|_|panic!("disabled"));
        Ok(draft(&source))
    },None::<RewriteWaitPolicy>,|h,_,_|h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true).await;
    assert_eq!(view(&h).writing_scene,Some(AppWritingScene::General));
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().input_context.is_none());
}
#[tokio::test]
async fn bounded_capture_grace_is_inside_rewrite_deadline(){
    let h=setup("auto","msedge.exe","");
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.target_capture_pending=true;s.prefs.app_writing.input_context_enabled=true;}
    let start=Instant::now();
    h.finish_rewrite_with_context_policy("s".into(),7,|_,_,_,_,execution,context|async move {
        assert!(context.unwrap().input.is_none());assert!(execution.deadline<=tokio::time::Instant::now()+Duration::from_millis(20));
        Err(CloudError::new("rewrite_timeout","rewrite","fixture"))
    },RewriteWaitPolicy{deadline:Duration::from_millis(30)},|h,_,_|h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true).await;
    assert!(start.elapsed()<Duration::from_secs(1));
    super::rewrite_fallback_tests::assert_unfinished(&h);
}
#[tokio::test]
async fn cancellation_never_commits_scene_or_retains_host_text(){
    let h=setup("auto","msedge.exe","Document body");let worker=h.clone();
    h.finish_rewrite_with_context_policy("s".into(),7,move|_,_,source,_,_,_|async move {
        worker.inner.lock().unwrap().active.as_ref().unwrap().cancel.cancel();Ok(draft(&source))
    },None::<RewriteWaitPolicy>,|_,_,_|panic!("cancelled result cannot deliver")).await;
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),0);
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().input_context.is_none());
}
#[tokio::test]
async fn accepted_scene_is_stored_and_dispatched_once(){
    let h=setup("chat","msedge.exe","Send a message");let copies=Arc::new(Mutex::new(Vec::new()));let sink=copies.clone();
    h.finish_rewrite_with_context_policy("s".into(),7,|_,_,source,_,_,_|async move{Ok(draft(&source))},None::<RewriteWaitPolicy>,move|h,id,generation|
        h.initial_delivery_with(id,generation,|_|true,|permit,target,text,gate|
            platform::delivery::dispatch_with_test_operations(permit,target,text,gate,|text|{sink.lock().unwrap().push(text.to_owned());true},||4))).await;
    assert_eq!(*copies.lock().unwrap(),vec![PROSE]);
    h.initial_delivery_with("s",7,|_|panic!("must not inspect twice"),|_,_,_,_|panic!("must not dispatch twice"));
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),1);
}

#[test]
fn failure_clears_host_text_and_late_capture_cannot_restore_it(){
    for before_capture in [true,false] {
        let h=setup("auto","msedge.exe","Document body");
        if before_capture {
            let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.input_context=None;s.target_capture_pending=true;
        }
        h.fail("s",7,ApiError::new("asr_disconnected","asr","fixture"),true);
        if before_capture {
            h.resolve_stop_target_with_context("s",7,||Ok(Some(delivery_target_fixture())),|_|Some("msedge.exe".into()),|_|panic!("failed session cannot read host text"));
        }
        let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();
        assert!(s.input_context.is_none());assert!(s.input_context_frozen);assert_eq!(s.input_context_status,InputContextStatus::Unavailable);
    }
}

#[tokio::test]
async fn recovered_recording_preserves_frozen_language_without_original_host(){
    let h=setup("document","msedge.exe","Send a message");
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.reason=CreationReason::Retry;s.original_delivery_revoked=true;s.prefs.app_writing.output_language=DraftLanguage::English;}
    complete(&h,|context|{let context=context.unwrap();assert_eq!(context.scene,AppWritingScene::Document);assert_eq!(context.language,DraftLanguage::English);assert!(context.input.is_none());}).await;
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().writing_receipt.unwrap().language,DraftLanguage::English);
}

#[tokio::test]
async fn default_application_matching_gets_same_bounded_grace_without_reading_text(){
    let h=setup("auto","msedge.exe","");
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.target_capture_pending=true;s.target=None;s.target_application=None;}
    let worker=h.clone();let target=tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        worker.resolve_stop_target_with_context("s",7,||Ok(Some(delivery_target_fixture())),|_|Some("winword.exe".into()),|_|panic!("context disabled"));
    });
    complete(&h,|context|{let context=context.unwrap();assert_eq!(context.scene,AppWritingScene::Document);assert!(context.input.is_none());}).await;
    target.await.unwrap();
}

#[test]
fn target_can_deliver_while_optional_context_is_still_reading(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.prefs.delivery_mode=DeliveryMode::Direct;s.prefs.app_writing.input_context_enabled=true;}
    h.begin_stop_target(&identity("s"),Instant::now()).unwrap();
    let (started_tx,started_rx)=std::sync::mpsc::channel();let (release_tx,release_rx)=std::sync::mpsc::channel();
    let worker=h.clone();let native=std::thread::spawn(move||worker.resolve_stop_target_with_context("s",7,
        ||Ok(Some(delivery_target_fixture())),|_|Some("msedge.exe".into()),|_|{
            started_tx.send(()).unwrap();release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(InputContext{control_label:"Document body".into(),before_text:"LATE_HOST_TEXT".into(),..Default::default()})
        }));
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    {let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert!(s.target.is_some());assert!(!s.target_capture_pending);assert!(s.input_context_capture_pending);}
    ready(&h,"s");
    // Emulate generation's already-frozen context. Late host text cannot
    // revise the saved draft, even though delivery can proceed now.
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.input_context_frozen=true;s.input_context_status=InputContextStatus::Unavailable;}
    let calls=std::sync::atomic::AtomicUsize::new(0);
    h.deliver_once("s",7,|permit,target,text,gate|platform::delivery::dispatch_with_test_operations(permit,target,text,gate,|_|true,||{calls.fetch_add(1,Ordering::SeqCst);4}));
    assert_eq!(calls.load(Ordering::SeqCst),1);assert!(!view(&h).worker_released,"native context lease is still owned");
    release_tx.send(()).unwrap();native.join().unwrap();
    let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert!(!s.input_context_capture_pending);assert!(s.input_context.is_none());
    assert_eq!(s.input_context_status,InputContextStatus::Unavailable);
}

#[test]
fn cancelled_or_panicked_context_cannot_remove_target_or_leak_its_lease(){
    for panic_context in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::Recording,true);
        h.inner.lock().unwrap().active.as_mut().unwrap().prefs.app_writing.input_context_enabled=true;
        h.begin_stop_target(&identity("s"),Instant::now()).unwrap();
        let resolved=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||h.resolve_stop_target_with_context("s",7,
            ||Ok(Some(delivery_target_fixture())),|_|Some("msedge.exe".into()),|_|{
                if panic_context {panic!("synthetic reader failure");}
                h.inner.lock().unwrap().active.as_ref().unwrap().cancel.cancel();
                Ok(InputContext{before_text:"DISCARDED_CONTEXT".into(),..Default::default()})
            })));
        assert_eq!(resolved.is_err(),panic_context);
        let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert!(s.target.is_some());
        assert!(!s.target_capture_pending&&!s.input_context_capture_pending);assert!(s.input_context.is_none());
        assert_eq!(s.input_context_status,InputContextStatus::Unavailable);
    }
}
