use super::*;

fn history_preview_payload(run:&str,id:&str)->Value {
    let mut payload=preview_payload(run);
    payload.as_object_mut().unwrap().remove("sample");
    payload["source"]=json!({"sessionId":id,"sourceRevisionId":format!("source-{id}")});
    payload
}

fn result_for(mode:&ModeSnapshot,source:&SourceRevision)->rewrite::RewriteResult {
    rewrite::RewriteResult{resolved_mode:mode.base_mode,blocks:vec![OutputBlock{id:"fixture-block".into(),text:source.text(),source_refs:source.items.iter().map(|item|item.id.clone()).collect()}]}
}

#[test]
fn comparison_is_independent_of_an_unfinished_personal_draft() {
    for draft in [Value::Null,json!({"name":"","baseMode":"auto","rules":""})] {
        let (_,mode)=preview_options(&json!({"compareBuiltins":true,"draft":draft})).unwrap();
        assert!(mode.is_none(),"built-in comparison cannot inherit personal formatting rules");
    }
    assert!(preview_options(&json!({"compareBuiltins":true})).unwrap().1.is_none());
    assert!(preview_options(&json!({"compareBuiltins":"true"})).is_err());
    assert!(preview_options(&json!({"compareBuiltins":false,"draft":{"name":"","baseMode":"auto","rules":""}})).is_err());
    let (_,mode)=preview_options(&preview_payload("normal")).unwrap();
    let mode=mode.unwrap();assert_eq!(mode.rules,"保留条件和否定。");assert_eq!(mode.base_mode,BaseMode::Light);
}

#[test]
fn preview_source_rejects_ambiguous_or_unbounded_payloads() {
    for payload in [
        json!({"source":{"sessionId":"s","sourceRevisionId":"r"},"sample":"文本"}),
        json!({"source":{"sessionId":"s","sourceRevisionId":"r"},"text":"文本"}),
        json!({"source":null,"sample":"文本"}),json!({"sample":"一","text":"二"}),
        json!({"source":{"sessionId":"","sourceRevisionId":"r"}}),
        json!({"source":{"sessionId":"s","sourceRevisionId":"r","items":["不能由前端提供"]}}),
        json!({"sample":" \r\n \n"}),json!({"sample":"字".repeat(80_001)}),
        json!({"sample":vec!["字";2001].join("\n")}),
    ] {assert!(preview_source_input(&payload).is_err(),"{payload}");}
    assert!(preview_source_input(&json!({"sample":"😀".repeat(80_000)})).is_ok(),"limit is Unicode scalars, not UTF-8 bytes");
    assert!(preview_source_input(&json!({"sample":vec!["字";2000].join("\n")})).is_ok());
}

#[tokio::test]
async fn synthetic_preview_splits_only_explicit_nonempty_lines() {
    let h=hub();
    let input=preview_source_input(&json!({"sample":" 第一项。第二句。\r\n\r\n第二项？\n \n第三项。 "})).unwrap();
    let loaded=h.load_preview_source(input,"synthetic-run").await.unwrap();
    assert_eq!(loaded.kind,"synthetic");assert_eq!(loaded.revision.adapter_version,"synthetic-lines-v1");
    assert_eq!(loaded.revision.items.iter().map(|item|item.text.as_str()).collect::<Vec<_>>(),[" 第一项。第二句。","第二项？","第三项。 "]);
    assert_eq!(loaded.revision.items.iter().map(|item|(item.id.as_str(),item.capture_order,item.start_ms,item.end_ms)).collect::<Vec<_>>(),[("preview:0",0,None,None),("preview:1",1,None,None),("preview:2",2,None,None)]);
    assert!(loaded.revision.items.iter().all(|item|item.is_final));
    h.validate_preview_source(&loaded).await.unwrap();
}

#[tokio::test]
async fn historical_preview_keeps_real_source_ids_segments_and_timestamps() {
    let h=hub();install(&h,"real-segments",SessionPhase::FinalizingAsr,true);ready(&h,"real-segments");
    let before=h.db.get_result("real-segments").unwrap();
    let input=preview_source_input(&history_preview_payload("preview-history","real-segments")).unwrap();
    let loaded=h.load_preview_source(input,"preview-history").await.unwrap();
    assert_eq!(loaded.kind,"history");assert_eq!(loaded.revision,before.source);
    h.validate_preview_source(&loaded).await.unwrap();
    let mode=preview_options(&preview_payload("unused")).unwrap().1.unwrap();
    let wire=preview_response("preview-history",&loaded,PreviewOutputs::Single(result_for(&mode,&loaded.revision)));
    assert_eq!(wire["sourceKind"],"history");assert_eq!(wire["sourceItemCount"],2);assert_eq!(wire["sourceRevisionId"],before.source.id);
    assert_eq!(wire["promptVersion"],rewrite::REWRITE_PROMPT_VERSION);assert_eq!(wire["blocks"][0]["sourceRefs"],json!(["item-real-segments-0","item-real-segments-1"]));
    assert_eq!(h.db.get_result("real-segments").unwrap().output,before.output,"trial never adopts an output");
}

#[tokio::test]
async fn stale_or_deleted_preview_history_fails_before_provider_configuration() {
    let h=hub();install(&h,"history-stale",SessionPhase::FinalizingAsr,true);ready(&h,"history-stale");
    assert!(h.db.snapshot().provider_config.region.is_none(),"fixture cannot read a credential");
    let newer=CancellationToken::new();h.inner.lock().unwrap().preview=Some(("newer".into(),newer.clone()));
    let mut stale=history_preview_payload("stale-source","history-stale");stale["source"]["sourceRevisionId"]=json!("old-source");
    assert_eq!(command(&h,"mode.preview",stale).await.unwrap_err().code,"revisionConflict");
    assert_eq!(command(&h,"mode.preview",history_preview_payload("deleted-source","missing-record")).await.unwrap_err().code,StoreError::NotFound.code());
    assert!(!newer.is_cancelled());assert_eq!(h.inner.lock().unwrap().preview.as_ref().unwrap().0,"newer");
    assert!(h.inner.lock().unwrap().auxiliary_workers.is_empty());
    let mut compare=history_preview_payload("comparison-preflight","history-stale");compare.as_object_mut().unwrap().remove("draft");compare["compareBuiltins"]=json!(true);
    assert_eq!(command(&h,"mode.preview",compare).await.unwrap_err().code,"region_required","comparison without a draft reaches configuration after validating the source");
}

#[tokio::test]
async fn oversized_history_source_is_rejected_before_configuration_or_credentials() {
    let h=hub();install(&h,"oversized-source",SessionPhase::FinalizingAsr,true);
    let mut oversized=source("oversized-source");oversized.items[0].text="字".repeat(80_001);
    h.checkpoint("oversized-source",7,oversized).unwrap();
    assert!(h.db.snapshot().provider_config.region.is_none());
    let error=command(&h,"mode.preview",history_preview_payload("oversized-preview","oversized-source")).await.unwrap_err();
    assert_eq!(error.code,"invalid_source");assert!(h.inner.lock().unwrap().preview.is_none());assert!(h.inner.lock().unwrap().auxiliary_workers.is_empty());
}

#[tokio::test]
async fn preview_source_validation_detects_deleted_or_changed_history_after_request() {
    let h=hub();install(&h,"history-gone",SessionPhase::FinalizingAsr,true);ready(&h,"history-gone");
    let mut loaded=h.load_preview_source(preview_source_input(&history_preview_payload("load","history-gone")).unwrap(),"load").await.unwrap();
    loaded.revision.items[0].text.push_str(" changed");
    assert_eq!(h.validate_preview_source(&loaded).await.unwrap_err().code,"revisionConflict","same revision ID does not conceal altered segmentation or content");
    loaded.revision=source("history-gone");
    h.db.run(DbCommand::DeleteHistory{session_id:"history-gone".into()}).unwrap();
    assert_eq!(h.validate_preview_source(&loaded).await.unwrap_err().code,StoreError::NotFound.code());
}

#[tokio::test]
async fn builtin_comparison_runs_two_sequential_calls_on_identical_source() {
    let source=source("comparison");let original=source.clone();let calls=Arc::new(Mutex::new(Vec::new()));let observed=Arc::clone(&calls);
    let result=run_preview_rewrites(None,&source,CancellationToken::new(),move |mode,input,cancel| {
        let observed=Arc::clone(&observed);
        async move {
            let expected=if mode.base_mode==BaseMode::Light{0}else{1};
            assert_eq!(observed.lock().unwrap().len(),expected,"second request starts after the first finishes");
            tokio::task::yield_now().await;
            assert!(!cancel.is_cancelled());assert!(mode.rules.is_empty());
            let result=result_for(&mode,&input);observed.lock().unwrap().push((mode,input));Ok(result)
        }
    }).await.unwrap();
    let calls=calls.lock().unwrap();assert_eq!(calls.len(),2);assert_eq!(calls[0].0.base_mode,BaseMode::Light);assert_eq!(calls[1].0.base_mode,BaseMode::Structure);
    assert_eq!(calls[0].1,original);assert_eq!(calls[1].1,original);
    let wire=preview_response("compare-run",&PreviewSource{kind:"history",revision:source},result);
    assert_eq!(wire["comparison"]["light"]["resolvedMode"],"light");assert_eq!(wire["comparison"]["structure"]["resolvedMode"],"structure");
    assert_eq!(wire["sourceItemCount"],2);assert_eq!(wire["sourceRevisionId"],"source-comparison");assert!(wire.get("blocks").is_none(),"there is no implicitly selected comparison output");
}

#[tokio::test]
async fn comparison_cancellation_after_light_never_starts_structure() {
    let source=source("cancel-pair");let token=CancellationToken::new();let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));let observed=Arc::clone(&calls);
    let result=run_preview_rewrites(None,&source,token.clone(),move |mode,input,cancel| {
        observed.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        async move {cancel.cancel();Ok(result_for(&mode,&input))}
    }).await;
    assert_eq!(result.err().unwrap().code,"cancelled");assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),1);
    let result=run_preview_rewrites(None,&source,token,|_,_,_|async {panic!("already cancelled preview cannot call the provider")}).await;
    assert_eq!(result.err().unwrap().code,"cancelled");
}

#[tokio::test]
async fn comparison_provider_failure_does_not_return_partial_results_or_retry() {
    for fail_on in [BaseMode::Light,BaseMode::Structure] {
        let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));let observed=Arc::clone(&calls);
        let result=run_preview_rewrites(None,&source("failure"),CancellationToken::new(),move |mode,input,_| {
            observed.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
            async move {if mode.base_mode==fail_on{Err(CloudError::new("synthetic_failure","rewrite","fixture"))}else{Ok(result_for(&mode,&input))}}
        }).await;
        assert_eq!(result.err().unwrap().code,"synthetic_failure");assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),if fail_on==BaseMode::Light{1}else{2});
    }
}

#[tokio::test]
async fn cancelling_pending_structure_call_keeps_one_lease_until_the_pair_exits() {
    let h=hub();let token=CancellationToken::new();let operation="preview:pair-lease".to_owned();
    {let mut inner=h.inner.lock().unwrap();inner.preview=Some(("pair-lease".into(),token.clone()));inner.auxiliary_workers.insert(operation.clone(),token.clone());}
    let lease=AuxiliaryLease{hub:h.clone(),key:operation.clone()};
    let (started_tx,started_rx)=tokio::sync::oneshot::channel();let started_tx=Arc::new(Mutex::new(Some(started_tx)));
    let worker=tokio::spawn(async move {
        let work=async {
            run_preview_rewrites(None,&source("pending-pair"),token,move |mode,input,_| {
                let started_tx=Arc::clone(&started_tx);
                async move {
                    if mode.base_mode==BaseMode::Structure {
                        started_tx.lock().unwrap().take().unwrap().send(()).unwrap();
                        // Deliberately ignores its token; the production runner
                        // must cancel and drop a pending provider request itself.
                        std::future::pending().await
                    } else {Ok(result_for(&mode,&input))}
                }
            }).await
        };
        (AuxiliaryFuture{future:Some(Box::pin(work)),lease:Some(lease)}).await
    });
    started_rx.await.unwrap();assert!(!worker.is_finished());assert_eq!(h.inner.lock().unwrap().auxiliary_workers.len(),1);
    h.cancel_preview(&json!({"runId":"pair-lease"})).unwrap();
    assert!(h.inner.lock().unwrap().auxiliary_workers.contains_key(&operation),"clearing a UI slot must not release a live request");
    let result=tokio::time::timeout(Duration::from_secs(1),worker).await.unwrap().unwrap();
    assert_eq!(result.err().unwrap().code,"cancelled");assert!(h.inner.lock().unwrap().auxiliary_workers.is_empty());assert!(h.inner.lock().unwrap().preview.is_none());
}

#[tokio::test]
async fn personal_preview_still_performs_one_call_with_its_exact_rules() {
    let mode=preview_options(&preview_payload("personal")).unwrap().1.unwrap();let expected=mode.clone();let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));let observed=Arc::clone(&calls);
    let result=run_preview_rewrites(Some(mode),&source("single"),CancellationToken::new(),move |mode,input,_| {
        assert_eq!(mode,expected);observed.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        async move {Ok(result_for(&mode,&input))}
    }).await.unwrap();
    assert!(matches!(result,PreviewOutputs::Single(_)));assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),1);
}

#[tokio::test]
async fn deleting_history_during_light_cancels_the_pair_before_structure_and_releases_its_lease() {
    let h=hub();install(&h,"delete-during-preview",SessionPhase::FinalizingAsr,true);ready(&h,"delete-during-preview");
    let source=h.load_preview_source(preview_source_input(&history_preview_payload("delete-pair","delete-during-preview")).unwrap(),"delete-pair").await.unwrap();
    let token=CancellationToken::new();let lease=h.reserve_preview("delete-pair",&source,token.clone(),0).unwrap();
    // Keep a pending provider resource alive until cancellation. Its Drop probe
    // asserts that native admission stays reserved until the request is dropped.
    let (_hold_request,request_rx)=tokio::sync::oneshot::channel();
    let dropped=Arc::new(AtomicBool::new(false));let mut probe=Some(ResourceProbe{hub:h.clone(),key:"preview:delete-pair".into(),release:request_rx,drop_observed:dropped.clone()});
    let (started_tx,started_rx)=tokio::sync::oneshot::channel();let mut started_tx=Some(started_tx);
    let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));let observed=calls.clone();
    let worker=tokio::spawn(async move {
        let work=async {
            run_preview_rewrites(None,&source.revision,token,move |mode,input,_| {
                observed.fetch_add(1,Ordering::SeqCst);assert_eq!(mode.base_mode,BaseMode::Light,"deletion must prevent the Structure request");
                let resource=probe.take().unwrap();let started=started_tx.take().unwrap();
                async move {started.send(()).unwrap();resource.await;Ok(result_for(&mode,&input))}
            }).await
        };
        (AuxiliaryFuture{future:Some(Box::pin(work)),lease:Some(lease)}).await
    });
    started_rx.await.unwrap();
    assert_eq!(h.inner.lock().unwrap().preview_sources.get("preview:delete-pair").map(String::as_str),Some("delete-during-preview"));
    command(&h,"history.delete",json!({"sessionId":"delete-during-preview","requestId":"delete-live-preview-source"})).await.unwrap();
    let result=tokio::time::timeout(Duration::from_secs(1),worker).await.unwrap().unwrap();
    assert_eq!(result.err().unwrap().code,"cancelled");assert_eq!(calls.load(Ordering::SeqCst),1);assert!(dropped.load(Ordering::SeqCst));
    let inner=h.inner.lock().unwrap();assert!(inner.preview.is_none());assert!(inner.preview_sources.is_empty());assert!(inner.auxiliary_workers.is_empty());drop(inner);
    assert!(matches!(h.db.get_result("delete-during-preview"),Err(StoreError::NotFound)));
}

#[tokio::test]
async fn deleting_old_source_or_dropping_its_lease_preserves_newer_other_and_synthetic_previews() {
    for synthetic in [false,true] {
        let h=hub();install(&h,"old-preview-source",SessionPhase::FinalizingAsr,true);ready(&h,"old-preview-source");
        install(&h,"other-preview-source",SessionPhase::FinalizingAsr,true);ready(&h,"other-preview-source");
        let old=PreviewSource{kind:"history",revision:source("old-preview-source")};let old_token=CancellationToken::new();let old_lease=h.reserve_preview("old-source-run",&old,old_token.clone(),0).unwrap();
        let next=if synthetic{h.load_preview_source(preview_source_input(&json!({"sample":"自由输入。"})).unwrap(),"new-source-run").await.unwrap()}else{PreviewSource{kind:"history",revision:source("other-preview-source")}};
        let next_token=CancellationToken::new();let next_lease=h.reserve_preview("new-source-run",&next,next_token.clone(),0).unwrap();
        assert!(old_token.is_cancelled(),"replacement already cancels the old request");
        command(&h,"history.delete",json!({"sessionId":"old-preview-source","requestId":"delete-old-source-only"})).await.unwrap();
        assert!(!next_token.is_cancelled());
        {let inner=h.inner.lock().unwrap();assert_eq!(inner.preview.as_ref().unwrap().0,"new-source-run");assert!(inner.preview_sources.contains_key("preview:old-source-run"),"old association remains until its resource exits");}
        drop(old_lease);
        {let inner=h.inner.lock().unwrap();assert!(!inner.preview_sources.contains_key("preview:old-source-run"));assert_eq!(inner.preview_sources.get("preview:new-source-run").map(String::as_str),if synthetic{None}else{Some("other-preview-source")});assert!(inner.auxiliary_workers.contains_key("preview:new-source-run"));assert_eq!(inner.preview.as_ref().unwrap().0,"new-source-run");}
        drop(next_lease);assert!(h.inner.lock().unwrap().preview_sources.is_empty());
    }
}

#[tokio::test]
async fn deleted_source_cannot_register_a_preview_after_preflight_or_displace_a_newer_run() {
    let h=hub();install(&h,"preflight-delete",SessionPhase::FinalizingAsr,true);ready(&h,"preflight-delete");
    let loaded=h.load_preview_source(preview_source_input(&history_preview_payload("preflight-run","preflight-delete")).unwrap(),"preflight-run").await.unwrap();
    h.validate_preview_source(&loaded).await.unwrap();
    let synthetic=h.load_preview_source(preview_source_input(&json!({"sample":"另一个试用。"})).unwrap(),"keep-new").await.unwrap();
    let next_token=CancellationToken::new();let next_lease=h.reserve_preview("keep-new",&synthetic,next_token.clone(),0).unwrap();
    command(&h,"history.delete",json!({"sessionId":"preflight-delete","requestId":"delete-between-preflight-and-register"})).await.unwrap();
    let error=h.reserve_preview("preflight-run",&loaded,CancellationToken::new(),0).err().unwrap();
    assert_eq!(error.code,"cancelled");assert!(!next_token.is_cancelled());
    {let inner=h.inner.lock().unwrap();assert_eq!(inner.preview.as_ref().unwrap().0,"keep-new");assert!(!inner.auxiliary_workers.contains_key("preview:preflight-run"));assert!(inner.preview_sources.is_empty());}
    drop(next_lease);
}
