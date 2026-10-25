// Real coordinator/SQLite failure paths with injected generation. No cloud,
// microphone, native input, profile DB, or clipboard access.
#[tokio::test]
async fn statistics_event_failure_preserves_write_error_and_permanently_latches_this_process() {
    let h=hub();assert!(!h.personalization_statistics_incomplete.load(Ordering::SeqCst));
    assert_eq!(command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap()["normal"]["total"],0);
    fault(&h,"CREATE TRIGGER reject_statistic_insert BEFORE INSERT ON personalization_events BEGIN SELECT RAISE(ABORT,'private-statistic-failure-sentinel'); END;");
    let event=personalization::task_event("failed-statistic-write",PersonalizationEventKind::Normal,None,PersonalizationEventStatus::Success,1);
    let original=h.record_personalization_event(event.clone()).await.unwrap_err();assert_eq!(original.code,"storageFailed");
    assert!(h.personalization_statistics_incomplete.load(Ordering::SeqCst));
    fault(&h,"DROP TRIGGER reject_statistic_insert;");
    h.record_personalization_event(event).await.unwrap();
    for window in ["all","week"] {
        let error=command(&h,"personalization.stats",json!({"window":window})).await.unwrap_err();
        assert_eq!(error.code,"personalization_statistics_incomplete");assert_eq!(error.stage,"personalization");
        assert!(error.message.contains("统计未能保存"));assert!(error.message.contains("不会删除识别稿或成稿"));
        assert!(!serde_json::to_string(&error).unwrap().contains("private-statistic-failure-sentinel"));
    }
    // A later failed read cannot mask the already-known incomplete denominator.
    h.db.shutdown().await.unwrap();
    assert_eq!(command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap_err().code,"personalization_statistics_incomplete");
}

#[tokio::test]
async fn statistics_initial_and_terminal_write_failures_do_not_discard_ordinary_source_or_draft() {
    for terminal in [false,true] {
        let h=hub();let saved=save(&h,config());let id=if terminal{"statistic-terminal-failure"}else{"statistic-initial-failure"};
        install(&h,id,SessionPhase::FinalizingAsr,true);
        {let mut inner=h.inner.lock().unwrap();inner.active.as_mut().unwrap().mode=h.db.mode_snapshot("technical",None).unwrap();}
        checkpoint(&h,id);let before=h.db.get_result(id).unwrap().source;
        fault(&h,if terminal{
            "CREATE TRIGGER reject_task_statistic BEFORE UPDATE ON personalization_events BEGIN SELECT RAISE(ABORT,'synthetic terminal statistic failure'); END;"
        }else{
            "CREATE TRIGGER reject_task_statistic BEFORE INSERT ON personalization_events BEGIN SELECT RAISE(ABORT,'synthetic initial statistic failure'); END;"
        });
        let delivered=Arc::new(AtomicBool::new(false));let delivery=delivered.clone();
        h.finish_rewrite_with_context_policy(id.into(),7,|_,_,input,_,_,context|async move {
            assert!(context.unwrap().personalization.is_some());Ok(result(&input))
        },None::<RewriteWaitPolicy>,move|h,_,_|{
            delivery.store(true,Ordering::SeqCst);h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;
        }).await;
        assert!(delivered.load(Ordering::SeqCst),"optional statistics must not block ordinary completion");
        let after=h.db.get_result(id).unwrap();assert_eq!(after.source,before);
        assert_eq!(after.output.as_ref().unwrap().text(),before.text());assert!(after.output.as_ref().unwrap().rewrite_fallback.is_none());
        assert_eq!(after.output.as_ref().unwrap().writing_receipt.as_ref().unwrap().personalization.as_ref().unwrap().version,saved.version);
        assert!(h.personalization_statistics_incomplete.load(Ordering::SeqCst));
        let DbValue::PersonalizationEvents(recorded)=h.db.run(DbCommand::GetPersonalizationEvents{since:None}).unwrap() else{panic!("events required");};
        if terminal {assert_eq!(recorded.len(),1);assert_eq!(recorded[0].status,PersonalizationEventStatus::Started);}
        else {assert!(recorded.is_empty(),"a missing task must not yield a seemingly complete zero denominator");}
        assert_eq!(command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap_err().code,"personalization_statistics_incomplete");
        fault(&h,"DROP TRIGGER reject_task_statistic;");
        let recovered=personalization::task_event("later-successful-statistic",PersonalizationEventKind::Preview,None,PersonalizationEventStatus::Success,1);
        h.record_personalization_event(recovered).await.unwrap();
        assert_eq!(command(&h,"personalization.stats",json!({"window":"all"})).await.unwrap_err().code,"personalization_statistics_incomplete");
        assert_eq!(h.db.get_result(id).unwrap(),after,"later statistics writes must not mutate source or output");
        h.db.shutdown().await.unwrap();
    }
}
