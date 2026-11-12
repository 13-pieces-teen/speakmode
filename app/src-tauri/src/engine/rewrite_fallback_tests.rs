//! Actual coordinator/SQLite behavior with synthetic provider futures and native IO seams.
use super::*;

fn budget_source(text: String) -> SourceRevision {
    let mut value=source("s");value.items.truncate(1);value.items[0].text=text;value
}

#[test]
fn initial_rewrite_wait_budget_uses_adaptive_unicode_characters_not_utf8_bytes() {
    for character in ["a","轻","🎙"] {
        for (count,seconds) in [(400,20),(401,20),(800,20),(801,30),(3000,30),(3001,60)] {
            let frozen=budget_source(character.repeat(count));
            assert_eq!(frozen.text().chars().count(),count);
            assert_eq!(RewriteWaitPolicy::for_source(CreationReason::NormalInitial,&frozen).deadline,
                Duration::from_secs(seconds),"{character}: {count} characters");
        }
    }
    // Separators are part of the actual source body sent for rewriting.
    let mut segmented=source("s");segmented.items[0].text="轻".repeat(399);segmented.items[1].text="语".repeat(400);
    assert_eq!(segmented.text().chars().count(),800);
    assert_eq!(RewriteWaitPolicy::for_source(CreationReason::NormalInitial,&segmented).deadline,Duration::from_secs(20));
    segmented.items[1].text.push('音');
    assert_eq!(RewriteWaitPolicy::for_source(CreationReason::NormalInitial,&segmented).deadline,Duration::from_secs(30));
}

#[test]
fn explicit_rewrite_attempts_allow_full_source_length_budget() {
    for reason in [CreationReason::Retry,CreationReason::History,CreationReason::UseSource,CreationReason::Preview] {
        for (count,seconds) in [(1,20),(800,20),(801,30),(3001,60)] {
            assert_eq!(RewriteWaitPolicy::for_source(reason,&budget_source("语".repeat(count))).deadline,
                Duration::from_secs(seconds),"{reason:?}: {count} characters");
        }
    }
    assert_eq!(RewriteWaitPolicy::default().deadline,Duration::from_secs(20));
}

pub(super) fn assert_unfinished(h:&Arc<AppHub>) {
    assert_eq!(view(h).state,SessionPhase::Failed);
    assert!(h.db.get_result("s").unwrap().output.is_none());
    assert_eq!(usage_count(h),0);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}

async fn fail_rewrite(h:&Arc<AppHub>,code:&'static str) {
    h.finish_rewrite_with_policy("s".into(),7,move|_,_,_,_,_|async move {
        Err(CloudError::new(code,"rewrite","synthetic failure"))
    },RewriteWaitPolicy{deadline:Duration::from_millis(10)},|_,_,_|panic!("unfinished result cannot deliver")).await;
}

#[tokio::test]
async fn failed_finalization_preserves_source_without_output_usage_or_delivery() {
    for code in ["rewrite_timeout","rewrite_network","invalid_response","rewrite_meaning_risk","rewrite_semantic_rejected"] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);checkpoint(&h,"s");
        fail_rewrite(&h,code).await;
        assert_unfinished(&h);
        assert_eq!(h.db.get_result("s").unwrap().source,source("s"));
        assert!(view(&h).rewrite_recovery.is_none());
        let diagnostics=h.diagnostics.snapshot().unwrap();
        let rewrite=diagnostics.traces[0].rewrite.as_ref().unwrap();
        assert_eq!(rewrite.automatic_recovery.as_ref().unwrap().error_code,Some("rewrite_network"));
        assert!(!diagnostics.traces[0].marks.iter().any(|mark|mark.stage==Stage::ReadyCommitted));
    }
}

#[tokio::test(start_paused=true)]
async fn production_budget_allows_complete_six_second_result_instead_of_basic_cleanup() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
    h.finish_rewrite_with_policy("s".into(),7,|_,_,source,_,_|async move {
        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok(rewrite_fallback::source_fallback(&source))
    },None::<RewriteWaitPolicy>,|_,_,_|{}).await;
    assert_eq!(view(&h).state,SessionPhase::Ready);
    assert!(h.db.get_result("s").unwrap().output.unwrap().rewrite_recovery.is_none());
}

#[tokio::test]
async fn late_validation_cannot_turn_unfinished_draft_into_success() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
    let captured=Arc::new(Mutex::new(None));let slot=captured.clone();
    h.finish_rewrite_with_policy("s".into(),7,move|_,_,source,cancel,_|async move {
        *slot.lock().unwrap()=Some(cancel);
        std::thread::sleep(Duration::from_millis(30));
        Ok(rewrite_fallback::source_fallback(&source))
    },RewriteWaitPolicy{deadline:Duration::from_millis(5)},|_,_,_|panic!("late draft cannot deliver")).await;
    assert_unfinished(&h);assert!(captured.lock().unwrap().as_ref().unwrap().is_cancelled());
}

#[tokio::test]
async fn rewrite_cancel_or_session_replacement_cannot_create_fallback() {
    for replace in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
        let (began_tx,began_rx)=oneshot::channel();let (release_tx,release_rx)=oneshot::channel();let worker=h.clone();
        let job=tokio::spawn(async move {worker.finish_rewrite_with_policy("s".into(),7,move|_,_,_,_,_execution|async move {
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
async fn rewrite_invalid_source_pending_save_and_history_attempts_do_not_fallback() {
    for case in ["incomplete","pending","history","revoked","invalid_source","cancelled"] {
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
