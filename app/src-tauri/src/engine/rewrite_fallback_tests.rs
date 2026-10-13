//! Actual coordinator/SQLite behavior with synthetic provider futures and native IO seams.
use super::*;

fn budget_source(text: String) -> SourceRevision {
    let mut value=source("s");value.items.truncate(1);value.items[0].text=text;value
}

#[test]
fn initial_rewrite_wait_budget_uses_adaptive_unicode_characters_not_utf8_bytes() {
    for character in ["a","轻","🎙"] {
        for (count,seconds) in [(400,5),(401,8),(800,8),(801,20)] {
            let frozen=budget_source(character.repeat(count));
            assert_eq!(frozen.text().chars().count(),count);
            assert_eq!(RewriteWaitPolicy::for_source(CreationReason::NormalInitial,&frozen).deadline,
                Duration::from_secs(seconds),"{character}: {count} characters");
        }
    }
    // Separators are part of the actual source body sent for rewriting.
    let mut segmented=source("s");segmented.items[0].text="轻".repeat(399);segmented.items[1].text="语".repeat(400);
    assert_eq!(segmented.text().chars().count(),800);
    assert_eq!(RewriteWaitPolicy::for_source(CreationReason::NormalInitial,&segmented).deadline,Duration::from_secs(8));
    segmented.items[1].text.push('音');
    assert_eq!(RewriteWaitPolicy::for_source(CreationReason::NormalInitial,&segmented).deadline,Duration::from_secs(20));
}

#[test]
fn explicit_rewrite_attempts_keep_twenty_seconds_even_for_short_multibyte_source() {
    for reason in [CreationReason::Retry,CreationReason::History,CreationReason::UseSource,CreationReason::Preview] {
        for count in [1,800,801] {
            assert_eq!(RewriteWaitPolicy::for_source(reason,&budget_source("语".repeat(count))).deadline,
                Duration::from_secs(20),"{reason:?}: {count} characters");
        }
    }
    assert_eq!(RewriteWaitPolicy::default().deadline,Duration::from_secs(20));
}

#[tokio::test(start_paused=true)]
async fn production_short_rewrite_budget_falls_back_at_five_seconds_without_second_generation() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
    let native=delivery_native_calls(&h);
    let captured=Arc::new(Mutex::new(None));let slot=captured.clone();
    let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));let invoked=calls.clone();
    let completed=Arc::new(std::sync::atomic::AtomicUsize::new(0));let returned=completed.clone();
    let began=tokio::time::Instant::now();
    // Exercise the production wrapper (no injected deadline). The target is absent,
    // so delivery records a skip without touching host input or the clipboard.
    h.finish_rewrite_with("s".into(),7,move|_,_,frozen,cancel|async move {
        invoked.fetch_add(1,Ordering::SeqCst);*slot.lock().unwrap()=Some(cancel);
        tokio::time::sleep(Duration::from_secs(6)).await;
        returned.fetch_add(1,Ordering::SeqCst);
        Ok(rewrite_fallback::source_fallback(&frozen))
    }).await;
    assert!(began.elapsed()>=Duration::from_secs(5)&&began.elapsed()<Duration::from_secs(6),"short rewrite budget: {:?}",began.elapsed());
    assert!(captured.lock().unwrap().as_ref().unwrap().is_cancelled());
    assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled());
    assert_eq!(calls.load(Ordering::SeqCst),1);assert_eq!(completed.load(Ordering::SeqCst),0);
    let saved=h.db.get_result("s").unwrap().output.unwrap();
    assert_eq!(saved.rewrite_fallback,Some(RewriteFallbackReason::Timeout));assert_eq!(saved.text(),source("s").text());
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),1);
    tokio::time::resume();
    wait_until(||h.inner.lock().unwrap().active.as_ref().unwrap().delivery_worker_released).await;
    assert_eq!(view(&h).delivery_state,Some(DeliveryState::Skipped));
    assert!(!native.lock().unwrap().iter().any(|(name,_)|name=="showMain"));
}

async fn fail_rewrite(h:&Arc<AppHub>,code:&'static str) {
    h.finish_rewrite_with_policy("s".into(),7,move|_,_,_,_|async move {
        Err(CloudError::new(code,"rewrite","synthetic failure"))
    },RewriteWaitPolicy::default(),|hub,_,_| {
        hub.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;
    }).await;
}

#[tokio::test]
async fn rewrite_failures_deliver_source_exactly_once_and_never_open_main() {
    for(code,reason)in [
        ("rewrite_timeout",RewriteFallbackReason::Timeout),("rewrite_network",RewriteFallbackReason::ServiceUnavailable),
        ("invalid_output",RewriteFallbackReason::InvalidOutput),("invalid_response",RewriteFallbackReason::InvalidOutput),
        ("invalid_local_repair",RewriteFallbackReason::InvalidOutput),
        ("rewrite_meaning_risk",RewriteFallbackReason::MeaningRisk),
        ("rewrite_admission_rejected",RewriteFallbackReason::MeaningRisk),
        ("rewrite_semantic_rejected",RewriteFallbackReason::MeaningRisk),
    ] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);checkpoint(&h,"s");enable_fixture_delivery(&h);
        let calls=delivery_native_calls(&h);fail_rewrite(&h,code).await;
        let saved=h.db.get_result("s").unwrap();let output=saved.output.unwrap();
        assert_eq!(saved.source,source("s"));assert_eq!(output.text(),saved.source.text());assert_eq!(output.rewrite_fallback,Some(reason));
        assert_eq!(output.creation_reason,CreationReason::NormalInitial);assert_eq!(view(&h).rewrite_fallback,Some(reason));
        let diagnostics=h.diagnostics.snapshot().unwrap();let trace=&diagnostics.traces[0];
        let rewrite=trace.rewrite.as_ref().unwrap();
        assert_eq!(rewrite.error_code,Some(code));assert_eq!(rewrite.fallback_reason,Some(reason));
        assert!(!trace.marks.iter().any(|mark|mark.stage==Stage::RewriteValidated));
        let sent=std::sync::atomic::AtomicUsize::new(0);
        h.initial_delivery_with("s",7,|_|true,|permit,target,text,gate| {
            assert_eq!(text,source("s").text());
            platform::delivery::dispatch_with_test_operations(permit,target,text,gate,|_|true,||{sent.fetch_add(1,Ordering::SeqCst);4})
        });
        h.initial_delivery_with("s",7,|_|panic!("terminal delivery must not validate again"),|_,_,_,_|panic!("must not duplicate input"));
        assert_eq!(sent.load(Ordering::SeqCst),1);assert_eq!(view(&h).delivery_state,Some(DeliveryState::Unconfirmed));
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),1);assert_eq!(usage_count(&h),1);
        assert!(!calls.lock().unwrap().iter().any(|(name,_)|name=="showMain"));
    }
}

#[tokio::test]
async fn rewrite_deadline_drops_late_candidate_and_cancels_only_its_child() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
    let captured=Arc::new(Mutex::new(None));let slot=captured.clone();
    h.finish_rewrite_with_policy("s".into(),7,move|_,_,_,cancel|async move {
        *slot.lock().unwrap()=Some(cancel);
        std::future::pending::<std::result::Result<rewrite::RewriteResult,CloudError>>().await
    },RewriteWaitPolicy{deadline:Duration::from_millis(10)},|hub,_,_|hub.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true).await;
    assert!(captured.lock().unwrap().as_ref().unwrap().is_cancelled());
    assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled());
    assert_eq!(view(&h).state,SessionPhase::Ready);assert_eq!(view(&h).rewrite_fallback,Some(RewriteFallbackReason::Timeout));
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),1);
    assert_eq!(RewriteWaitPolicy::default().deadline,Duration::from_secs(20));
}

#[tokio::test]
async fn synchronous_validation_finishing_after_deadline_cannot_commit_its_candidate() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
    let captured=Arc::new(Mutex::new(None));let slot=captured.clone();
    h.finish_rewrite_with_policy("s".into(),7,move|_,_,frozen,cancel|async move {
        *slot.lock().unwrap()=Some(cancel);
        // Simulates synchronous JSON/semantic checking: no yield to Tokio's timer.
        std::thread::sleep(Duration::from_millis(30));
        let mut draft=rewrite_fallback::source_fallback(&frozen);
        draft.blocks[0].text="late synthetic candidate".into();Ok(draft)
    },RewriteWaitPolicy{deadline:Duration::from_millis(5)},|hub,_,_|hub.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true).await;
    let saved=h.db.get_result("s").unwrap().output.unwrap();
    assert_eq!(saved.rewrite_fallback,Some(RewriteFallbackReason::Timeout));
    assert_eq!(saved.text(),source("s").text());
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),1);
    assert!(captured.lock().unwrap().as_ref().unwrap().is_cancelled());
}

#[tokio::test]
async fn rewrite_cancel_or_session_replacement_cannot_create_fallback() {
    for replace in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
        let (began_tx,began_rx)=oneshot::channel();let (release_tx,release_rx)=oneshot::channel();let worker=h.clone();
        let job=tokio::spawn(async move {worker.finish_rewrite_with_policy("s".into(),7,move|_,_,_,_|async move {
            began_tx.send(()).unwrap();let _=release_rx.await;Err(CloudError::new("rewrite_network","rewrite","late failure"))
        },RewriteWaitPolicy::default(),|_,_,_|panic!("cancelled/replaced session cannot deliver")).await;});
        began_rx.await.unwrap();
        if replace {install(&h,"new",SessionPhase::Recording,true);}else {h.cancel_session_async(&identity("s")).await.unwrap();}
        let _=release_tx.send(());job.await.unwrap();
        assert_eq!(usage_count(&h),0);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),0);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    }
}

#[tokio::test]
async fn rewrite_invalid_source_pending_save_and_noninitial_attempts_do_not_fallback() {
    for case in ["incomplete","pending","retry","history","revoked","invalid_source","cancelled"] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
        {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();match case {
            "incomplete"=>s.source.as_mut().unwrap().complete=false,
            "pending"=>s.db_pending=1,
            "retry"=>s.reason=CreationReason::Retry,
            "history"=>s.reason=CreationReason::History,
            "revoked"=>s.original_delivery_revoked=true,
            _=>{},
        }}
        fail_rewrite(&h,match case{"invalid_source"=>"invalid_source","cancelled"=>"cancelled",_=>"rewrite_network"}).await;
        assert_eq!(view(&h).state,SessionPhase::Failed,"{case}");assert!(view(&h).rewrite_fallback.is_none());
        assert_eq!(usage_count(&h),0);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    }
}

#[tokio::test]
async fn fallback_save_failure_preserves_retryable_output_without_delivery() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
    fault(&h,"CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'synthetic save failure'); END;");
    fail_rewrite(&h,"rewrite_timeout").await;
    assert_eq!(view(&h).state,SessionPhase::Failed);assert!(view(&h).can_retry_save);
    assert_eq!(usage_count(&h),0);assert!(h.db.get_result("s").unwrap().output.is_none());
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    fault(&h,"DROP TRIGGER fail_usage;");let mut payload=identity("s");payload["pendingCommitId"]=json!(view(&h).pending_commit_id.unwrap());
    h.retry_save(&payload).await.unwrap();
    assert_eq!(view(&h).state,SessionPhase::Ready);assert_eq!(view(&h).rewrite_fallback,Some(RewriteFallbackReason::Timeout));
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().creation_reason,CreationReason::Retry);
    assert_eq!(usage_count(&h),1);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}

#[tokio::test]
async fn fallback_missing_target_and_preview_preserve_delivery_preferences() {
    for preview in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
        if preview {h.inner.lock().unwrap().active.as_mut().unwrap().prefs.delivery_mode=DeliveryMode::Preview;}
        let calls=delivery_native_calls(&h);fail_rewrite(&h,"rewrite_timeout").await;
        h.initial_delivery_with("s",7,|_|panic!("no target"),|_,_,_,_|panic!("no automatic input"));
        assert_eq!(view(&h).state,SessionPhase::Ready);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),u64::from(!preview));
        assert_eq!(calls.lock().unwrap().iter().any(|(name,_)|name=="showMain"),preview);
    }
}

#[tokio::test]
async fn fallback_persists_across_database_reopen_and_has_no_success_validation_mark() {
    let(h,path)=file_hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);checkpoint(&h,"s");
    fail_rewrite(&h,"rewrite_timeout").await;
    let trace=h.diagnostics.snapshot().unwrap();assert!(!trace.traces[0].marks.iter().any(|m|m.stage==Stage::RewriteValidated));
    assert!(trace.traces[0].marks.iter().any(|m|m.stage==Stage::RewriteFallbackSelected));
    h.shutdown_ready().await.unwrap();let reopened=Store::open(&path).unwrap();
    let output=reopened.get_result("s").unwrap().output.unwrap();assert_eq!(output.rewrite_fallback,Some(RewriteFallbackReason::Timeout));
    assert_eq!(output.text(),source("s").text());assert_eq!(reopened.usage_today("2026-10-04").unwrap().count,1);
}
