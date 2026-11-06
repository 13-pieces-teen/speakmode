// Local SQLite/serialization evidence only. These tests do not establish that
// a provider received a request or followed a preference.
#[test]
fn personalization_dispatch_wire_contract_preserves_legacy_unknown_and_global_preferences() {
    let mut event=personalization_event("dispatch-wire");
    event.reference_dispatch=None;
    let legacy=serde_json::to_value(&event).unwrap();
    assert!(legacy.get("referenceDispatch").is_none());
    assert_eq!(serde_json::from_value::<PersonalizationEvent>(legacy.clone()).unwrap(),event);
    // Global/scene preferences can have no selected named identity.
    event.profile_id=None;event.reference_dispatch=Some(ReferenceDispatch::Started);event.validate().unwrap();
    assert_eq!(serde_json::to_value(&event).unwrap()["referenceDispatch"],"started");
    event.reference_dispatch=Some(ReferenceDispatch::Unknown);event.validate().unwrap();
    assert_eq!(serde_json::to_value(&event).unwrap()["referenceDispatch"],"unknown");
    for marker in [ReferenceDispatch::Started,ReferenceDispatch::Unknown] {
        event.reference_dispatch=Some(marker);event.personalized=false;
        assert_eq!(event.validate(),Err("personalization_dispatch_requires_reference"));
    }
    event.reference_dispatch=Some(ReferenceDispatch::NotStarted);event.validate().unwrap();
    let mut invalid=legacy;invalid["referenceDispatch"]=serde_json::json!("providerReceived");
    assert!(serde_json::from_value::<PersonalizationEvent>(invalid).is_err());
}

#[test]
fn personalization_dispatch_upgrade_deduplicates_task_and_rejects_downgrades_or_identity_changes() {
    let mut store=Store::in_memory().unwrap();let mut event=personalization_event("dispatch-dedup");
    store.record_personalization_event(event.clone()).unwrap();
    // Generation/review/repair callbacks may all refer to this one task.
    event.reference_dispatch=Some(ReferenceDispatch::Started);
    for _ in 0..4 {store.record_personalization_event(event.clone()).unwrap();}
    event.status=PersonalizationEventStatus::Success;event.duration_ms=42;
    store.record_personalization_event(event.clone()).unwrap();
    store.set_personalization_feedback(&event.task_id,PersonalizationFeedback::Better).unwrap();
    store.record_personalization_event(event.clone()).unwrap();
    for marker in [None,Some(ReferenceDispatch::NotStarted),Some(ReferenceDispatch::Unknown)] {
        let mut replay=event.clone();replay.reference_dispatch=marker;
        assert!(matches!(store.record_personalization_event(replay),Err(StoreError::Conflict{..})));
    }
    let mut changed=event.clone();changed.personalized=false;changed.reference_dispatch=Some(ReferenceDispatch::NotStarted);
    assert!(store.record_personalization_event(changed).is_err());
    let mut changed=event.clone();changed.profile_version+=1;assert!(store.record_personalization_event(changed).is_err());
    let events=store.personalization_events(None).unwrap();assert_eq!(events.len(),1);
    assert_eq!(events[0].reference_dispatch,Some(ReferenceDispatch::Started));assert_eq!(events[0].feedback,Some(PersonalizationFeedback::Better));
    event.status=PersonalizationEventStatus::Cancelled;store.record_personalization_event(event).unwrap();
    let events=store.personalization_events(None).unwrap();assert_eq!(events[0].reference_dispatch,Some(ReferenceDispatch::Started));assert_eq!(events[0].feedback,None);
    let mut unselected=personalization_event("unselected-frozen");unselected.profile_id=None;unselected.profile_version=0;unselected.personalized=false;
    store.record_personalization_event(unselected.clone()).unwrap();unselected.personalized=true;
    assert!(matches!(store.record_personalization_event(unselected),Err(StoreError::Conflict{..})),"selection is frozen even before dispatch");
}

#[test]
fn personalization_dispatch_legacy_and_terminal_no_dispatch_cannot_be_promoted_by_late_replay() {
    let mut store=Store::in_memory().unwrap();
    for status in [PersonalizationEventStatus::Success,PersonalizationEventStatus::Failed,PersonalizationEventStatus::Fallback,PersonalizationEventStatus::Cancelled] {
        let mut event=personalization_event(&format!("terminal-{status:?}"));
        store.record_personalization_event(event.clone()).unwrap();
        event.status=status;store.record_personalization_event(event.clone()).unwrap();
        event.reference_dispatch=Some(ReferenceDispatch::Started);
        assert!(matches!(store.record_personalization_event(event),Err(StoreError::Conflict{..})));
    }
    let mut legacy=personalization_event("legacy-unknown");legacy.reference_dispatch=None;
    store.record_personalization_event(legacy.clone()).unwrap();
    for marker in [ReferenceDispatch::NotStarted,ReferenceDispatch::Started,ReferenceDispatch::Unknown] {
        let mut replay=legacy.clone();replay.reference_dispatch=Some(marker);
        assert!(matches!(store.record_personalization_event(replay),Err(StoreError::Conflict{..})));
    }
    legacy.status=PersonalizationEventStatus::Success;store.record_personalization_event(legacy).unwrap();
    store.set_personalization_feedback("legacy-unknown",PersonalizationFeedback::Same).unwrap();
    let recorded=store.personalization_events(None).unwrap();
    assert!(recorded.iter().all(|event|event.reference_dispatch!=Some(ReferenceDispatch::Started)));
    assert_eq!(recorded.iter().find(|event|event.task_id=="legacy-unknown").unwrap().reference_dispatch,None);
}

#[test]
fn personalization_v5_migration_keeps_v4_event_bytes_feedback_and_history_without_fabricating_dispatch() {
    let file=path();let config;let historical;let references;let legacy_data;
    {
        let mut store=Store::open(&file).unwrap();let saved=store.save_personalization(personal_config(),1).unwrap();
        config=saved.clone();
        store.checkpoint_source(seed("v4-dispatch-migration",StoragePolicy::History),source("v4-dispatch-migration")).unwrap();
        let mut next=output("v4-dispatch-migration");
        next.writing_receipt=Some(super::writing_context::WritingReceipt{scene:AppWritingScene::Technical,language:DraftLanguage::AsSpoken,context_used:false,contract_version:"personalization-v1".into(),personalization:saved.freeze(AppWritingScene::Technical)});
        historical=store.commit_ready(next).unwrap();
        references=store.test_connection().query_row("SELECT data FROM personalization_references",[],|row|row.get::<_,String>(0)).unwrap();
        let mut legacy=personalization_event("v4-comparison");legacy.status=PersonalizationEventStatus::Success;legacy.reference_dispatch=None;legacy.feedback=Some(PersonalizationFeedback::Better);
        // A v4 record has no marker. Pretty formatting detects any silent JSON rewrite.
        legacy_data=serde_json::to_string_pretty(&legacy).unwrap();
        store.test_connection().execute("INSERT INTO personalization_events(task_id,created_at,data) VALUES(?1,?2,?3)",rusqlite::params![legacy.task_id,legacy.created_at,legacy_data]).unwrap();
        store.test_connection().execute_batch("PRAGMA user_version=4;").unwrap();
    }
    {
        let mut store=Store::open(&file).unwrap();
        assert_eq!(store.test_connection().pragma_query_value(None,"user_version",|row|row.get::<_,u32>(0)).unwrap(),6);
        assert_eq!(store.test_connection().query_row("SELECT data FROM personalization_events",[],|row|row.get::<_,String>(0)).unwrap(),legacy_data);
        assert_eq!(store.test_connection().query_row("SELECT data FROM personalization_references",[],|row|row.get::<_,String>(0)).unwrap(),references);
        assert_eq!(store.personalization().unwrap(),config);assert_eq!(store.get_result("v4-dispatch-migration").unwrap(),historical);
        assert_eq!(store.usage_today("2026-10-04").unwrap().count,1);
        let events=store.personalization_events(None).unwrap();assert_eq!(events.len(),1);assert!(events[0].personalized);assert_eq!(events[0].reference_dispatch,None);assert_eq!(events[0].feedback,Some(PersonalizationFeedback::Better));
        store.set_personalization_feedback("v4-comparison",PersonalizationFeedback::Same).unwrap();
        let events=store.personalization_events(None).unwrap();assert_eq!(events[0].reference_dispatch,None);
        let data:String=store.test_connection().query_row("SELECT data FROM personalization_events",[],|row|row.get(0)).unwrap();assert!(!data.contains("referenceDispatch"));
        assert!(store.test_connection().execute("UPDATE sources SET data='changed'",[]).is_err());
        assert!(store.test_connection().execute("UPDATE outputs SET data='changed'",[]).is_err());
    }
    let store=Store::open(&file).unwrap();assert_eq!(store.personalization_events(None).unwrap()[0].reference_dispatch,None);drop(store);fs::remove_file(file).unwrap();
}

#[test]
fn personalization_v5_rejects_future_schema_without_rewriting_it() {
    let file=path();
    {let mut store=Store::open(&file).unwrap();store.record_personalization_event(personalization_event("future-schema")).unwrap();store.test_connection().execute_batch("PRAGMA user_version=7;").unwrap();}
    assert!(matches!(Store::open(&file),Err(StoreError::Invalid(message)) if message=="database schema is newer than this application"));
    let connection=rusqlite::Connection::open(&file).unwrap();
    assert_eq!(connection.pragma_query_value(None,"user_version",|row|row.get::<_,u32>(0)).unwrap(),7);
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM personalization_events",[],|row|row.get::<_,u64>(0)).unwrap(),1);
    drop(connection);fs::remove_file(file).unwrap();
}

#[test]
fn personalization_restart_distinguishes_pending_unknown_from_known_dispatch_and_no_reference() {
    let mut store=Store::in_memory().unwrap();
    for (id,selected,marker,status) in [
        ("pending-selected",true,Some(ReferenceDispatch::NotStarted),PersonalizationEventStatus::Started),
        ("pending-dispatched",true,Some(ReferenceDispatch::Started),PersonalizationEventStatus::Started),
        ("pending-legacy",true,None,PersonalizationEventStatus::Started),
        ("pending-no-reference",false,Some(ReferenceDispatch::NotStarted),PersonalizationEventStatus::Started),
        ("terminal-no-dispatch",true,Some(ReferenceDispatch::NotStarted),PersonalizationEventStatus::Failed),
    ] {
        let mut event=personalization_event(id);event.personalized=selected;event.reference_dispatch=marker;event.status=status;
        if !selected {event.profile_id=None;event.profile_version=0;}
        store.record_personalization_event(event).unwrap();
    }
    store.recover_after_restart().unwrap();let once=store.personalization_events(None).unwrap();
    let marker=|id:&str|once.iter().find(|event|event.task_id==id).unwrap().reference_dispatch;
    assert_eq!(marker("pending-selected"),Some(ReferenceDispatch::Unknown));
    assert_eq!(marker("pending-dispatched"),Some(ReferenceDispatch::Started));
    assert_eq!(marker("pending-legacy"),None);
    assert_eq!(marker("pending-no-reference"),Some(ReferenceDispatch::NotStarted));
    assert_eq!(marker("terminal-no-dispatch"),Some(ReferenceDispatch::NotStarted));
    assert!(once.iter().all(|event|event.status==PersonalizationEventStatus::Failed));
    store.recover_after_restart().unwrap();assert_eq!(store.personalization_events(None).unwrap(),once);
    let unknown=once.iter().find(|event|event.task_id=="pending-selected").unwrap();
    for replacement in [None,Some(ReferenceDispatch::NotStarted),Some(ReferenceDispatch::Started)] {
        let mut replay=unknown.clone();replay.reference_dispatch=replacement;
        assert!(matches!(store.record_personalization_event(replay),Err(StoreError::Conflict{..})));
    }
    let mut cancelled=unknown.clone();cancelled.status=PersonalizationEventStatus::Cancelled;store.record_personalization_event(cancelled).unwrap();
    assert_eq!(store.personalization_events(None).unwrap().iter().find(|event|event.task_id=="pending-selected").unwrap().reference_dispatch,Some(ReferenceDispatch::Unknown));
}
