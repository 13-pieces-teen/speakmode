//! Actual coordinator -> local HTTP -> SQLite evidence. No model, key lookup,
//! microphone, host insertion, or clipboard is exercised by these tests.
use super::*;
use crate::cloud::rewrite::minimum_pipeline::tests::{Server,Reply};

const ACCEPT:&str=r#"{"decision":"accept","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"pass"},"issues":[]}"#;

fn install_personalized(h:&Arc<AppHub>,id:&str,text:&str,language:DraftLanguage) {
    save(h,config());install(h,id,SessionPhase::FinalizingAsr,true);
    {let mut inner=h.inner.lock().unwrap();let session=inner.active.as_mut().unwrap();
        session.mode=h.db.mode_snapshot("technical",None).unwrap();session.config.region=Some("beijing".into());
        session.prefs.app_writing.output_language=language;
    }
    let mut input=source(id);input.items.truncate(1);input.items[0].text=text.into();
    h.checkpoint(id,7,input).unwrap();
}

#[tokio::test]
async fn selected_snapshot_without_transport_does_not_claim_dispatch_and_legacy_stays_unknown() {
    let h=hub();let mode=h.db.mode_snapshot("technical",None).unwrap();
    let context=personalization::context_for(&mode,DraftLanguage::AsSpoken,&config()).unwrap();
    for (id,marker,status) in [
        ("sent-ok",Some(ReferenceDispatch::Started),PersonalizationEventStatus::Success),
        ("sent-error",Some(ReferenceDispatch::Started),PersonalizationEventStatus::Failed),
        ("preflight-error",Some(ReferenceDispatch::NotStarted),PersonalizationEventStatus::Failed),
        ("pending",Some(ReferenceDispatch::NotStarted),PersonalizationEventStatus::Started),
        ("legacy-ok",None,PersonalizationEventStatus::Success),
        ("legacy-error",None,PersonalizationEventStatus::Failed),
        ("interrupted",Some(ReferenceDispatch::Unknown),PersonalizationEventStatus::Failed),
    ] {
        let mut event=personalization::task_event(id,PersonalizationEventKind::Normal,Some(&context),status,0);
        event.reference_dispatch=marker;h.record_personalization_event(event.clone()).await.unwrap();
        h.record_personalization_event(event).await.unwrap(); // exact completion replay is one task
    }
    h.record_personalization_event(personalization::task_event("unselected",PersonalizationEventKind::Normal,None,PersonalizationEventStatus::Success,0)).await.unwrap();
    for (id,marker) in [("preview-sent",ReferenceDispatch::Started),("preview-local-error",ReferenceDispatch::NotStarted)] {
        let mut event=personalization::task_event(id,PersonalizationEventKind::Preview,Some(&context),PersonalizationEventStatus::Failed,0);
        event.reference_dispatch=Some(marker);h.record_personalization_event(event).await.unwrap();
    }
    let stats=command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap();
    assert_eq!(stats["normal"],json!({"total":8,"personalized":2,"selected":7,"legacyUnknown":2,"interruptedUnknown":1,"pending":1}));
    assert_eq!(stats["preview"],json!({"total":2,"personalized":1,"selected":2,"legacyUnknown":0,"interruptedUnknown":0,"pending":0}));
    let technical=stats["scenes"].as_array().unwrap().iter().find(|row|row["scene"]=="technical").unwrap();
    assert_eq!(technical["count"],7);assert_eq!(technical["personalized"],2);assert_eq!(technical["legacyUnknown"],2);
    assert!(!stats.to_string().contains("工作背景哨兵"));
}

#[tokio::test]
async fn coordinator_preflight_zero_http_requests_is_selected_but_not_dispatched() {
    let h=hub();let id="dispatch-local-preflight";let text="请把这两个约束分开列点。";
    install_personalized(&h,id,text,DraftLanguage::English);
    let server=Server::new(vec![Reply::new("{}")] ).await;let endpoint=server.endpoint.clone();
    h.finish_rewrite_with_context_policy(id.into(),7,move|config,mode,input,cancel,execution,context|async move{
        rewrite::minimum_pipeline::run_until_with_writing_context(&reqwest::Client::builder().no_proxy().build().unwrap(),
            &config,"synthetic",&mode,&input,&DictionarySnapshot::default(),cancel,&endpoint,execution,&context.unwrap()).await
    },None::<RewriteWaitPolicy>,|h,_,_|h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true).await;
    assert_eq!(server.count(),0);
    let stats=command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap();
    assert_eq!(stats["normal"]["selected"],1);assert_eq!(stats["normal"]["personalized"],0);assert_eq!(stats["records"][0]["referenceDispatch"],"notStarted");
    let detail=h.db.get_result(id).unwrap();assert_eq!(detail.source.text(),text);assert_eq!(detail.output.unwrap().text(),text);
}

#[tokio::test]
async fn coordinator_four_real_profile_requests_count_once_even_with_repair_and_review() {
    let h=hub();let id="dispatch-four-stages";let text="嗯，呃呃，就是就是，我我觉得方案暂时不可行。";
    install_personalized(&h,id,text,DraftLanguage::AsSpoken);install_trace(&h);
    let server=Server::new(vec![Reply::new("__ECHO_GUARDED_SOURCE__"),Reply::new(ACCEPT),
        Reply::new(json!({"replacements":[{"id":"r0","text":"我觉得方案暂时不可行。"}]}).to_string()),Reply::new(ACCEPT)]).await;
    let endpoint=server.endpoint.clone();
    h.finish_rewrite_with_context_policy(id.into(),7,move|config,mode,input,cancel,execution,context|async move{
        rewrite::minimum_pipeline::run_until_with_writing_context(&reqwest::Client::builder().no_proxy().build().unwrap(),
            &config,"synthetic",&mode,&input,&DictionarySnapshot::default(),cancel,&endpoint,execution,&context.unwrap()).await
    },None::<RewriteWaitPolicy>,|h,_,_|h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true).await;
    assert_eq!(server.count(),4);
    let stats=command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap();
    assert_eq!(stats["normal"]["total"],1);assert_eq!(stats["normal"]["personalized"],1);assert_eq!(stats["records"][0]["referenceDispatch"],"started");
    assert_eq!(stats["records"][0]["status"],"success");
    assert_eq!(h.diagnostics.snapshot().unwrap().traces[0].rewrite.as_ref().unwrap().requests.len(),4);
    let detail=h.db.get_result(id).unwrap();assert_eq!(detail.source.text(),text);assert_eq!(detail.output.unwrap().text(),"我觉得方案暂时不可行。");
}

#[tokio::test]
async fn coordinator_http_without_profile_payload_cannot_count_selected_context_as_dispatched() {
    let h=hub();let id="dispatch-payload-mismatch";let text="方案暂时不可行。";
    install_personalized(&h,id,text,DraftLanguage::AsSpoken);
    let server=Server::new(vec![Reply::new("__ECHO_GUARDED_SOURCE__"),Reply::new(ACCEPT)]).await;let endpoint=server.endpoint.clone();
    h.finish_rewrite_with_context_policy(id.into(),7,move|config,mode,input,cancel,execution,context|async move{
        assert!(context.unwrap().personalization.is_some());
        // This substituted provider seam intentionally omits the frozen context.
        // The observer must inspect wire data, not just assume selection = use.
        rewrite::rewrite_with_client_execution(&reqwest::Client::builder().no_proxy().build().unwrap(),
            &config,"synthetic",&mode,&input,&DictionarySnapshot::default(),cancel,&endpoint,execution).await
    },None::<RewriteWaitPolicy>,|h,_,_|h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true).await;
    assert_eq!(server.count(),2);
    let stats=command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap();
    assert_eq!(stats["normal"]["selected"],1);assert_eq!(stats["normal"]["personalized"],0);assert_eq!(stats["records"][0]["referenceDispatch"],"notStarted");
}
