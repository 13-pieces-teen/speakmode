//! Coordinator behavior with synthetic source and provider seams; no credentials or network.
use super::*;
use crate::core::personalization::*;

fn config()->PersonalizationConfig {
    PersonalizationConfig{enabled:true,active_profile_id:Some("dev".into()),profiles:vec![PersonalProfile{id:"dev".into(),name:"AI 后端".into(),profession:"软件开发".into(),domain:"AI 后端".into(),description:"工作背景哨兵".into(),tasks:vec!["技术沟通".into()],audience:"同事".into(),scenes:vec![AppWritingScene::Technical],instructions:"".into()}],preferences:ExpressionStyle{paragraphs:true,..Default::default()},..Default::default()}
}
fn save(h:&AppHub,config:PersonalizationConfig)->PersonalizationConfig {
    let expected_version=h.db.snapshot().personalization.version;
    match h.db.run(DbCommand::SavePersonalization{config,expected_version}).unwrap(){DbValue::Personalization(value)=>value,_=>panic!("profile reply")}
}
fn result(source:&SourceRevision)->rewrite::RewriteResult {
    rewrite::RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![OutputBlock{id:"fixture".into(),text:source.text(),source_refs:source.items.iter().map(|i|i.id.clone()).collect()}]}
}

#[tokio::test]
async fn save_conflicts_retain_current_config_and_get_does_not_require_cloud() {
    let h=hub();let fetched=command(&h,"personalization.get",json!({})).await.unwrap();assert_eq!(fetched["enabled"],false);assert_eq!(fetched["capability"]["modelQuality"],"notEvaluated");
    let next=config();let saved=command(&h,"personalization.save",json!({"config":next,"expectedVersion":1,"requestId":"save-profile"})).await.unwrap();assert_eq!(saved["version"],2);
    assert_eq!(command(&h,"personalization.save",json!({"config":next,"expectedVersion":1,"requestId":"stale-profile"})).await.unwrap_err().code,"revisionConflict");
    assert_eq!(h.db.snapshot().personalization.version,2);
    let mut bad=h.db.snapshot().personalization;bad.profiles[0].description="字".repeat(301);
    assert!(command(&h,"personalization.save",json!({"config":bad,"expectedVersion":2,"requestId":"too-long"})).await.is_err());
    assert_eq!(h.db.snapshot().personalization.profiles[0].description,"工作背景哨兵");
}

#[tokio::test]
async fn ordinary_edits_freeze_next_task_and_receipt_uses_original_reference() {
    let h=hub();let original=save(&h,config());install(&h,"profile-frozen",SessionPhase::FinalizingAsr,true);
    {let mut inner=h.inner.lock().unwrap();let session=inner.active.as_mut().unwrap();session.mode=h.db.mode_snapshot("technical",None).unwrap();}
    checkpoint(&h,"profile-frozen");
    let mut next=original.clone();next.profiles[0].profession="银行业务".into();
    command(&h,"personalization.save",json!({"config":next,"expectedVersion":original.version,"requestId":"next-profile"})).await.unwrap();
    h.finish_rewrite_with_context_policy("profile-frozen".into(),7,|_,_,input,_,_,context|async move{
        let profile=context.unwrap().personalization.unwrap();assert_eq!(profile.version,original.version);assert_eq!(profile.background.unwrap().profession,"软件开发");Ok(result(&input))
    },None::<RewriteWaitPolicy>,|h,_,_|{h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;}).await;
    let detail=h.db.get_result("profile-frozen").unwrap();assert_eq!(detail.source,source("profile-frozen"));
    assert_eq!(detail.output.unwrap().writing_receipt.unwrap().personalization.unwrap().version,original.version);
    let stats=command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap();assert_eq!(stats["normal"]["total"],1);assert_eq!(stats["normal"]["personalized"],1);
    assert!(!stats.to_string().contains("工作背景哨兵"));
}

#[tokio::test]
async fn disabling_cancels_registered_workers_and_preserves_raw_source() {
    let h=hub();let old=save(&h,config());install(&h,"profile-cancel",SessionPhase::FinalizingAsr,true);checkpoint(&h,"profile-cancel");
    let token=CancellationToken::new();let plain=CancellationToken::new();let profile=old.freeze(AppWritingScene::Technical).unwrap();
    {let mut inner=h.inner.lock().unwrap();inner.personalization_workers.insert("personalization:live".into(),profile.clone());inner.auxiliary_workers.insert("personalization:live".into(),token.clone());inner.auxiliary_workers.insert("plain".into(),plain.clone());}
    let mut next=old.clone();next.enabled=false;
    command(&h,"personalization.save",json!({"config":next,"expectedVersion":old.version,"requestId":"disable-profile"})).await.unwrap();
    assert!(token.is_cancelled());assert!(!plain.is_cancelled());
    let inner=h.inner.lock().unwrap();let session=inner.active.as_ref().unwrap();assert!(session.cancel.is_cancelled());assert_eq!(session.source.as_ref().unwrap(),&source("profile-cancel"));assert_eq!(session.phase,SessionPhase::Incomplete);
    assert!(personalization::revoked(&inner,&profile),"read-before-disable cannot register later");
}

#[tokio::test]
async fn deleting_old_identity_after_switch_still_cancels_its_frozen_task() {
    let h=hub();let old=save(&h,config());install(&h,"old-profile",SessionPhase::FinalizingAsr,true);checkpoint(&h,"old-profile");
    let mut next=old.clone();next.profiles.push(PersonalProfile{id:"bank".into(),name:"银行".into(),profession:"银行".into(),..next.profiles[0].clone()});next.active_profile_id=Some("bank".into());
    let switched=command(&h,"personalization.save",json!({"config":next,"expectedVersion":old.version,"requestId":"switch-profile"})).await.unwrap();assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled());
    let mut next:PersonalizationConfig=serde_json::from_value(switched).unwrap();let expected=next.version;next.profiles.retain(|p|p.id!="dev");
    command(&h,"personalization.save",json!({"config":next,"expectedVersion":expected,"requestId":"delete-old"})).await.unwrap();
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled());assert_eq!(h.db.get_result("old-profile").unwrap().source,source("old-profile"));
}

#[tokio::test]
async fn feedback_is_explicit_valid_only_and_aggregate_denominator_is_deduplicated() {
    let h=hub();let cfg=config();let mode=h.db.mode_snapshot("technical",None).unwrap();let context=personalization::context_for(&mode,DraftLanguage::AsSpoken,&cfg).unwrap();
    let mut good=personalization::task_event("paired-1",PersonalizationEventKind::Comparison,Some(&context),PersonalizationEventStatus::Started,0);
    h.record_personalization_event(good.clone()).await.unwrap();good.status=PersonalizationEventStatus::Success;h.record_personalization_event(good.clone()).await.unwrap();h.record_personalization_event(good.clone()).await.unwrap();
    let bad=personalization::task_event("paired-2",PersonalizationEventKind::Comparison,Some(&context),PersonalizationEventStatus::Failed,40);h.record_personalization_event(bad).await.unwrap();
    assert!(command(&h,"personalization.feedback",json!({"comparisonId":"paired-2","verdict":"better","requestId":"failed-feedback"})).await.is_err());
    let empty=command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap();assert_eq!(empty["comparisons"]["valid"],1);assert_eq!(empty["comparisons"]["feedback"]["total"],0);
    command(&h,"personalization.feedback",json!({"comparisonId":"paired-1","verdict":"better","requestId":"feedback-one"})).await.unwrap();
    command(&h,"personalization.feedback",json!({"comparisonId":"paired-1","verdict":"worse","requestId":"feedback-change"})).await.unwrap();
    let stats=command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap();assert_eq!(stats["comparisons"]["total"],2);assert_eq!(stats["comparisons"]["feedback"]["total"],1);assert_eq!(stats["comparisons"]["feedback"]["worse"],1);assert_eq!(stats["comparisons"]["feedback"]["better"],0);
}

#[tokio::test]
async fn compare_preflight_does_not_mutate_history_or_require_a_key_for_invalid_source() {
    let h=hub();save(&h,config());
    let empty=json!({"operationId":"empty-compare","source":{"kind":"text","text":" "},"modeId":"technical","requestId":"empty-compare"});
    assert!(command(&h,"personalization.compare",empty).await.is_err());
    install(&h,"compare-real",SessionPhase::FinalizingAsr,true);ready(&h,"compare-real");let before=h.db.get_result("compare-real").unwrap();
    let stale=json!({"operationId":"stale-compare","source":{"kind":"history","sessionId":"compare-real","sourceRevisionId":"old"},"modeId":"technical","requestId":"stale-compare"});
    assert_eq!(command(&h,"personalization.compare",stale).await.unwrap_err().code,"revisionConflict");assert_eq!(before,h.db.get_result("compare-real").unwrap());
    assert!(h.inner.lock().unwrap().auxiliary_workers.is_empty());
    assert_eq!(command(&h,"personalization.get",json!({})).await.unwrap()["capability"]["supported"],true);
}

#[tokio::test]
async fn disable_during_generation_cannot_commit_or_deliver_a_late_draft() {
    let h=hub();let old=save(&h,config());install(&h,"late-profile",SessionPhase::FinalizingAsr,true);checkpoint(&h,"late-profile");
    let worker=h.clone();let delivered=Arc::new(AtomicBool::new(false));let delivery=delivered.clone();
    h.finish_rewrite_with_context_policy("late-profile".into(),7,move|_,_,input,_,_,context|async move {
        assert!(context.unwrap().personalization.is_some());let mut disabled=old.clone();disabled.enabled=false;
        command(&worker,"personalization.save",json!({"config":disabled,"expectedVersion":old.version,"requestId":"mid-generation-disable"})).await.unwrap();
        Ok(result(&input))
    },None::<RewriteWaitPolicy>,move|_,_,_|{delivery.store(true,Ordering::SeqCst);}).await;
    assert!(!delivered.load(Ordering::SeqCst));let detail=h.db.get_result("late-profile").unwrap();assert!(detail.output.is_none());assert_eq!(detail.source,source("late-profile"));
    let stats=command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap();assert_eq!(stats["records"][0]["status"],"cancelled");
}

#[tokio::test]
async fn replay_of_old_save_cannot_resurrect_a_deleted_profile() {
    let h=hub();let original=json!({"config":config(),"expectedVersion":1,"requestId":"original-profile-save"});
    let saved=command(&h,"personalization.save",original.clone()).await.unwrap();let mut deleted:PersonalizationConfig=serde_json::from_value(saved).unwrap();let version=deleted.version;deleted.profiles.clear();deleted.active_profile_id=None;
    command(&h,"personalization.save",json!({"config":deleted,"expectedVersion":version,"requestId":"remove-profile-save"})).await.unwrap();
    assert_eq!(command(&h,"personalization.save",original).await.unwrap_err().code,"request_already_processed");
    assert!(command(&h,"personalization.get",json!({})).await.unwrap()["profiles"].as_array().unwrap().is_empty());
}
