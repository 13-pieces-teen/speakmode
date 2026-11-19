use super::*;
use super::rewrite_fallback_tests::assert_unfinished;
use crate::cloud::rewrite::minimum_pipeline::tests::{Server, Reply, rejection};

const ACCEPT: &str = r#"{"decision":"accept","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"pass"},"issues":[]}"#;

fn uncertain_review()->String {
    let mut review:Value=serde_json::from_str(ACCEPT).unwrap();review["decision"]=json!("reject");review["checks"]["cleanup"]=json!("uncertain");review.to_string()
}

const REVIEWABLE_DRAFT:&str="预算3500元，先不要发送。";
const LATEST_REVIEWABLE_DRAFT:&str="预算3500元。先不要发送。";

/// The full coordinator and both real HTTP attempts run here. Only the tiny
/// test deadline changes; a stalled primary server cannot block recovery.
async fn failed_reviewable_pipeline(h:&Arc<AppHub>,primary_timeout:bool,recovery_review_timeout:bool)->SourceRevision {
    install(h,"s",SessionPhase::FinalizingAsr,true);install_trace(h);
    h.inner.lock().unwrap().active.as_mut().unwrap().config.region=Some("beijing".into());
    let mut frozen=source("s");frozen.items.truncate(1);frozen.items[0].text="嗯，预算3500元，先不要发送。".into();
    h.checkpoint("s",7,frozen.clone()).unwrap();let processing=h.processing_lease("s",7);
    let review=if primary_timeout {Reply::new(ACCEPT).after(4000)}else{Reply::new(uncertain_review())};
    let primary=Server::new(vec![Reply::draft(REVIEWABLE_DRAFT),review]).await;
    let recovery=Server::new(if recovery_review_timeout {
        vec![Reply::draft(LATEST_REVIEWABLE_DRAFT),Reply::new(ACCEPT).after(4000)]
    }else{vec![Reply::new("synthetic provider credential failure").http_status(401)]}).await;
    let primary_endpoint=primary.endpoint.clone();let recovery_endpoint=recovery.endpoint.clone();
    h.finish_rewrite_with_recovery_policy("s".into(),7,move|config,mode,input,cancel,execution,context|async move {
        rewrite::rewrite_with_client_optional_context(&reqwest::Client::builder().no_proxy().build().unwrap(),&config,
            "synthetic-key",&mode,&input,&DictionarySnapshot::default(),cancel,&primary_endpoint,execution,context.as_ref()).await
    },move|config,mode,input,cancel,execution,context|async move {
        // Exercise a real review timeout without sleeping for the production
        // recovery budget. Observers and draft ownership remain unchanged.
        let execution=if recovery_review_timeout {execution.recovery(tokio::time::Instant::now()+Duration::from_millis(800))}else{execution};
        rewrite::minimum_pipeline::run_until_with_writing_context(&reqwest::Client::builder().no_proxy().build().unwrap(),
            &config,"synthetic-key",&mode,&input,&DictionarySnapshot::default(),cancel,&recovery_endpoint,execution,&context).await
    },RewriteWaitPolicy{deadline:Duration::from_millis(if primary_timeout{1600}else{3000})},|_,_,_|panic!("unreviewed draft cannot be delivered")).await;
    drop(processing);
    assert_unfinished(h);assert_eq!(h.db.get_result("s").unwrap().source,frozen);
    assert_eq!(primary.count(),2);assert_eq!(recovery.count(),if recovery_review_timeout{2}else{1});
    let expected=if recovery_review_timeout{"rewrite_timeout"}else{"authentication_failed"};
    let session=view(h);assert_eq!(session.error_code.as_deref(),Some(expected));assert!(session.recovery_candidate_id.is_some());
    let error=h.inner.lock().unwrap().active.as_ref().unwrap().error.clone().unwrap();assert_eq!(error.stage,"rewrite");
    assert!(error.message.contains(if recovery_review_timeout{"整理稿复核超时"}else{"认证失败"}),"{}",error.message);
    assert!(error.message.contains("待核对的整理候选"));
    let diagnostics=h.diagnostics.snapshot().unwrap();let trace=&diagnostics.traces[0];let rewrite=trace.rewrite.as_ref().unwrap();
    assert_eq!(trace.error_code.as_deref(),Some(expected));
    assert_eq!(rewrite.error_code,Some(if primary_timeout{"rewrite_timeout"}else{"rewrite_semantic_rejected"}));
    let recovery=rewrite.automatic_recovery.as_ref().unwrap();assert!(!recovery.validated);assert_eq!(recovery.error_code,Some(expected));
    assert!(rewrite.requests.iter().all(|request|request.end_ms.is_some()));
    frozen
}

#[tokio::test]
async fn failed_review_keeps_original_or_latest_draft_only_for_explicit_identity_bound_adoption() {
    for (primary_timeout,recovery_review_timeout) in [(false,false),(true,false),(false,true)] {
        let h=hub();let frozen=failed_reviewable_pipeline(&h,primary_timeout,recovery_review_timeout).await;
        let candidate_id=view(&h).recovery_candidate_id.unwrap();
        let expected_text=if recovery_review_timeout{LATEST_REVIEWABLE_DRAFT}else{REVIEWABLE_DRAFT};
        for window in ["main","recording"] {
            let snapshot=h.snapshot(window).unwrap();assert!(snapshot["session"]["recoveryCandidateId"].is_string());
            assert!(!snapshot.to_string().contains(expected_text),"snapshots expose candidate identity, never its body");
        }
        let payload=||json!({"sessionId":"s","expectedGeneration":7,"sourceRevisionId":frozen.id,"recoveryCandidateId":candidate_id,
            "runId":Uuid::new_v4().to_string(),"requestId":Uuid::new_v4().to_string()});
        let forbidden=h.command("recording",CommandRequest{name:"history.reviewRecovery".into(),payload:payload()}).await.unwrap_err();
        assert_eq!(forbidden.code,"forbidden");
        for field in ["expectedGeneration","sourceRevisionId","recoveryCandidateId"] {
            let mut stale=payload();stale[field]=if field=="expectedGeneration"{json!(8)}else{json!("foreign")};
            assert!(command(&h,"history.reviewRecovery",stale).await.is_err());
            assert!(h.inner.lock().unwrap().candidate.is_none());assert_unfinished(&h);
        }
        let request=payload();let run=request["runId"].as_str().unwrap().to_owned();
        let candidate=command(&h,"history.reviewRecovery",request).await.unwrap();
        assert_eq!(candidate["id"],run);assert_eq!(candidate["sessionId"],"s");assert_eq!(candidate["sourceRevisionId"],frozen.id);
        assert_eq!(candidate["blocks"][0]["text"],expected_text);assert!(!candidate["reviewIssues"].as_array().unwrap().is_empty());
        assert_eq!(candidate["blocks"][0]["sourceRefs"],json!([frozen.items[0].id]));assert_unfinished(&h);
        let unreviewed=command(&h,"revision.adopt",json!({"sessionId":"s","expectedCurrentRevisionId":"","candidateId":run,"requestId":Uuid::new_v4().to_string()})).await.unwrap_err();
        assert_eq!(unreviewed.code,"candidate_review_required");assert_unfinished(&h);
        command(&h,"revision.adopt",json!({"sessionId":"s","expectedCurrentRevisionId":"","candidateId":run,"reviewedText":expected_text,"requestId":Uuid::new_v4().to_string()})).await.unwrap();
        let adopted=h.db.get_result("s").unwrap();assert_eq!(adopted.source,frozen);assert_eq!(adopted.output.unwrap().text(),expected_text);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),1);assert_eq!(usage_count(&h),1);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);assert!(view(&h).recovery_candidate_id.is_none());
    }
}

#[tokio::test]
async fn cancelling_after_a_locally_admitted_http_draft_does_not_expose_a_recovery_candidate() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().config.region=Some("beijing".into());
    let mut frozen=source("s");frozen.items.truncate(1);frozen.items[0].text="嗯，预算3500元，先不要发送。".into();h.checkpoint("s",7,frozen).unwrap();
    let processing=h.processing_lease("s",7);
    let server=Server::new(vec![Reply::draft(REVIEWABLE_DRAFT),Reply::new(ACCEPT).after(4000)]).await;let endpoint=server.endpoint.clone();
    let worker=h.clone();let task=tokio::spawn(async move {worker.finish_rewrite_with_recovery_policy("s".into(),7,move|config,mode,input,cancel,execution,context|async move {
        rewrite::rewrite_with_client_optional_context(&reqwest::Client::builder().no_proxy().build().unwrap(),&config,
            "synthetic-key",&mode,&input,&DictionarySnapshot::default(),cancel,&endpoint,execution,context.as_ref()).await
    },|_,_,_,_,_,_|async{panic!("cancelled primary cannot start recovery")},None::<RewriteWaitPolicy>,|_,_,_|panic!("cancelled draft cannot deliver")).await;});
    wait_until(||server.count()==2).await;assert!(view(&h).recovery_candidate_id.is_none());
    h.cancel_session_async(&identity("s")).await.unwrap();task.await.unwrap();drop(processing);
    assert_eq!(view(&h).state,SessionPhase::Cancelled);assert!(view(&h).recovery_candidate_id.is_none());
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),0);assert_eq!(usage_count(&h),0);
    assert!(h.inner.lock().unwrap().candidate.is_none());
}

#[tokio::test]
async fn permanent_failures_inside_correction_never_start_a_second_session_attempt(){
    let input="嗯，预算3500元，如果测试通过再上线。";
    let bad="预算3500元，直接上线。";
    let corrected="预算3500元，如果测试通过再上线。";
    for final_review in [false,true] {
        for (status,expected) in [(401,"authentication_failed"),(403,"authentication_failed"),
            (429,"rate_limited"),(400,"rewrite_request_rejected"),(404,"rewrite_request_rejected"),(200,"rewrite_refused")] {
            let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);
            h.inner.lock().unwrap().active.as_mut().unwrap().config.region=Some("beijing".into());
            let mut frozen=source("s");frozen.items.truncate(1);frozen.items[0].text=input.into();
            h.checkpoint("s",7,frozen.clone()).unwrap();
            let terminal=if status==200{Reply::refused()}else{Reply::new("private provider failure").http_status(status)};
            let mut replies=vec![Reply::draft(bad)];if final_review{replies.push(Reply::draft(corrected));}replies.push(terminal);
            let server=Server::new(replies).await;let endpoint=server.endpoint.clone();
            h.finish_rewrite_with_recovery_policy("s".into(),7,move|config,mode,source,cancel,execution,context|async move{
                rewrite::rewrite_with_client_optional_context(&reqwest::Client::builder().no_proxy().build().unwrap(),&config,
                    "synthetic-key",&mode,&source,&DictionarySnapshot::default(),cancel,&endpoint,execution,context.as_ref()).await
            },|_,_,_,_,_,_|async{panic!("a terminal correction/review failure cannot start automatic recovery")},
            None::<RewriteWaitPolicy>,|_,_,_|panic!("a failed pipeline cannot deliver")).await;
            assert_eq!(view(&h).error_code.as_deref(),Some(expected),"status={status}, final_review={final_review}");
            assert_unfinished(&h);assert_eq!(h.db.get_result("s").unwrap().source,frozen);
            assert_eq!(server.count(),if final_review{3}else{2});
            let diagnostics=h.diagnostics.snapshot().unwrap();
            assert!(diagnostics.traces[0].rewrite.as_ref().unwrap().automatic_recovery.is_none());
        }
    }
}

#[tokio::test]
async fn primary_and_recovery_http_requests_keep_separate_diagnostic_receipts() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().config.region=Some("beijing".into());
    let mut frozen=source("s");frozen.items.truncate(1);frozen.items[0].text="嗯，先核对需求再安排下一步。".into();h.checkpoint("s",7,frozen.clone()).unwrap();
    let server=Server::new(vec![Reply::draft("先核对需求再安排下一步。"),Reply::new(uncertain_review()),
        Reply::draft("先核对需求，再安排下一步。"),Reply::new(ACCEPT)]).await;
    let primary_endpoint=server.endpoint.clone();let recovery_endpoint=server.endpoint.clone();
    h.finish_rewrite_with_recovery_policy("s".into(),7,move|config,mode,input,cancel,execution,context|async move {
        rewrite::rewrite_with_client_optional_context(&reqwest::Client::builder().no_proxy().build().unwrap(),&config,
            "synthetic-secret",&mode,&input,&DictionarySnapshot::default(),cancel,&primary_endpoint,execution,context.as_ref()).await
    },move|config,mode,input,cancel,execution,context|async move {
        rewrite::minimum_pipeline::run_until_with_writing_context(&reqwest::Client::builder().no_proxy().build().unwrap(),
            &config,"synthetic-secret",&mode,&input,&DictionarySnapshot::default(),cancel,&recovery_endpoint,execution,&context).await
    },None::<RewriteWaitPolicy>,|_,_,_|{}).await;
    assert_eq!(server.count(),4);
    let diagnostics=h.diagnostics.snapshot().unwrap();let rewrite=diagnostics.traces[0].rewrite.as_ref().unwrap();
    assert_eq!(rewrite.requests.len(),4);assert!(rewrite.requests.iter().all(|r|r.end_ms.is_some()));
    assert!(rewrite.automatic_recovery.as_ref().unwrap().validated);
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().rewrite_recovery.unwrap().kind,RewriteRecoveryKind::AutomaticRewrite);
    let saved=h.db.get_result("s").unwrap();assert_eq!(saved.source,frozen);assert_eq!(saved.output.unwrap().text(),"先核对需求，再安排下一步。");
    assert!(view(&h).recovery_candidate_id.is_none(),"successful recovery cannot expose an earlier reviewable draft");
}

#[tokio::test(start_paused=true)]
async fn automatic_recovery_timeout_cancels_child_and_finishes_without_another_attempt() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
    let captured=Arc::new(Mutex::new(None));let slot=captured.clone();
    h.finish_rewrite_with_recovery_policy("s".into(),7,|_,_,_,_,_,_|async {
        Err(CloudError::new("rewrite_timeout","rewrite","synthetic failure"))
    },move|_,_,_,cancel,_,_|async move {
        *slot.lock().unwrap()=Some(cancel);
        std::future::pending::<std::result::Result<rewrite::RewriteResult,CloudError>>().await
    },None::<RewriteWaitPolicy>,|_,_,_|{}).await;
    assert!(captured.lock().unwrap().as_ref().unwrap().is_cancelled());
    assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled());
    assert_unfinished(&h);
}

#[tokio::test]
async fn primary_failure_runs_complete_automatic_recovery_before_single_commit_and_delivery() {
    for failure in ["rewrite_timeout", "invalid_response", "rewrite_semantic_rejected", "rewrite_network"] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);
        h.inner.lock().unwrap().active.as_mut().unwrap().config.region=Some("beijing".into());
        if failure=="rewrite_semantic_rejected" {
            let custom=h.db.save_mode(ModeDraft{id:None,name:"测试简报".into(),base_mode:BaseMode::Structure,
                rules:"保留用户自定义规则的快照".into(),expected_revision:None}).unwrap();
            let selected=h.db.mode_snapshot(&custom.id,None).unwrap();
            h.inner.lock().unwrap().active.as_mut().unwrap().mode=selected;
        }
        let selected=h.inner.lock().unwrap().active.as_ref().unwrap().mode.clone();
        let mut frozen=source("s");frozen.items.truncate(1);
        // Long enough to exercise the independent 60s recovery budget.
        frozen.items[0].text=format!("嗯，预算3500元，先不要发送。{}", "我们会在核对需求后安排下一步。".repeat(65));
        h.checkpoint("s",7,frozen.clone()).unwrap();
        let expected=frozen.text().trim_start_matches("嗯，").to_owned();
        let server=Server::new(vec![Reply::new(json!({"resolvedMode":if selected.base_mode==BaseMode::Structure{"structure"}else{"light"},"text":expected}).to_string()),Reply::new(ACCEPT)]).await;
        let recovery_selection=selected.clone();
        let endpoint=server.endpoint.clone();let worker=h.clone();
        let delivered=Arc::new(std::sync::atomic::AtomicUsize::new(0));let receipt=delivered.clone();
        h.finish_rewrite_with_recovery_policy("s".into(),7,move|_,_,_,_,execution,_|async move {
            if failure=="rewrite_timeout" {tokio::time::sleep_until(execution.deadline).await;}
            Err(CloudError::new(failure,"rewrite","synthetic primary failure"))
        },move|config,mode,input,cancel,execution,context|async move {
            assert_eq!(worker.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),0);
            assert_eq!(mode,recovery_selection);
            assert_eq!(context.scene,AppWritingScene::General);assert!(context.input.is_none()&&context.personalization.is_none());
            assert!(!cancel.is_cancelled());assert!(execution.deadline>tokio::time::Instant::now()+Duration::from_secs(59));
            rewrite::minimum_pipeline::run_until_with_writing_context(&reqwest::Client::builder().no_proxy().build().unwrap(),
                &config,"synthetic-secret",&mode,&input,&DictionarySnapshot::default(),cancel,&endpoint,execution,&context).await
        },RewriteWaitPolicy{deadline:Duration::from_millis(40)},move|_,_,_|{receipt.fetch_add(1,Ordering::SeqCst);}).await;
        let saved=h.db.get_result("s").unwrap();let output=saved.output.unwrap();
        assert_eq!(saved.source,frozen);assert_eq!(output.text(),expected);
        assert_eq!(output.rewrite_recovery.unwrap().kind,RewriteRecoveryKind::AutomaticRewrite);
        assert_eq!(output.mode_snapshot,selected);assert_eq!(server.count(),2);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),1);assert_eq!(delivered.load(Ordering::SeqCst),1);
        let diagnostics=h.diagnostics.snapshot().unwrap();let view=diagnostics.traces[0].rewrite.as_ref().unwrap();
        assert_eq!(view.error_code,Some(failure));assert_eq!(view.requests.len(),2);
        let recovery=view.automatic_recovery.as_ref().unwrap();assert!(recovery.validated);assert_eq!(recovery.configured_budget_ms,60000);
        assert_eq!(output.writing_receipt.unwrap().scene,AppWritingScene::General);
    }
}

#[tokio::test]
async fn recovery_rejection_never_commits_unsafe_draft_or_retries_again() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().config.region=Some("beijing".into());
    let mut frozen=source("s");frozen.items.truncate(1);frozen.items[0].text="嗯，预算3500元，先不要发送。".into();
    h.checkpoint("s",7,frozen.clone()).unwrap();
    let server=Server::new(vec![Reply::draft("预算8500元，现在发送。")]).await;let endpoint=server.endpoint.clone();
    h.finish_rewrite_with_recovery_policy("s".into(),7,|_,_,_,_,_,_|async {
        Err(CloudError::new("invalid_response","rewrite","synthetic failure"))
    },move|config,mode,input,cancel,execution,context|async move {
        rewrite::minimum_pipeline::run_until_with_writing_context(&reqwest::Client::builder().no_proxy().build().unwrap(),
            &config,"synthetic-secret",&mode,&input,&DictionarySnapshot::default(),cancel,&endpoint,execution,&context).await
    },None::<RewriteWaitPolicy>,|_,_,_|{}).await;
    assert_unfinished(&h);
    assert_eq!(server.count(),1);
    assert!(!h.diagnostics.snapshot().unwrap().traces[0].rewrite.as_ref().unwrap().automatic_recovery.as_ref().unwrap().validated);
}

#[tokio::test]
async fn cancellation_or_replacement_during_automatic_recovery_cannot_save_or_deliver() {
    for replace in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
        let (began_tx,began_rx)=oneshot::channel();let worker=h.clone();
        let job=tokio::spawn(async move {worker.finish_rewrite_with_recovery_policy("s".into(),7,|_,_,_,_,_,_|async {
            Err(CloudError::new("rewrite_timeout","rewrite","synthetic failure"))
        },move|_,_,_,_,_,_|async move {
            began_tx.send(()).unwrap();std::future::pending::<std::result::Result<rewrite::RewriteResult,CloudError>>().await
        },None::<RewriteWaitPolicy>,|_,_,_|panic!("cancelled recovery cannot deliver")).await;});
        began_rx.await.unwrap();
        if replace {h.begin_cancel(&identity("s")).unwrap();install(&h,"new",SessionPhase::Recording,true);}
        else {h.begin_cancel(&identity("s")).unwrap();}
        job.await.unwrap();assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),0);assert_eq!(usage_count(&h),0);
    }
}

#[test]
fn recovery_budget_preserves_the_frozen_language_scene_and_context() {
    let mut input=source("s");input.items.truncate(1);
    for (chars,seconds) in [(800,30),(801,60),(3000,60),(3001,120)] {
        input.items[0].text="字".repeat(chars);
        assert_eq!(rewrite_fallback::automatic_wait(&input),Duration::from_secs(seconds));
    }
    let context=WritingContext{scene:AppWritingScene::Email,language:DraftLanguage::English,
        input:Some(InputContext{before_text:"nearby host text".into(),..Default::default()}),personalization:None};
    let recovery=rewrite_fallback::automatic_context(Some(&context));
    assert_eq!(recovery.language,DraftLanguage::English);assert_eq!(recovery.scene,AppWritingScene::Email);assert_eq!(recovery.input,context.input);
}

#[tokio::test]
async fn polishing_failure_or_timeout_cannot_deliver_fidelity_only_draft() {
    let original = "嗯，方案目前还没有太能做到。预算的话是3500元，先不要发送。";
    let draft = "方案目前还没有太能做到。预算3500元，先不要发送。";
    for failure in ["invalidRepair", "timeout", "invalidFinalReview", "changedMeaning"] {
        let h = hub();
        install(&h, "s", SessionPhase::FinalizingAsr, true);
        h.inner.lock().unwrap().active.as_mut().unwrap().config.region = Some("beijing".into());
        let mut frozen = source("s");
        frozen.items.truncate(1);
        frozen.items[0].text = original.into();
        h.checkpoint("s", 7, frozen.clone()).unwrap();
        let review = rejection("方案目前还没有太能做到。", "方案目前还没有太能做到。", "cleanup");
        let repair = match failure {
            "timeout" => Reply::new("{}").after(4000),
            "invalidFinalReview" => Reply::new(serde_json::json!({"replacements":[{"id":"r0","text":"方案目前还没有很好地做到。"}]}).to_string()),
            "changedMeaning" => Reply::new(serde_json::json!({"replacements":[{"id":"r0","text":"方案目前已经做好了。"}]}).to_string()),
            _ => Reply::new("invalid json"),
        };
        let server = Server::new(vec![Reply::draft(draft), Reply::new(review), repair, Reply::new("invalid final review")]).await;
        let endpoint = server.endpoint.clone();
        h.finish_rewrite_with_policy("s".into(), 7, move |config, mode, source, cancel, execution| async move {
            rewrite::rewrite_with_client_optional_context(&reqwest::Client::new(), &config, "synthetic-secret",
                &mode, &source, &DictionarySnapshot::default(), cancel, &endpoint, execution, None).await
        }, RewriteWaitPolicy { deadline: Duration::from_millis(1800) }, |_,_,_| {}).await;

        let detail = h.db.get_result("s").unwrap();
        assert_eq!(detail.source, frozen);
        assert_unfinished(&h);
        assert_eq!(server.count(), if failure == "invalidFinalReview" {4} else {3});
    }
}

#[tokio::test]
async fn uncertain_content_and_cancelled_polishing_cannot_commit() {
    let original = "嗯，方案目前还没有太能做到。预算的话是3500元，先不要发送。";
    let draft = "方案目前还没有太能做到。预算3500元，先不要发送。";
    for cancel_after_review in [false, true] {
        let h = hub();
        install(&h, "s", SessionPhase::FinalizingAsr, true);
        h.inner.lock().unwrap().active.as_mut().unwrap().config.region = Some("beijing".into());
        let mut frozen = source("s");
        frozen.items.truncate(1);
        frozen.items[0].text = original.into();
        h.checkpoint("s", 7, frozen.clone()).unwrap();
        let mut review: serde_json::Value = serde_json::from_str(&rejection(
            "方案目前还没有太能做到。", "方案目前还没有太能做到。", "cleanup")).unwrap();
        if !cancel_after_review {
            review["checks"]["facts"] = serde_json::json!("uncertain");
            review["issues"] = serde_json::json!([]);
        }
        let server = Server::new(vec![Reply::draft(draft), Reply::new(review.to_string()), Reply::new("{}").after(4000)]).await;
        let endpoint = server.endpoint.clone();
        let worker = h.clone();
        let task = tokio::spawn(async move {
            worker.finish_rewrite_with_policy("s".into(), 7, move |config, mode, source, cancel, execution| async move {
                rewrite::rewrite_with_client_optional_context(&reqwest::Client::new(), &config, "synthetic-secret",
                    &mode, &source, &DictionarySnapshot::default(), cancel, &endpoint, execution, None).await
            }, RewriteWaitPolicy { deadline: Duration::from_secs(3) }, |_,_,_| {}).await;
        });
        if cancel_after_review {
            wait_until(||server.count() == 3).await;
            h.begin_cancel(&identity("s")).unwrap();
        }
        task.await.unwrap();
        if cancel_after_review {
            assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(), 0);
        } else {
            assert_unfinished(&h);
            assert_eq!(server.count(), 2);
        }
    }
}

#[tokio::test]
async fn explicit_retry_can_finish_automatic_recovery_without_renewing_input_permission() {
    for revoked in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
        {let mut inner=h.inner.lock().unwrap();let session=inner.active.as_mut().unwrap();
            session.reason=CreationReason::Retry;session.original_delivery_revoked=revoked;
            session.prefs.delivery_mode=DeliveryMode::Direct;session.target=Some(delivery_target_fixture());
            session.config.region=Some("beijing".into());}
        let frozen=h.db.get_result("s").unwrap().source;
        let server=Server::new(vec![Reply::draft(&frozen.text()),Reply::new(ACCEPT)]).await;let endpoint=server.endpoint.clone();
        h.finish_rewrite_with_recovery_policy("s".into(),7,|_,_,_,_,_,_|async {
            Err(CloudError::new("rewrite_timeout","rewrite","synthetic retry failure"))
        },move|config,mode,source,cancel,execution,context|async move {
            rewrite::minimum_pipeline::run_until_with_writing_context(&reqwest::Client::builder().no_proxy().build().unwrap(),
                &config,"synthetic",&mode,&source,&DictionarySnapshot::default(),cancel,&endpoint,execution,&context).await
        },None::<RewriteWaitPolicy>,|h,id,generation|h.initial_delivery_with(id,generation,
            |_|panic!("retry cannot validate an input target"),|_,_,_,_|panic!("retry cannot inject input"))).await;
        let output=h.db.get_result("s").unwrap().output.unwrap();
        assert_eq!(output.creation_reason,CreationReason::Retry);
        assert_eq!(output.rewrite_recovery.unwrap().kind,RewriteRecoveryKind::AutomaticRewrite);
        assert_eq!(server.count(),2);assert_eq!(usage_count(&h),1);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    }
}

#[tokio::test]
async fn final_recovery_save_failure_can_retry_save_without_regenerating_or_delivering() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
    fault(&h,"CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'synthetic save failure'); END;");
    h.finish_rewrite_with_recovery_policy("s".into(),7,|_,_,_,_,_,_|async {
        Err(CloudError::new("rewrite_timeout","rewrite","synthetic failure"))
    },|_,_,source,_,_,_|async move {Ok(rewrite_fallback::source_fallback(&source))},None::<RewriteWaitPolicy>,
        |_,_,_|panic!("uncommitted result cannot deliver")).await;
    assert_unfinished(&h);assert!(view(&h).can_retry_save);
    let mut config=h.db.snapshot().provider_config;let version=config.version;config.region=Some("beijing".into());
    h.db.submit(DbCommand::SaveProviderConfig{config,expected_version:version},None,DbPriority::Normal).unwrap().wait_blocking().unwrap();
    h.inner.lock().unwrap().key_configured=true;
    let pending=view(&h).pending_commit_id;
    h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();s.phase=SessionPhase::Ready;s.processing_worker_released=true;}
    h.activate_recovery(&identity("s")).unwrap();
    assert_eq!(view(&h).pending_commit_id,pending);
    fault(&h,"DROP TRIGGER fail_usage;");let mut payload=identity("s");payload["pendingCommitId"]=json!(view(&h).pending_commit_id.unwrap());
    h.retry_save(&payload).await.unwrap();
    assert_eq!(view(&h).state,SessionPhase::Ready);assert_eq!(usage_count(&h),1);
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().rewrite_recovery.unwrap().kind,RewriteRecoveryKind::AutomaticRewrite);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}

#[tokio::test]
async fn failed_final_rewrite_is_recoverable_after_starting_another_recording() {
    for keep_history in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);
        h.inner.lock().unwrap().active.as_mut().unwrap().prefs.keep_history=keep_history;
        checkpoint(&h,"s");h.inner.lock().unwrap().active.as_mut().unwrap().phase=SessionPhase::Failed;
        let mut config=h.db.snapshot().provider_config;let expected=config.version;config.region=Some("beijing".into());
        h.db.submit(DbCommand::SaveProviderConfig{config,expected_version:expected},None,DbPriority::Normal).unwrap().wait_blocking().unwrap();
        h.inner.lock().unwrap().key_configured=true;
        h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
        {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();s.phase=SessionPhase::Ready;s.processing_worker_released=true;}
        let restored=h.activate_recovery(&identity("s")).unwrap();
        assert_eq!(restored["canRetryRewrite"],true);assert_eq!(restored["generation"],7);
        h.use_source_async(&identity("s")).await.unwrap();
        assert_eq!(view(&h).state,SessionPhase::Ready);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
        assert!(h.prepare_start_state(Instant::now(),Origin::UiCommand).is_ok());
    }
}
