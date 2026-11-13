use super::*;
use super::rewrite_fallback_tests::assert_unfinished;
use crate::cloud::rewrite::minimum_pipeline::tests::{Server, Reply, rejection};

const ACCEPT: &str = r#"{"decision":"accept","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"pass"},"issues":[]}"#;

#[tokio::test]
async fn primary_and_recovery_http_requests_keep_separate_diagnostic_receipts() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().config.region=Some("beijing".into());
    let mut frozen=source("s");frozen.items.truncate(1);frozen.items[0].text="嗯，先核对需求再安排下一步。".into();h.checkpoint("s",7,frozen).unwrap();
    let server=Server::new(vec![Reply::new("invalid json"),Reply::draft("先核对需求再安排下一步。"),Reply::new(ACCEPT)]).await;
    let primary_endpoint=server.endpoint.clone();let recovery_endpoint=server.endpoint.clone();
    h.finish_rewrite_with_recovery_policy("s".into(),7,move|config,mode,input,cancel,execution,context|async move {
        rewrite::rewrite_with_client_optional_context(&reqwest::Client::builder().no_proxy().build().unwrap(),&config,
            "synthetic-secret",&mode,&input,&DictionarySnapshot::default(),cancel,&primary_endpoint,execution,context.as_ref()).await
    },move|config,mode,input,cancel,execution,context|async move {
        rewrite::minimum_pipeline::run_until_with_writing_context(&reqwest::Client::builder().no_proxy().build().unwrap(),
            &config,"synthetic-secret",&mode,&input,&DictionarySnapshot::default(),cancel,&recovery_endpoint,execution,&context).await
    },None::<RewriteWaitPolicy>,|_,_,_|{}).await;
    assert_eq!(server.count(),3);
    let diagnostics=h.diagnostics.snapshot().unwrap();let rewrite=diagnostics.traces[0].rewrite.as_ref().unwrap();
    assert_eq!(rewrite.requests.len(),3);assert!(rewrite.requests.iter().all(|r|r.end_ms.is_some()));
    assert!(rewrite.automatic_recovery.as_ref().unwrap().validated);
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().rewrite_recovery.unwrap().kind,RewriteRecoveryKind::AutomaticRewrite);
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
        let server=Server::new(vec![Reply::draft(&expected),Reply::new(ACCEPT)]).await;
        let endpoint=server.endpoint.clone();let worker=h.clone();
        let delivered=Arc::new(std::sync::atomic::AtomicUsize::new(0));let receipt=delivered.clone();
        h.finish_rewrite_with_recovery_policy("s".into(),7,move|_,_,_,_,execution,_|async move {
            if failure=="rewrite_timeout" {tokio::time::sleep_until(execution.deadline).await;}
            Err(CloudError::new(failure,"rewrite","synthetic primary failure"))
        },move|config,mode,input,cancel,execution,context|async move {
            assert_eq!(worker.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),0);
            assert_eq!(mode.mode_id,"auto");assert!(mode.rules.is_empty());
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
fn recovery_budget_and_language_are_frozen_independently_of_failed_context() {
    let mut input=source("s");input.items.truncate(1);
    for (chars,seconds) in [(800,30),(801,60),(3000,60),(3001,120)] {
        input.items[0].text="字".repeat(chars);
        assert_eq!(rewrite_fallback::automatic_wait(&input),Duration::from_secs(seconds));
    }
    let context=WritingContext{scene:AppWritingScene::Email,language:DraftLanguage::English,
        input:Some(InputContext{before_text:"nearby host text".into(),..Default::default()}),personalization:None};
    let recovery=rewrite_fallback::automatic_context(Some(&context));
    assert_eq!(recovery.language,DraftLanguage::English);assert_eq!(recovery.scene,AppWritingScene::General);assert!(recovery.input.is_none());
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
