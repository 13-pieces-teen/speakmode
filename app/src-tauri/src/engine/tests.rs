//! Behavioral integration checks against the actual AppHub and SQLite Store.
//! Fixtures install already-finished capture states; no Key lookup, cloud call,
//! microphone capture, native clipboard mutation, or native input occurs here.
use super::*;
#[path="candidate_review_tests.rs"]
mod candidate_review;
use std::sync::Barrier;

#[path="onboarding_tests.rs"]
mod onboarding_tests;

#[path="preview_tests.rs"]
mod preview_tests;
#[path="rewrite_fallback_tests.rs"]
mod rewrite_fallback_tests;

#[path="app_writing_tests.rs"]
mod app_writing_tests;
#[path="rewrite_deadline_tests.rs"]
mod rewrite_deadline_tests;
#[path="recorder_copy_tests.rs"]
mod recorder_copy_tests;
#[path="personalization_tests.rs"]
mod personalization_tests;
#[path="typeless_import_tests.rs"]
mod typeless_import_tests;


trait TestDb {
 fn run(&self,c:DbCommand)->StoreResult<DbValue>;
 fn preferences(&self)->StoreResult<Preferences>;fn provider_config(&self)->StoreResult<ProviderConfig>;
 fn mode_snapshot(&self,id:&str,revision:Option<u64>)->StoreResult<ModeSnapshot>;
 fn get_result(&self,id:&str)->StoreResult<HistoryDetail>;fn get_source(&self,id:&str)->StoreResult<SourceRevision>;
 fn usage_today(&self,date:&str)->StoreResult<DailyUsage>;fn save_mode(&self,draft:ModeDraft)->StoreResult<ModeRecord>;
 fn delete_mode(&self,id:&str,expected:u64)->StoreResult<()>;fn save_preferences(&self,next:Preferences,expected:u64)->StoreResult<Preferences>;
 fn latest_temporary_result(&self)->StoreResult<Option<HistoryDetail>>;fn prepare_delivery(&self,attempt:DeliveryAttempt)->StoreResult<DeliveryAttempt>;
 fn delivery_attempt(&self,id:&str)->StoreResult<DeliveryAttempt>;fn test_sql(&self,sql:&str)->StoreResult<()>;fn query_count(&self,sql:&str)->StoreResult<u64>;
}
impl TestDb for DbClient {
 fn run(&self,c:DbCommand)->StoreResult<DbValue>{let guard=if let DbCommand::PrepareDelivery{attempt}=&c{Some(Arc::new(CommitGuard::new(attempt.session_id.clone(),attempt.generation)))}else{None};self.submit(c,guard,DbPriority::Normal)?.wait_blocking()}
 fn preferences(&self)->StoreResult<Preferences>{Ok(self.snapshot().preferences)}
 fn provider_config(&self)->StoreResult<ProviderConfig>{Ok(self.snapshot().provider_config)}
 fn mode_snapshot(&self,id:&str,revision:Option<u64>)->StoreResult<ModeSnapshot>{match self.run(DbCommand::ModeSnapshot{id:id.into(),revision})?{DbValue::ModeSnapshot(v)=>Ok(v),_=>panic!("wrong type")}}
 fn get_result(&self,id:&str)->StoreResult<HistoryDetail>{match self.run(DbCommand::GetResult{session_id:id.into()})?{DbValue::Detail(v)=>Ok(v),_=>panic!("wrong type")}}
 fn get_source(&self,id:&str)->StoreResult<SourceRevision>{match self.run(DbCommand::GetSource{source_id:id.into()})?{DbValue::Source(v)=>Ok(v),_=>panic!("wrong type")}}
 fn usage_today(&self,date:&str)->StoreResult<DailyUsage>{match self.run(DbCommand::UsageToday{date:date.into()})?{DbValue::Usage(v)=>Ok(v),_=>panic!("wrong type")}}
 fn save_mode(&self,draft:ModeDraft)->StoreResult<ModeRecord>{match self.run(DbCommand::SaveMode{draft})?{DbValue::Mode(v)=>Ok(v),_=>panic!("wrong type")}}
 fn delete_mode(&self,id:&str,expected:u64)->StoreResult<()>{self.run(DbCommand::DeleteMode{id:id.into(),expected_revision:expected})?;Ok(())}
 fn save_preferences(&self,next:Preferences,expected:u64)->StoreResult<Preferences>{match self.run(DbCommand::SavePreferences{next,expected_version:expected})?{DbValue::Preferences(v)=>Ok(v),_=>panic!("wrong type")}}
 fn latest_temporary_result(&self)->StoreResult<Option<HistoryDetail>>{Ok(self.snapshot().temporary_result)}
 fn prepare_delivery(&self,attempt:DeliveryAttempt)->StoreResult<DeliveryAttempt>{match self.run(DbCommand::PrepareDelivery{attempt})?{DbValue::Delivery(v)=>Ok(v),_=>panic!("wrong type")}}
 fn delivery_attempt(&self,id:&str)->StoreResult<DeliveryAttempt>{match self.run(DbCommand::DeliveryAttempt{attempt_id:id.into()})?{DbValue::Delivery(v)=>Ok(v),_=>panic!("wrong type")}}
 fn test_sql(&self,sql:&str)->StoreResult<()>{self.run(DbCommand::TestExecuteBatch{sql:sql.into()})?;Ok(())}
 fn query_count(&self,sql:&str)->StoreResult<u64>{match self.run(DbCommand::TestQueryCount{sql:sql.into()})?{DbValue::Count(v)=>Ok(v),_=>panic!("wrong type")}}
}

fn test_data_directory() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).canonicalize().unwrap();
    let scope = manifest.join(".test-data/engine");
    std::fs::create_dir_all(&scope).unwrap();
    assert_eq!(scope.canonicalize().unwrap(), scope, "fixture directory must stay in the source tree");
    let path = scope.join(Uuid::new_v4().to_string());
    std::fs::create_dir(&path).unwrap();
    assert_eq!(path.canonicalize().unwrap(), path, "fixture directory must not be redirected");
    path
}
pub(super) fn hub() -> Arc<AppHub> {
    Arc::new(AppHub {
        diagnostics:diagnostics::DiagnosticsRecorder::default(),
        inner: Mutex::new(Inner {
            onboarding:OnboardingRuntime::default(),
            guards: Default::default(), sequence: 0, active: None, recovery: None,
            candidate: None, preview: None, preview_sources:Default::default(),personalization_workers:Default::default(),personalization_revocations:Default::default(), shortcut_capture:None,capture_commit_guard:None, used_operation_ids:Default::default(),
            cancelled_operation_ids:Default::default(),auxiliary_workers:Default::default(),key_configured: false, asr_key_configured:false, rewrite_key_configured:false, requests: Default::default(),
            request_order: Default::default(),settings_pending:None,
        }),
        db:DbClient::in_memory().unwrap(),db_command_workers:std::sync::atomic::AtomicUsize::new(0),personalization_statistics_incomplete:AtomicBool::new(false),settings_io:Mutex::new(()),shortcut_io:Mutex::new(()),
        subscribers: Mutex::new(HashMap::new()), publication: Mutex::new(()),
        data_dir: test_data_directory(),
        hooks: Mutex::new(None), native_actions: Mutex::new(None), recorder_control: Mutex::new(None), stop_target_control: Mutex::new(None), native_io: Mutex::new(()),
        lifecycle_epoch: std::sync::atomic::AtomicU64::new(0), capture_available: AtomicBool::new(true),quitting:AtomicBool::new(false),shortcut_capture_available:AtomicBool::new(true),shortcut_focus_epoch:std::sync::atomic::AtomicU64::new(0),
    })
}

pub(super) fn install(hub: &AppHub, id: &str, phase: SessionPhase, worker_released: bool) {
    let prefs = hub.db.preferences().unwrap();
    let config = hub.db.provider_config().unwrap();
    assert!(config.region.is_none(), "configuration must fail before any credential lookup");
    let mode = hub.db.mode_snapshot("auto", None).unwrap();
    let mut i=hub.inner.lock().unwrap();i.guards.insert(id.into(),Arc::new(CommitGuard::new(id.into(),7)));
    i.active = Some(ActiveSession {
        personalization:hub.db.snapshot().personalization,personalization_commit:None,
        onboarding_lesson:None,
        diagnostic_trace:None,caption_render_token:None,
        id: id.into(), generation: 7, phase, prefs, config, mode, dictionary:hub.db.snapshot().dictionary,
        started_at: "2026-10-04T10:00:00+08:00".into(),
        ended_at: Some("2026-10-04T10:00:12+08:00".into()), capture_duration_ms: 12_000, recording_limit_ms:RECORDING_LIMIT_MS, recording_limit_remaining_ms:None,
        first_samples: true, provider_ready: true, level: 0.0, captions: vec![], error: None,
        stop: Arc::new(AtomicBool::new(false)), cancel: CancellationToken::new(), worker_released,
        processing_worker_released: true,db_pending:0,cancel_pending:false,
        source: None, output: None, pending: None, cache: None, cache_complete: false,
        cache_created: None, target: None, target_application:None, writing_scene:None, input_context_frozen:false,input_context:None,input_context_status:InputContextStatus::Disabled, target_capture_pending:false, target_capture_failure_reason:None, delivery_failure_reason:None, delivery: None,
        delivery_permit: None, delivery_worker_released: true,
        original_delivery_revoked:false,revision_intent_sequence:0,revision_receipt_sequence:0,
        reason: CreationReason::NormalInitial, asr_attempt_id: Some(format!("attempt-{id}")),
    });
}

fn source(id: &str) -> SourceRevision {
    SourceRevision {
        id: format!("source-{id}"), session_id: id.into(), asr_attempt_id: format!("attempt-{id}"),
        generation: 7, provider: "aliyun".into(), model: "candidate-asr".into(), adapter_version: "v1".into(),
        complete: true, created_at: "2026-10-04T10:00:12+08:00".into(),
        items: vec![
            SourceItem { id: format!("item-{id}-0"), provider_item_id: "0".into(), capture_order: 0,
                text: "如果会议未确认，不要订机票。".into(), is_final: true, start_ms: Some(0), end_ms: Some(7000) },
            SourceItem { id: format!("item-{id}-1"), provider_item_id: "1".into(), capture_order: 1,
                text: "先保留行程。".into(), is_final: true, start_ms: Some(7000), end_ms: Some(12000) },
        ],
    }
}

fn output(hub: &AppHub, id: &str) -> OutputRevision {
    OutputRevision {
        writing_receipt:None, confirmed_context:None, id: format!("output-{id}"), session_id: id.into(), parent_revision_id: None, base_revision_id: None,
        source_revision_id: format!("source-{id}"),
        mode_snapshot: hub.inner.lock().unwrap().active.as_ref().unwrap().mode.clone(),
        resolved_mode: BaseMode::Structure, rewrite_fallback: None,
        blocks: vec![OutputBlock { id: "block-0".into(), text: "会议未确认前，不要订机票；先保留行程。".into(),
            source_refs: vec![format!("item-{id}-0"), format!("item-{id}-1")] }],
        kind: RevisionKind::Initial, creation_reason: CreationReason::NormalInitial,
        created_at: "2026-10-04T10:00:13+08:00".into(),
    }
}

fn checkpoint(hub: &AppHub, id: &str) { hub.checkpoint(id, 7, source(id)).unwrap(); }
fn ready(hub: &AppHub, id: &str) {
    checkpoint(hub, id); hub.commit_output(id, 7, output(hub, id)).unwrap();
    // This is a saved-output fixture, not a native delivery. A missing target
    // now correctly consumes a Skipped delivery ledger entry, so exercising
    // that path here would pre-consume the tests' later explicit input attempt.
    hub.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;
}
fn identity(id: &str) -> Value { json!({"sessionId":id,"expectedGeneration":7,"requestId":Uuid::new_v4().to_string()}) }
fn view(hub: &AppHub) -> LiveSession { hub.inner.lock().unwrap().active.as_ref().unwrap().view() }
fn usage_count(hub: &AppHub) -> u64 { hub.db.usage_today("2026-10-04").unwrap().count }
fn fault(hub: &AppHub, sql: &str) { hub.db.test_sql(sql).unwrap(); }
async fn command(hub: &Arc<AppHub>, name: &str, payload: Value) -> Result<Value> {
    hub.command("main", CommandRequest { name: name.into(), payload }).await
}

#[tokio::test]
async fn usage_total_ipc_is_a_main_window_read_with_camel_case_counters() {
    let h = hub();
    assert_eq!(command(&h, "usage.total", json!({})).await.unwrap(), json!({"count":0,"sourceCharCount":0,"captureDurationMs":0}));
    install(&h, "usage-total", SessionPhase::FinalizingAsr, true);
    ready(&h, "usage-total");
    let sequence = h.inner.lock().unwrap().sequence;
    let result = command(&h, "usage.total", json!({})).await.unwrap();
    assert_eq!(result, json!({"count":1,"sourceCharCount":crate::core::store::source_character_count(&source("usage-total").text()),"captureDurationMs":12_000}));
    assert_eq!(h.inner.lock().unwrap().sequence, sequence, "statistics reads must not publish changed events");
    assert_eq!(h.command("recording", CommandRequest { name:"usage.total".into(), payload:json!({}) }).await.unwrap_err().code, "forbidden");
    h.db.shutdown().await.unwrap();
}

#[tokio::test]
async fn correction_remembers_only_the_selected_term_and_preserves_source_and_undo() {
    let h=hub();install(&h,"correct-term",SessionPhase::FinalizingAsr,true);ready(&h,"correct-term");
    let before=h.db.get_result("correct-term").unwrap();
    let result=command(&h,"revision.correctTerm",json!({"sessionId":"correct-term","expectedCurrentRevisionId":"output-correct-term","requestId":"correct-first","patch":{"blockId":"block-0","start":0,"end":2,"expectedText":"会议","replacement":"PolyU"},"remember":true,"expectedDictionaryVersion":1})).await.unwrap();
    let detail:HistoryDetail=serde_json::from_value(result["detail"].clone()).unwrap();
    assert_eq!(detail.output.as_ref().unwrap().blocks[0].text,"PolyU未确认前，不要订机票；先保留行程。");
    assert_eq!(detail.source,before.source);assert!(detail.can_undo);
    assert_eq!(result["dictionaryStatus"],"added");assert_eq!(result["dictionary"]["entries"][0]["term"],"PolyU");
    assert_eq!(h.db.snapshot().dictionary.entries[0].term,"PolyU");
    let undone:HistoryDetail=serde_json::from_value(command(&h,"revision.undo",json!({"sessionId":"correct-term","expectedCurrentRevisionId":detail.output.unwrap().id,"requestId":"correct-undo"})).await.unwrap()).unwrap();
    assert_eq!(undone.output.unwrap().blocks,before.output.unwrap().blocks);
    assert_eq!(h.db.snapshot().dictionary.entries[0].term,"PolyU","undo text keeps the explicitly remembered dictionary word");
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}

fn correction_payload(id:&str,revision:&str,replacement:&str,remember:bool)->Value {
    json!({"sessionId":id,"expectedCurrentRevisionId":revision,"requestId":Uuid::new_v4().to_string(),"patch":{"blockId":"block-0","start":0,"end":2,"expectedText":"会议","replacement":replacement},"remember":remember,"expectedDictionaryVersion":1})
}

#[tokio::test]
async fn correction_dictionary_failure_and_stale_versions_commit_neither_side() {
    let h=hub();install(&h,"correction-atomic",SessionPhase::FinalizingAsr,true);ready(&h,"correction-atomic");
    let original=h.db.get_result("correction-atomic").unwrap();
    fault(&h,"CREATE TRIGGER reject_correction_word BEFORE INSERT ON dictionary_entries BEGIN SELECT RAISE(ABORT,'synthetic dictionary failure'); END;");
    let payload=correction_payload("correction-atomic","output-correction-atomic","SpeakMode",true);
    assert_eq!(command(&h,"revision.correctTerm",payload).await.unwrap_err().code,"storageFailed");
    assert_eq!(h.db.get_result("correction-atomic").unwrap(),original);assert_eq!(h.db.snapshot().dictionary,DictionarySnapshot::default());
    fault(&h,"DROP TRIGGER reject_correction_word;");
    command(&h,"dictionary.import",json!({"terms":["Figma"],"expectedVersion":1,"requestId":"correction-dictionary-change"})).await.unwrap();
    let payload=correction_payload("correction-atomic","output-correction-atomic","SpeakMode",true);
    assert_eq!(command(&h,"revision.correctTerm",payload).await.unwrap_err().code,"revisionConflict");
    assert_eq!(h.db.get_result("correction-atomic").unwrap(),original);assert_eq!(h.db.snapshot().dictionary.entries[0].term,"Figma");
    let edited:HistoryDetail=serde_json::from_value(command(&h,"revision.edit",edit_payload("correction-atomic","output-correction-atomic","已有新版本")).await.unwrap()).unwrap();
    let mut payload=correction_payload("correction-atomic","output-correction-atomic","SpeakMode",true);payload["expectedDictionaryVersion"]=json!(2);
    assert_eq!(command(&h,"revision.correctTerm",payload).await.unwrap_err().code,"revisionConflict");
    assert_eq!(h.db.get_result("correction-atomic").unwrap(),edited);assert_eq!(h.db.snapshot().dictionary.entries.len(),1);
}

#[tokio::test]
async fn correction_duplicate_word_is_idempotent_and_replay_cannot_resurrect_a_deleted_word() {
    let h=hub();install(&h,"correction-replay",SessionPhase::FinalizingAsr,true);ready(&h,"correction-replay");
    let dictionary=command(&h,"dictionary.import",json!({"terms":["SpeakMode"],"expectedVersion":1,"requestId":"correction-existing"})).await.unwrap();
    let mut payload=correction_payload("correction-replay","output-correction-replay","ＳＰＥＡＫＭＯＤＥ",true);payload["expectedDictionaryVersion"]=json!(2);
    let result=command(&h,"revision.correctTerm",payload.clone()).await.unwrap();
    assert_eq!(result["dictionaryStatus"],"existing");assert_eq!(result["dictionary"],dictionary);assert_eq!(h.db.snapshot().dictionary.version,2);
    assert_eq!(command(&h,"revision.correctTerm",payload.clone()).await.unwrap(),result);
    assert_eq!(h.db.get_result("correction-replay").unwrap().output.unwrap().blocks[0].text,"ＳＰＥＡＫＭＯＤＥ未确认前，不要订机票；先保留行程。");
    command(&h,"dictionary.delete",json!({"entryId":dictionary["entries"][0]["id"],"expectedVersion":2,"requestId":"correction-delete-existing"})).await.unwrap();
    assert_eq!(command(&h,"revision.correctTerm",payload).await.unwrap_err().code,"request_already_processed");
    assert!(h.db.snapshot().dictionary.entries.is_empty());
}

#[tokio::test]
async fn correction_capacity_is_atomic_and_user_may_correct_without_remembering() {
    let h=hub();install(&h,"correction-full",SessionPhase::FinalizingAsr,true);ready(&h,"correction-full");
    let terms:Vec<_>=(0..200).map(|index|format!("term{index}")).collect();
    command(&h,"dictionary.import",json!({"terms":terms,"expectedVersion":1,"requestId":"correction-fill"})).await.unwrap();
    let original=h.db.get_result("correction-full").unwrap();
    let mut payload=correction_payload("correction-full","output-correction-full","SpeakMode",true);payload["expectedDictionaryVersion"]=json!(2);
    assert_eq!(command(&h,"revision.correctTerm",payload).await.unwrap_err().code,"dictionary_entry_limit");
    assert_eq!(h.db.get_result("correction-full").unwrap(),original);assert_eq!(h.db.snapshot().dictionary.entries.len(),200);
    let result=command(&h,"revision.correctTerm",correction_payload("correction-full","output-correction-full","SpeakMode",false)).await.unwrap();
    assert_eq!(result["dictionaryStatus"],"notRequested");assert!(result["dictionary"].is_null());assert_eq!(h.db.snapshot().dictionary.entries.len(),200);
    assert_eq!(result["detail"]["output"]["blocks"][0]["text"],"SpeakMode未确认前，不要订机票；先保留行程。");
}

#[tokio::test]
async fn correction_rejects_wrong_occurrence_invalid_terms_and_recording_window_without_mutation() {
    let h=hub();install(&h,"correction-validate",SessionPhase::FinalizingAsr,true);ready(&h,"correction-validate");
    let original=h.db.get_result("correction-validate").unwrap();
    for change in [json!({"start":0,"end":0}),json!({"expectedText":"不是这里"}),json!({"blockId":"other"}),json!({"replacement":"x".repeat(81)}),json!({"replacement":"name\n"}),json!({"replacement":"name\u{200b}"})] {
        let mut payload=correction_payload("correction-validate","output-correction-validate","SpeakMode",true);
        for (key,value) in change.as_object().unwrap(){payload["patch"][key]=value.clone();}
        assert!(command(&h,"revision.correctTerm",payload).await.is_err());
        assert_eq!(h.db.get_result("correction-validate").unwrap(),original);assert_eq!(h.db.snapshot().dictionary.version,1);
    }
    let request=CommandRequest{name:"revision.correctTerm".into(),payload:correction_payload("correction-validate","output-correction-validate","SpeakMode",true)};
    assert_eq!(h.command("recording",request).await.unwrap_err().code,"forbidden");
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn correction_revokes_prepared_delivery_without_clipboard_or_input() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");enable_fixture_delivery(&h);
    let copies=Arc::new(std::sync::atomic::AtomicUsize::new(0));let chords=Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let native=h.native_io.lock().unwrap();let delivery=fake_delivery(h.clone(),copies.clone(),chords.clone());wait_prepared(&h).await;
    let result=command(&h,"revision.correctTerm",correction_payload("s","output-s","SpeakMode",true)).await.unwrap();
    assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().output.as_ref().unwrap().id,result["detail"]["output"]["id"].as_str().unwrap());
    drop(native);delivery.await.unwrap();assert_eq!(copies.load(Ordering::SeqCst),0);assert_eq!(chords.load(Ordering::SeqCst),0);assert_eq!(current_delivery(&h).state,DeliveryState::Skipped);
}

#[tokio::test]
async fn dictionary_ipc_lists_without_request_id_and_mutations_return_authoritative_snapshots() {
    let h=hub();let empty=command(&h,"dictionary.list",json!({})).await.unwrap();
    assert_eq!(empty["version"],1);assert_eq!(empty["entries"],json!([]));
    let saved=command(&h,"dictionary.save",json!({"draft":{"term":"SpeakMode"},"expectedVersion":1,"requestId":"dictionary-save"})).await.unwrap();
    assert_eq!(saved["version"],2);assert_eq!(saved["entries"][0]["term"],"SpeakMode");
    assert_eq!(h.snapshot("main").unwrap()["dictionary"],saved);
    assert!(h.snapshot("recording").unwrap().get("dictionary").is_none());
    let rejected=h.command("recording",CommandRequest{name:"dictionary.list".into(),payload:json!({})}).await.unwrap_err();assert_eq!(rejected.code,"forbidden");
    let duplicate=command(&h,"dictionary.import",json!({"terms":["ＳＰＥＡＫＭＯＤＥ"],"expectedVersion":2,"requestId":"dictionary-duplicate"})).await.unwrap();
    assert_eq!(duplicate,saved);
    let deleted=command(&h,"dictionary.delete",json!({"entryId":saved["entries"][0]["id"],"expectedVersion":2,"requestId":"dictionary-delete"})).await.unwrap();
    assert_eq!(deleted["version"],3);assert_eq!(deleted["entries"],json!([]));
}

#[tokio::test]
async fn dictionary_conflicts_and_capacity_errors_are_specific_and_do_not_apply_a_draft() {
    let h=hub();command(&h,"dictionary.import",json!({"terms":["PolyU"],"expectedVersion":1,"requestId":"dictionary-seed"})).await.unwrap();
    let stale=command(&h,"dictionary.save",json!({"draft":{"term":"StaleDraft"},"expectedVersion":1,"requestId":"dictionary-stale"})).await.unwrap_err();
    assert_eq!(stale.code,"revisionConflict");assert_eq!(stale.stage,"dictionary");
    let invalid=command(&h,"dictionary.save",json!({"draft":{"term":"x".repeat(81)},"expectedVersion":2,"requestId":"dictionary-long"})).await.unwrap_err();
    assert_eq!(invalid.code,"dictionary_term_limit");assert_eq!(invalid.stage,"dictionary");
    let duplicate=command(&h,"dictionary.save",json!({"draft":{"term":"ＰＯＬＹＵ"},"expectedVersion":2,"requestId":"dictionary-existing"})).await.unwrap_err();
    assert_eq!(duplicate.code,"dictionary_duplicate");assert_eq!(h.db.snapshot().dictionary.entries[0].term,"PolyU");assert_eq!(h.db.snapshot().dictionary.version,2);
}

#[tokio::test]
async fn dictionary_replay_never_resurrects_deleted_terms_or_repeats_an_import() {
    let h=hub();let request=json!({"terms":["PrivateTerm"],"expectedVersion":1,"requestId":"dictionary-replay"});
    let saved=command(&h,"dictionary.import",request.clone()).await.unwrap();
    assert_eq!(command(&h,"dictionary.import",request.clone()).await.unwrap(),saved);assert_eq!(h.db.snapshot().dictionary.version,2);
    assert!(matches!(h.inner.lock().unwrap().requests["main:dictionary-replay"].outcome.lock().unwrap().as_ref().unwrap(),Ok(ReplayValue::Dictionary(2))));
    command(&h,"dictionary.delete",json!({"entryId":saved["entries"][0]["id"],"expectedVersion":2,"requestId":"dictionary-delete-replay"})).await.unwrap();
    assert_eq!(command(&h,"dictionary.import",request).await.unwrap_err().code,"request_already_processed");
    assert!(h.db.snapshot().dictionary.entries.is_empty());
}

#[tokio::test]
async fn dictionary_changes_during_capture_only_apply_to_the_next_session() {
    let h=hub();let saved=command(&h,"dictionary.import",json!({"terms":["FrozenVocabulary"],"expectedVersion":1,"requestId":"dictionary-before-capture"})).await.unwrap();
    install(&h,"dictionary-frozen",SessionPhase::Recording,true);
    let (frozen,mode)={let inner=h.inner.lock().unwrap();let active=inner.active.as_ref().unwrap();(active.dictionary.clone(),active.mode.clone())};
    command(&h,"dictionary.delete",json!({"entryId":saved["entries"][0]["id"],"expectedVersion":2,"requestId":"dictionary-while-capture"})).await.unwrap();
    command(&h,"dictionary.import",json!({"terms":["NextVocabulary"],"expectedVersion":3,"requestId":"dictionary-next-capture"})).await.unwrap();
    {let inner=h.inner.lock().unwrap();let active=inner.active.as_ref().unwrap();assert_eq!(active.dictionary,frozen);assert_eq!(active.mode,mode);assert_eq!(active.phase,SessionPhase::Recording);}
    assert!(!h.db.snapshot().dictionary.asr_context().unwrap().contains("FrozenVocabulary"));
    install(&h,"dictionary-next",SessionPhase::Recording,true);
    let inner=h.inner.lock().unwrap();let active=inner.active.as_ref().unwrap();assert_eq!(active.dictionary.version,4);assert!(active.dictionary.asr_context().unwrap().contains("NextVocabulary"));
}
fn events(hub: &AppHub) -> Arc<Mutex<Vec<AppEvent>>> {
    let values = Arc::new(Mutex::new(vec![])); let target = values.clone();
    hub.subscribe("main", Arc::new(move |event| target.lock().unwrap().push(event))).unwrap(); values
}

fn install_trace(h:&AppHub)->TraceHandle {
    let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();
    let trace=h.diagnostics.begin(&s.id,s.generation,Origin::UiCommand,Instant::now(),&s.config,&s.mode).unwrap();
    h.diagnostics.attempt(Some(&trace),s.asr_attempt_id.as_deref().unwrap());s.diagnostic_trace=Some(trace.clone());trace
}
fn synthetic_caption()->CaptionItem{CaptionItem{id:"attempt-s:0".into(),text:"如果没确认，先不要发邀请。".into(),revision:2,is_final:true,capture_order:0}}

#[test]
fn microphone_level_survives_caption_and_session_snapshots_without_consuming_sequence(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);let seen=events(&h);
    let before=h.inner.lock().unwrap().sequence;
    assert!(h.accept_audio_level("s",7,0.01));
    assert_eq!(view(&h).level,0.01);assert_eq!(h.inner.lock().unwrap().sequence,before);
    {let e=seen.lock().unwrap();assert_eq!(e.len(),1);assert_eq!(e[0].kind,"level");assert_eq!(e[0].sequence,0);assert_eq!(e[0].payload["sessionId"],"s");assert_eq!(e[0].payload["generation"],7);}
    h.accept_asr_progress("s",7,"attempt-s",AsrProgress::Caption(vec![synthetic_caption()]));
    assert_eq!(view(&h).level,0.01);
    let e=seen.lock().unwrap();assert_eq!(e[1].kind,"session");assert_eq!(e[1].sequence,before+1);
    assert!((e[1].payload["level"].as_f64().unwrap()-0.01).abs()<1e-6);
    assert!((h.snapshot("recording").unwrap()["session"]["level"].as_f64().unwrap()-0.01).abs()<1e-6);
}

#[test]
fn stale_invalid_and_post_stop_levels_cannot_change_the_current_meter(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);let seen=events(&h);
    assert!(h.accept_audio_level("s",7,0.04));
    for (id,generation,level) in [("old",7,0.9),("s",6,0.9),("s",7,f32::NAN),("s",7,f32::INFINITY)]{assert!(!h.accept_audio_level(id,generation,level));}
    assert_eq!(view(&h).level,0.04);assert_eq!(seen.lock().unwrap().len(),1);
    h.stop_with_capture(&identity("s"),||None).unwrap();assert_eq!(view(&h).level,0.0);
    let count=seen.lock().unwrap().len();assert!(!h.accept_audio_level("s",7,0.9));assert_eq!(seen.lock().unwrap().len(),count);assert_eq!(view(&h).level,0.0);
    install(&h,"new",SessionPhase::Recording,true);assert!(!h.accept_audio_level("s",7,0.9));assert_eq!(view(&h).level,0.0);
    assert!(h.accept_audio_level("new",7,5.0));assert_eq!(view(&h).level,1.0);
    assert!(h.accept_audio_level("new",7,-1.0));assert_eq!(view(&h).level,0.0);
    h.inner.lock().unwrap().active.as_ref().unwrap().cancel.cancel();let count=seen.lock().unwrap().len();
    assert!(!h.accept_audio_level("new",7,0.9));assert_eq!(seen.lock().unwrap().len(),count);
}

#[tokio::test]
async fn caption_receipt_is_scoped_deduplicated_and_does_not_enter_storage_or_publish_state(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);install_trace(&h);
    h.accept_asr_progress("s",7,"attempt-s",AsrProgress::Caption(vec![synthetic_caption()]));
    let live=view(&h);assert!(live.capture_ready);assert!(live.provider_ready);assert_eq!(live.captions.len(),1);assert!(live.captions[0].is_final);
    let payload=json!({"sessionId":"s","expectedGeneration":7,"asrAttemptId":"attempt-s","token":live.caption_render_token.unwrap()});
    let sequence=h.inner.lock().unwrap().sequence;let revision=h.db.snapshot().revision;
    for _ in 0..2{let answer=h.command("recording",CommandRequest{name:"session.captionRendered".into(),payload:payload.clone()}).await.unwrap();assert_eq!(answer["accepted"],true);}
    let i=h.inner.lock().unwrap();assert_eq!(i.sequence,sequence);assert!(i.requests.is_empty());drop(i);
    assert_eq!(h.db.snapshot().revision,revision);assert_eq!(h.db_command_workers.load(Ordering::SeqCst),0);
    let snap=h.diagnostics.snapshot().unwrap();assert_eq!(snap.traces[0].marks.iter().filter(|m|m.stage==Stage::CaptionRenderReceiptProxy).count(),1);
    assert_eq!(command(&h,"session.captionRendered",payload.clone()).await.unwrap_err().code,"forbidden");
    for key in ["sessionId","expectedGeneration","asrAttemptId","token"]{
        let mut stale=payload.clone();stale[key]=if key=="expectedGeneration"{json!(6)}else{json!("old")};
        assert_eq!(h.command("recording",CommandRequest{name:"session.captionRendered".into(),payload:stale}).await.unwrap()["accepted"],false);
    }
    h.inner.lock().unwrap().active.as_ref().unwrap().cancel.cancel();
    assert_eq!(h.command("recording",CommandRequest{name:"session.captionRendered".into(),payload}).await.unwrap()["accepted"],false);
}

#[tokio::test]
async fn diagnostics_are_main_only_readonly_metadata_and_not_in_recording_snapshot(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);install_trace(&h);
    h.accept_asr_progress("s",7,"attempt-s",AsrProgress::Caption(vec![synthetic_caption()]));
    let sequence=h.inner.lock().unwrap().sequence;
    let diagnostic=command(&h,"diagnostics.get",Value::Null).await.unwrap();
    assert_eq!(diagnostic["inMemoryOnly"],true);assert!(!diagnostic.to_string().contains("发邀请"));
    assert_eq!(h.command("recording",CommandRequest{name:"diagnostics.get".into(),payload:Value::Null}).await.unwrap_err().code,"forbidden");
    assert!(h.snapshot("recording").unwrap().get("diagnostics").is_none());
    let i=h.inner.lock().unwrap();assert_eq!(i.sequence,sequence);assert!(i.requests.is_empty());
}

#[test]
fn stale_attempt_callback_cannot_mutate_caption_or_create_first_render_token(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);install_trace(&h);
    h.accept_asr_progress("s",7,"attempt-old",AsrProgress::Caption(vec![synthetic_caption()]));
    h.accept_asr_progress("s",6,"attempt-s",AsrProgress::Caption(vec![synthetic_caption()]));
    let live=view(&h);assert!(live.captions.is_empty());assert!(live.caption_render_token.is_none());
    assert_eq!(h.diagnostics.snapshot().unwrap().traces[0].marks.len(),1);
}

#[test]
fn coverage_failure_keeps_checkpoint_and_offers_recovery_without_ready_usage_or_delivery(){
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);checkpoint(&h,"s");
    h.fail("s",7,ApiError::new("incomplete_source_coverage","rewrite","整理未覆盖全部来源"),true);
    let live=view(&h);assert_eq!(live.state,SessionPhase::Failed);assert!(live.can_retry_rewrite&&live.can_use_source);assert!(!live.can_retry_asr);
    let saved=h.db.get_result("s").unwrap();assert_eq!(saved.source,source("s"));assert!(saved.output.is_none());assert_eq!(usage_count(&h),0);
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().delivery.is_none());
    assert_eq!(h.diagnostics.snapshot().unwrap().traces[0].error_code.as_deref(),Some("incomplete_source_coverage"));
}

fn range_rewrite_source()->SourceRevision {
    let mut value=source("s");
    value.items[0].text="今天用 SpeakMode 比较 Typeless。".into();
    value.items[1].text="音频采集用 WASAPI。".into();
    value
}
fn range_rewrite_content(retain_today:bool,include_tail:bool)->String {
    let mut blocks=vec![json!({"text":if retain_today{"今天用 SpeakMode 比较 Typeless。"}else{"用 SpeakMode 比较 Typeless。"},"sourceRange":[0,0]})];
    if include_tail{blocks.push(json!({"text":"音频采集用 WASAPI。","sourceRange":[1,1]}));}
    json!({"resolvedMode":"structure","blocks":blocks}).to_string()
}
fn range_rewrite_checkpoint(h:&AppHub) {
    install(h,"s",SessionPhase::FinalizingAsr,true);install_trace(h);
    h.checkpoint("s",7,range_rewrite_source()).unwrap();
}
async fn finish_parsed_fixture(h:&Arc<AppHub>,content:String) {
    h.finish_rewrite_with_policy("s".into(),7,move|_config,mode,source,_cancel,_execution|async move{
        rewrite::parse_model_output(&content,&mode,&source)
    },RewriteWaitPolicy::default(),|hub,_,_|{hub.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;}).await;
}
fn assert_range_rewrite_rejected(h:&AppHub,expected_code:&str) {
    let live=view(h);assert_eq!(live.state,SessionPhase::Failed);
    assert_eq!(live.error_code.as_deref(),Some(expected_code));
    assert!(live.can_retry_rewrite&&live.can_use_source&&live.can_copy_recovery);
    assert!(!live.can_retry_asr);assert!(live.worker_released);
    let saved=h.db.get_result("s").unwrap();assert_eq!(saved.source,range_rewrite_source());
    assert_eq!(saved.session.status,StoredSessionStatus::Checkpointed);assert!(saved.output.is_none());
    assert_eq!(usage_count(h),0);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    let inner=h.inner.lock().unwrap();let active=inner.active.as_ref().unwrap();
    assert!(active.output.is_none()&&active.delivery.is_none()&&active.delivery_permit.is_none());
}

#[tokio::test]
async fn parsed_complete_ranges_commit_stable_source_refs_without_changing_source() {
    let h=hub();range_rewrite_checkpoint(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().prefs.delivery_mode=DeliveryMode::Preview;
    finish_parsed_fixture(&h,range_rewrite_content(true,true)).await;
    let live=view(&h);assert_eq!(live.state,SessionPhase::Ready);assert!(live.error.is_none());
    let stored=h.db.get_result("s").unwrap();assert_eq!(stored.source,range_rewrite_source());
    let output=stored.output.unwrap();assert_eq!(output.source_revision_id,"source-s");
    assert_eq!(output.blocks[0].source_refs,["item-s-0"]);assert_eq!(output.blocks[1].source_refs,["item-s-1"]);
    assert_eq!(output.resolved_mode,BaseMode::Structure);assert_eq!(output.creation_reason,CreationReason::NormalInitial);
    assert_eq!(usage_count(&h),1);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}

#[tokio::test]
async fn parsed_missing_time_qualifier_delivers_complete_source_instead_of_rejected_output() {
    let h=hub();range_rewrite_checkpoint(&h);let seen=events(&h);
    {let mut inner=h.inner.lock().unwrap();let active=inner.active.as_mut().unwrap();active.prefs.delivery_mode=DeliveryMode::Direct;active.target=Some(delivery_target_fixture());}
    finish_parsed_fixture(&h,range_rewrite_content(false,true)).await;
    assert_eq!(view(&h).state,SessionPhase::Ready);
    assert_eq!(view(&h).rewrite_fallback,Some(RewriteFallbackReason::MeaningRisk));
    assert!(view(&h).error.is_none());
    let output=h.db.get_result("s").unwrap().output.unwrap();assert_eq!(output.text(),range_rewrite_source().text());
    assert_eq!(output.blocks[0].source_refs,["item-s-0","item-s-1"]);
    assert_eq!(usage_count(&h),1);
    let trace=h.diagnostics.snapshot().unwrap();assert!(trace.traces[0].marks.iter().any(|m|m.stage==Stage::RewriteFallbackSelected));
    assert!(!trace.traces[0].marks.iter().any(|m|m.stage==Stage::RewriteValidated));
    assert!(!seen.lock().unwrap().iter().any(|event|event.kind=="session"&&event.payload["state"]=="failed"));
}

#[tokio::test]
async fn parsed_range_gap_uses_complete_source_and_counts_one_completed_dictation() {
    let h=hub();range_rewrite_checkpoint(&h);
    finish_parsed_fixture(&h,range_rewrite_content(true,false)).await;
    assert_eq!(view(&h).state,SessionPhase::Ready);assert_eq!(view(&h).rewrite_fallback,Some(RewriteFallbackReason::InvalidOutput));
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),range_rewrite_source().text());assert_eq!(usage_count(&h),1);
}

#[tokio::test]
async fn cancellation_rejects_late_successfully_parsed_range_output_before_commit() {
    let h=hub();range_rewrite_checkpoint(&h);
    let (parsed_tx,parsed_rx)=tokio::sync::oneshot::channel();let(release_tx,release_rx)=tokio::sync::oneshot::channel();
    let worker=h.clone();let job=tokio::spawn(async move{
        worker.finish_rewrite_with("s".into(),7,move|_config,mode,source,_cancel,_execution|async move{
            let parsed=rewrite::parse_model_output(&range_rewrite_content(true,true),&mode,&source);
            assert!(parsed.is_ok());parsed_tx.send(()).unwrap();
            // Deliberately ignore cancellation in the fixture transport: the
            // AppHub receipt gate must still reject this already-parsed result.
            release_rx.await.unwrap();parsed
        }).await;
    });
    parsed_rx.await.unwrap();h.cancel_session_async(&identity("s")).await.unwrap();
    let _=release_tx.send(());job.await.unwrap();
    let live=view(&h);assert_eq!(live.state,SessionPhase::Cancelled);assert!(!live.can_copy_recovery);
    assert!(h.db.get_result("s").is_err());assert_eq!(usage_count(&h),0);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    let trace=h.diagnostics.snapshot().unwrap();assert!(!trace.traces[0].marks.iter().any(|mark|mark.stage==Stage::RewriteValidated||mark.stage==Stage::ReadyCommitted));
}

#[tokio::test]
async fn rejected_retry_output_never_falls_back_or_reinputs_after_explicit_source_adoption() {
    let h=hub();range_rewrite_checkpoint(&h);
    {let mut inner=h.inner.lock().unwrap();let active=inner.active.as_mut().unwrap();active.reason=CreationReason::Retry;active.prefs.delivery_mode=DeliveryMode::Direct;active.target=Some(delivery_target_fixture());}
    finish_parsed_fixture(&h,range_rewrite_content(false,true)).await;
    assert_range_rewrite_rejected(&h,"rewrite_meaning_risk");
    h.initial_delivery_with("s",7,|_|panic!("failure cannot enter input"),|_,_,_,_|panic!("failure cannot fall back to source input"));
    let adopted=h.use_source_async(&identity("s")).await.unwrap();
    assert_eq!(adopted["source"],serde_json::to_value(range_rewrite_source()).unwrap());
    let saved=h.db.get_result("s").unwrap();let output=saved.output.unwrap();
    let original=range_rewrite_source();
    assert_eq!(output.kind,RevisionKind::UseSource);assert_eq!(output.creation_reason,CreationReason::UseSource);
    assert_eq!(output.blocks.iter().map(|block|block.text.as_str()).collect::<Vec<_>>(),original.items.iter().map(|item|item.text.as_str()).collect::<Vec<_>>());
    assert_eq!(view(&h).state,SessionPhase::Ready);assert_eq!(usage_count(&h),1);
    h.initial_delivery_with("s",7,|_|panic!("explicit source adoption cannot reclaim the original target"),|_,_,_,_|panic!("explicit source adoption cannot automatically input"));
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}

#[test]
fn same_generation_trace_handles_keep_worker_exit_on_its_own_retry_run(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);let old=install_trace(&h);
    let lease=h.processing_lease("s",7);let new=install_trace(&h);assert_ne!(old.run_id,new.run_id);drop(lease);
    let snapshot=h.diagnostics.snapshot().unwrap();assert!(snapshot.traces[0].marks.iter().any(|m|m.stage==Stage::ProcessingWorkerReleased));assert!(!snapshot.traces[1].marks.iter().any(|m|m.stage==Stage::ProcessingWorkerReleased));
}

#[tokio::test]
async fn ready_save_retries_admit_one_run_and_do_not_accept_prior_caption_ack(){
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);
    h.accept_asr_progress("s",7,"attempt-s",AsrProgress::Caption(vec![synthetic_caption()]));
    let old_ack=json!({"sessionId":"s","expectedGeneration":7,"asrAttemptId":"attempt-s","token":view(&h).caption_render_token.unwrap()});
    checkpoint(&h,"s");fault(&h,"CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'fault'); END;");
    let error=h.commit_output("s",7,output(&h,"s")).unwrap_err();h.fail("s",7,error,true);
    let mut p=identity("s");p["pendingCommitId"]=json!(view(&h).pending_commit_id.unwrap());fault(&h,"DROP TRIGGER fail_usage;");
    let(a,b)=tokio::join!(h.retry_save(&p),h.retry_save(&p));assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
    assert!(view(&h).worker_released);assert_eq!(usage_count(&h),1);assert_eq!(h.caption_rendered(&old_ack).unwrap()["accepted"],false);
    let s=h.diagnostics.snapshot().unwrap();assert_eq!(s.traces.len(),2);assert!(s.traces[1].marks.iter().any(|m|m.stage==Stage::ProcessingWorkerReleased));assert!(!s.traces[1].marks.iter().any(|m|m.stage==Stage::CaptionRenderReceiptProxy));
}

#[test]
fn source_checkpoint_is_not_ready_and_does_not_count_usage() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); checkpoint(&h, "s");
    let saved = h.db.get_result("s").unwrap();
    assert_eq!(saved.source, source("s")); assert!(saved.output.is_none());
    assert_eq!(saved.session.status, StoredSessionStatus::Checkpointed);
    assert_eq!(h.db.usage_today("2026-10-04").unwrap().count, 0);
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().pending.is_none());
}

#[test]
fn ready_commits_output_and_once_usage_without_mutating_source() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
    h.commit_output("s", 7, output(&h, "s")).unwrap();
    h.initial_delivery("s", 7);
    assert_eq!(view(&h).state, SessionPhase::Ready); assert_eq!(usage_count(&h), 1);
    assert_eq!(h.db.get_source("source-s").unwrap(), source("s"));
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().id, "output-s");
}

#[test]
fn cancellation_first_rejects_late_checkpoint_and_ready() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true);
    assert_eq!(h.cancel_session(&identity("s")).unwrap()["status"], "cancelled");
    assert!(h.checkpoint("s", 7, source("s")).is_err());
    assert!(h.commit_output("s", 7, output(&h, "s")).is_err());
    assert_eq!(view(&h).state, SessionPhase::Cancelled); assert_eq!(usage_count(&h), 0);
    assert!(h.db.get_source("source-s").is_err());
}

#[test]
fn cancellation_after_checkpoint_removes_only_unadopted_content() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); checkpoint(&h, "s");
    h.cancel_session(&identity("s")).unwrap();
    assert!(h.db.get_result("s").is_err());
    assert!(h.commit_output("s", 7, output(&h, "s")).is_err());
    assert_eq!(usage_count(&h), 0); assert!(!view(&h).can_copy_recovery);
}

#[test]
fn ready_first_cancellation_reports_already_completed_and_keeps_usage() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
    assert_eq!(h.cancel_session(&identity("s")).unwrap()["status"], "alreadyCompleted");
    assert_eq!(view(&h).state, SessionPhase::Ready); assert_eq!(usage_count(&h), 1);
    assert_eq!(h.db.get_result("s").unwrap().source, source("s"));
}

#[test]
fn concurrent_cancel_and_ready_have_one_serialized_consistent_outcome() {
    for _ in 0..24 {
        let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); checkpoint(&h, "s");
        let candidate = output(&h, "s"); let barrier = Arc::new(Barrier::new(3));
        let a = h.clone(); let gate = barrier.clone();
        let committing = std::thread::spawn(move || { gate.wait(); a.commit_output("s", 7, candidate) });
        let a = h.clone(); let gate = barrier.clone();
        let cancelling = std::thread::spawn(move || { gate.wait(); a.cancel_session(&identity("s")) });
        barrier.wait(); let commit = committing.join().unwrap(); let cancel = cancelling.join().unwrap().unwrap();
        if commit.is_ok() {
            assert_eq!(cancel["status"], "alreadyCompleted"); assert_eq!(view(&h).state, SessionPhase::Ready);
            assert_eq!(usage_count(&h), 1); assert!(h.db.get_result("s").unwrap().output.is_some());
        } else {
            assert_eq!(cancel["status"], "cancelled"); assert_eq!(view(&h).state, SessionPhase::Cancelled);
            assert_eq!(usage_count(&h), 0); assert!(h.db.get_result("s").is_err());
        }
    }
}

#[test]
fn checkpoint_fault_keeps_pending_identity_and_no_partial_database_content() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true);
    fault(&h, "CREATE TRIGGER fail_checkpoint BEFORE INSERT ON source_items BEGIN SELECT RAISE(ABORT,'fault'); END;");
    let error = h.checkpoint("s", 7, source("s")).unwrap_err();
    assert_eq!(error.commit_stage.as_deref(), Some("sourceCheckpoint")); assert!(error.pending_commit_id.is_some());
    h.fail("s", 7, error.clone(), true);
    assert_eq!(view(&h).pending_commit_id, error.pending_commit_id); assert!(view(&h).can_copy_recovery);
    assert_eq!(view(&h).state, SessionPhase::Failed); assert_eq!(usage_count(&h), 0);
    assert!(h.db.get_result("s").is_err());
}

#[tokio::test]
async fn source_save_retry_preflight_failure_preserves_pending_and_is_not_busy() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true);
    fault(&h, "CREATE TRIGGER fail_checkpoint BEFORE INSERT ON source_items BEGIN SELECT RAISE(ABORT,'fault'); END;");
    let error = h.checkpoint("s", 7, source("s")).unwrap_err(); h.fail("s", 7, error, true);
    let pending = view(&h).pending_commit_id.unwrap(); fault(&h, "DROP TRIGGER fail_checkpoint;");
    let mut p = identity("s"); p["pendingCommitId"] = json!(pending);
    assert!(h.retry_save(&p).await.is_err());
    assert_eq!(view(&h).state, SessionPhase::Failed); assert_eq!(view(&h).pending_commit_id.as_deref(), Some(pending.as_str()));
    assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().busy());
    assert!(h.db.get_result("s").is_err());
    h.cancel_session(&identity("s")).unwrap(); assert!(h.retry_save(&p).await.is_err()); assert_eq!(usage_count(&h), 0);
}

#[tokio::test]
async fn ready_save_retry_uses_retained_output_without_cloud_or_automatic_delivery() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); checkpoint(&h, "s");
    fault(&h, "CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'fault'); END;");
    let generated = output(&h, "s"); let error = h.commit_output("s", 7, generated.clone()).unwrap_err();
    assert_eq!(error.commit_stage.as_deref(), Some("ready")); h.fail("s", 7, error, true);
    assert_eq!(usage_count(&h), 0); assert!(h.db.get_result("s").unwrap().output.is_none());
    let pending = view(&h).pending_commit_id.unwrap(); fault(&h, "DROP TRIGGER fail_usage;");
    let mut p = identity("s"); p["pendingCommitId"] = json!(pending);
    h.retry_save(&p).await.unwrap();
    let stored = h.db.get_result("s").unwrap().output.unwrap();
    assert_eq!(stored.id, generated.id); assert_eq!(stored.blocks, generated.blocks);
    assert_eq!(stored.creation_reason, CreationReason::Retry); assert_eq!(h.db.usage_today("2026-10-04").unwrap().count, 1);
    let count: u64 = h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap();
    assert_eq!(count, 0); assert!(h.inner.lock().unwrap().active.as_ref().unwrap().pending.is_none());
}

#[tokio::test]
async fn stale_save_identity_does_not_consume_retained_ready_output() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); checkpoint(&h, "s");
    fault(&h, "CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'fault'); END;");
    let error = h.commit_output("s", 7, output(&h, "s")).unwrap_err(); h.fail("s", 7, error, true);
    let original = view(&h).pending_commit_id; let mut p = identity("s"); p["pendingCommitId"] = json!("old-pending");
    assert_eq!(h.retry_save(&p).await.unwrap_err().code, "save_expired");
    assert_eq!(view(&h).pending_commit_id, original); assert_eq!(usage_count(&h), 0);
}

#[tokio::test]
async fn cancellation_revokes_retained_ready_save_and_prevents_late_adoption() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); checkpoint(&h, "s");
    fault(&h, "CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'fault'); END;");
    let error = h.commit_output("s", 7, output(&h, "s")).unwrap_err(); h.fail("s", 7, error, true);
    let mut p = identity("s"); p["pendingCommitId"] = json!(view(&h).pending_commit_id.unwrap());
    fault(&h, "DROP TRIGGER fail_usage;"); h.cancel_session(&identity("s")).unwrap();
    assert!(h.retry_save(&p).await.is_err()); assert_eq!(view(&h).state, SessionPhase::Cancelled);
    assert_eq!(usage_count(&h), 0); assert!(h.db.get_result("s").is_err());
}

#[tokio::test]
async fn rewrite_retry_configuration_failure_leaves_failed_source_usable() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); checkpoint(&h, "s");
    h.fail("s", 7, ApiError::new("rewrite_timeout", "rewrite", "整理未完成"), true);
    let mut p = identity("s"); p["sourceRevisionId"] = json!("source-s");
    assert!(h.retry_rewrite(&p).await.is_err());
    assert_eq!(view(&h).state, SessionPhase::Failed); assert!(view(&h).can_retry_rewrite); assert!(view(&h).can_use_source);
    assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().busy()); assert_eq!(usage_count(&h), 0);
}

#[tokio::test]
async fn asr_retry_configuration_failure_does_not_advance_generation_or_attempt() {
    let h = hub(); install(&h, "s", SessionPhase::Incomplete, true);
    { let mut i = h.inner.lock().unwrap(); let s = i.active.as_mut().unwrap(); s.cache = Some(vec![0; 1600]);
      s.cache_complete = true; s.cache_created = Some(Instant::now()); }
    let before = view(&h); let mut p = identity("s"); p["failedAttemptId"] = json!("attempt-s");
    assert!(h.retry_asr(&p).await.is_err());
    let after = view(&h); assert_eq!(after.state, before.state); assert_eq!(after.generation, before.generation);
    assert_eq!(after.asr_attempt_id, before.asr_attempt_id); assert!(after.can_retry_asr);
    assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().busy());
}

#[tokio::test]
async fn incomplete_or_expired_audio_cache_cannot_start_an_asr_retry() {
    for (complete, age) in [(false, 0), (true, 1801)] {
        let h = hub(); install(&h, "s", SessionPhase::Incomplete, true);
        { let mut i = h.inner.lock().unwrap(); let s = i.active.as_mut().unwrap(); s.cache = Some(vec![0; 1600]);
          s.cache_complete = complete; s.cache_created = Some(Instant::now() - Duration::from_secs(age)); }
        let mut p = identity("s"); p["failedAttemptId"] = json!("attempt-s");
        assert_eq!(h.retry_asr(&p).await.unwrap_err().code, "asr_retry_unavailable");
        assert_eq!(view(&h).generation, 7); assert_eq!(view(&h).asr_attempt_id.as_deref(), Some("attempt-s"));
    }
}

#[test]
fn stale_session_and_generation_cannot_cancel_current_capture() {
    let h = hub(); install(&h, "s", SessionPhase::Recording, false);
    for p in [json!({"sessionId":"older","expectedGeneration":7}), json!({"sessionId":"s","expectedGeneration":6})] {
        assert_eq!(h.cancel_session(&p).unwrap_err().code, "stale_session");
    }
    let i = h.inner.lock().unwrap(); let s = i.active.as_ref().unwrap();
    assert_eq!(s.phase, SessionPhase::Recording); assert!(!s.cancel.is_cancelled()); assert!(!s.stop.load(Ordering::SeqCst));
}

#[test]
fn double_stop_freezes_end_time_and_prevents_mode_changes() {
    let h = hub(); install(&h, "s", SessionPhase::Recording, false);
    h.select_session_mode(&json!({"sessionId":"s","expectedGeneration":7,"modeId":"light","revision":1})).unwrap();
    h.stop_with_capture(&identity("s"),||None).unwrap(); let first_end = h.inner.lock().unwrap().active.as_ref().unwrap().ended_at.clone();
    h.stop_with_capture(&identity("s"),||None).unwrap(); assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().ended_at, first_end);
    assert_eq!(view(&h).mode_snapshot.mode_id, "light"); assert_eq!(view(&h).state, SessionPhase::FinalizingAsr);
    assert_eq!(h.select_session_mode(&json!({"sessionId":"s","expectedGeneration":7,"modeId":"structure"})).unwrap_err().code, "mode_frozen");
}

#[test]
fn late_old_generation_failure_cannot_overwrite_new_live_state() {
    let h = hub(); install(&h, "new", SessionPhase::Recording, false);
    h.fail("old", 6, ApiError::new("rewrite_timeout", "rewrite", "old"), true);
    assert_eq!(view(&h).session_id, "new"); assert_eq!(view(&h).state, SessionPhase::Recording);
    assert!(!view(&h).worker_released); assert!(view(&h).error.is_none());
}

#[test]
fn explicit_use_source_counts_once_and_never_claims_delivery() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); checkpoint(&h, "s");
    h.fail("s", 7, ApiError::new("rewrite_timeout", "rewrite", "整理失败"), true);
    let mut p = identity("s"); p["sourceRevisionId"] = json!("source-s"); h.use_source(&p).unwrap();
    assert_eq!(view(&h).state, SessionPhase::Ready); assert_eq!(usage_count(&h), 1);
    let result = h.db.get_result("s").unwrap();
    let generated = result.output.unwrap();
    assert_eq!(generated.blocks.iter().map(|block|block.text.as_str()).collect::<Vec<_>>(),
        result.source.items.iter().map(|item|item.text.as_str()).collect::<Vec<_>>());
    assert_eq!(generated.kind, RevisionKind::UseSource);
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().delivery.is_none());
}

#[tokio::test]
async fn read_commands_do_not_emit_changed_or_increment_sequence() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s"); let seen = events(&h);
    let sequence = h.snapshot("main").unwrap()["sequence"].clone();
    for (name, p) in [("state.snapshot", json!({})), ("preferences.get", json!({})), ("mode.list", json!({})),
        ("usage.today", json!({})), ("usage.total", json!({})), ("provider.status", json!({})), ("credential.status", json!({})),
        ("history.list", json!({"query":""})), ("history.get", json!({"sessionId":"s"}))] {
        command(&h, name, p).await.unwrap();
    }
    assert!(seen.lock().unwrap().is_empty()); assert_eq!(h.snapshot("main").unwrap()["sequence"], sequence);
}

#[tokio::test]
async fn failed_commands_do_not_emit_changed_or_change_preferences() {
    let h = hub(); let seen = events(&h);
    let before = h.db.preferences().unwrap();
    assert!(command(&h, "preferences.update", json!({"expectedVersion":0,"requestId":"bad","patch":{"keepHistory":false}})).await.is_err());
    assert!(command(&h, "history.get", json!({"sessionId":"missing"})).await.is_err());
    assert!(seen.lock().unwrap().is_empty()); assert_eq!(h.db.preferences().unwrap(), before);
}

#[tokio::test]
async fn recording_window_cannot_write_preferences_or_read_history() {
    let h = hub(); let seen = events(&h);
    for name in ["preferences.update", "history.get", "history.delete", "provider.configure", "credential.status"] {
        assert_eq!(h.command("recording", CommandRequest { name: name.into(), payload: json!({}) }).await.unwrap_err().code, "forbidden");
    }
    assert!(seen.lock().unwrap().is_empty()); assert_eq!(h.db.preferences().unwrap().version, 1);
}

#[tokio::test]
async fn repeated_write_request_commits_once_and_emits_one_changed_event() {
    let h = hub(); let seen = events(&h);
    let payload = json!({"expectedVersion":1,"requestId":"preferences-once","patch":{"keepHistory":false}});
    let first = command(&h, "preferences.update", payload.clone()).await.unwrap();
    let duplicate = command(&h, "preferences.update", payload).await.unwrap();
    assert_eq!(first, duplicate); assert_eq!(h.db.preferences().unwrap().version, 2);
    assert_eq!(seen.lock().unwrap().iter().filter(|e| e.kind == "changed").count(), 1);
}

#[tokio::test]
async fn request_identity_cannot_be_reused_for_different_write_parameters() {
    let h = hub();
    command(&h, "preferences.update", json!({"expectedVersion":1,"requestId":"same-request","patch":{"keepHistory":false}})).await.unwrap();
    assert!(command(&h, "preferences.update", json!({"expectedVersion":2,"requestId":"same-request","patch":{"keepHistory":true}})).await.is_err());
    assert_eq!(h.db.preferences().unwrap().version, 2); assert!(!h.db.preferences().unwrap().keep_history);
}

#[tokio::test]
async fn new_recording_waits_for_capture_resource_release_even_after_ready() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, false); ready(&h, "s");
    assert_eq!(h.start_session().await.unwrap_err().code, "session_busy");
    h.inner.lock().unwrap().active.as_mut().unwrap().worker_released = true;
    // Missing region preflight now runs; the test never enters native capture.
    assert_ne!(h.start_session().await.unwrap_err().code, "session_busy");
    assert_eq!(view(&h).session_id, "s");
}

#[tokio::test]
async fn terminal_delivery_still_blocks_new_capture_until_native_worker_returns() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
    { let mut i = h.inner.lock().unwrap(); let s = i.active.as_mut().unwrap();
      s.delivery = Some(DeliveryAttempt { id: "delivery-s".into(), session_id: "s".into(), generation: 7,
          output_revision_id: "output-s".into(), target_fingerprint: "opaque".into(), claimed_at: now(), state: DeliveryState::Skipped, reason: None });
      s.delivery_worker_released = false; }
    assert_eq!(h.start_session().await.unwrap_err().code, "session_busy");
    h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released = true;
    assert_ne!(h.start_session().await.unwrap_err().code, "session_busy");
}

#[tokio::test]
async fn prepared_and_dispatched_ledger_states_block_new_capture() {
    for state in [DeliveryState::Prepared, DeliveryState::Dispatched] {
        let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
        h.inner.lock().unwrap().active.as_mut().unwrap().delivery = Some(DeliveryAttempt {
            id: "delivery-s".into(), session_id: "s".into(), generation: 7, output_revision_id: "output-s".into(),
            target_fingerprint: "opaque".into(), claimed_at: now(), state, reason: None,
        });
        assert_eq!(h.start_session().await.unwrap_err().code, "session_busy");
    }
}

#[test]
fn ready_cancel_revokes_atomic_native_permit_and_prepared_ledger_together() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
    let attempt = DeliveryAttempt { id: "delivery-s".into(), session_id: "s".into(), generation: 7,
        output_revision_id: "output-s".into(), target_fingerprint: "opaque".into(), claimed_at: now(), state: DeliveryState::Prepared, reason: None };
    let permit = Arc::new(platform::delivery::DeliveryPermit::from_committed_attempt(attempt.id.clone(), 7));
    h.db.prepare_delivery(attempt.clone()).unwrap();
    { let mut i = h.inner.lock().unwrap();
      let s = i.active.as_mut().unwrap(); s.delivery = Some(attempt); s.delivery_permit = Some(permit.clone()); s.delivery_worker_released = false; }
    assert_eq!(h.cancel_session(&identity("s")).unwrap()["status"], "alreadyCompleted");
    assert!(permit.consumed()); assert_eq!(view(&h).delivery_state, Some(DeliveryState::Skipped));
    assert_eq!(h.db.delivery_attempt("delivery-s").unwrap().state, DeliveryState::Skipped);
    assert_eq!(usage_count(&h), 1); assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().delivery_worker_released);
}

#[tokio::test]
async fn cancellation_cleanup_failure_is_not_reported_as_deleted_and_can_be_retried() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); checkpoint(&h, "s"); let seen = events(&h);
    fault(&h, "CREATE TRIGGER fail_cleanup BEFORE DELETE ON sessions BEGIN SELECT RAISE(ABORT,'fault'); END;");
    assert!(h.cancel_session(&identity("s")).is_err());
    assert_ne!(view(&h).state, SessionPhase::Cancelled);
    assert_eq!(h.db.get_source("source-s").unwrap(), source("s"));
    assert!(!h.valid_active("s", 7)); assert_eq!(usage_count(&h), 0);
    assert!(seen.lock().unwrap().iter().any(|event| event.kind=="session" && event.payload["errorCode"]=="cancel_cleanup_failed"));
    let failed = view(&h); assert!(!failed.can_retry_save); assert!(!failed.can_retry_rewrite);
    assert!(!failed.can_use_source); assert!(!failed.can_retry_asr);
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().busy());
    assert!(h.start_session().await.is_err()); assert_eq!(view(&h).session_id, "s");
    fault(&h, "DROP TRIGGER fail_cleanup;");
    assert_eq!(h.cancel_session(&identity("s")).unwrap()["status"], "cancelled");
    assert!(h.db.get_result("s").is_err()); assert_eq!(view(&h).state, SessionPhase::Cancelled);
}

#[test]
fn custom_mode_revision_selected_for_capture_survives_later_edit_and_delete() {
    let h = hub(); install(&h, "s", SessionPhase::Recording, true);
    let original = h.db.save_mode(ModeDraft {
        id: None, name: "简报".into(), base_mode: BaseMode::Structure,
        rules: "保留条件，整理成分点。".into(), expected_revision: None,
    }).unwrap();
    h.select_session_mode(&json!({"sessionId":"s","expectedGeneration":7,"modeId":original.id,"revision":1})).unwrap();
    { h.db.save_mode(ModeDraft { id: Some(original.id.clone()), name: "新版简报".into(), base_mode: BaseMode::Light,
          rules: "保留顺序。".into(), expected_revision: Some(1) }).unwrap();
      h.db.delete_mode(&original.id, 2).unwrap(); }
    h.stop_with_capture(&identity("s"),||None).unwrap(); checkpoint(&h, "s"); h.commit_output("s", 7, output(&h, "s")).unwrap();
    let result = h.db.get_result("s").unwrap();
    assert_eq!(result.session.seed.mode_snapshot.revision, 1); assert_eq!(result.session.seed.mode_snapshot.rules, original.rules);
    assert_eq!(result.output.unwrap().mode_snapshot.name, "简报"); assert_eq!(result.source, source("s"));
}

#[test]
fn history_disabled_app_pipeline_only_keeps_latest_body_in_memory_and_counts_usage() {
    let h = hub();
    { let mut prefs = h.db.preferences().unwrap(); prefs.keep_history = false;
      h.db.save_preferences(prefs, 1).unwrap(); }
    install(&h, "s", SessionPhase::FinalizingAsr, true); checkpoint(&h, "s");
    assert_eq!(usage_count(&h), 0); assert!(h.db.latest_temporary_result().unwrap().is_none());
    h.commit_output("s", 7, output(&h, "s")).unwrap();
    assert_eq!(h.db.latest_temporary_result().unwrap().unwrap().session.seed.id, "s");
    for table in ["sessions", "sources", "source_items", "outputs"] {
        let count: u64 = h.db.query_count(&format!("SELECT COUNT(*) FROM {table}")).unwrap();
        assert_eq!(count, 0, "{table}");
    }
    assert_eq!(h.db.usage_today("2026-10-04").unwrap().count, 1);
}

fn candidate(hub: &AppHub, id: &str, run: &str) -> CancellationToken {
    let mut generated = output(hub, id); generated.id = run.into(); generated.base_revision_id = Some(format!("output-{id}"));
    generated.kind = RevisionKind::Adopt; generated.creation_reason = CreationReason::History;
    generated.blocks[0].text = "候选：会议未确认前，不要订机票；先保留行程。".into();
    let token = CancellationToken::new();
    hub.inner.lock().unwrap().candidate = Some(CandidateSlot { personalization:None,key: id.into(), run: run.into(), cancel: token.clone(),
        base: format!("output-{id}"), result: Some(HistoryCandidate{revision:generated,review_issues:vec![]}),adopting:None }); token
}

#[tokio::test]
async fn stale_candidate_cancel_does_not_cancel_new_run_and_run_id_is_required() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s"); let token = candidate(&h, "s", "run-new");
    command(&h, "history.cancelCandidate", json!({"sessionId":"s","runId":"run-old","requestId":"cancel-old"})).await.unwrap();
    assert!(!token.is_cancelled()); assert_eq!(h.inner.lock().unwrap().candidate.as_ref().unwrap().run, "run-new");
    assert!(command(&h, "history.cancelCandidate", json!({"sessionId":"s","requestId":"cancel-missing"})).await.is_err());
    assert!(!token.is_cancelled());
    command(&h, "history.cancelCandidate", json!({"sessionId":"s","runId":"run-new","requestId":"cancel-current"})).await.unwrap();
    assert!(token.is_cancelled()); assert!(h.inner.lock().unwrap().candidate.is_none());
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().id, "output-s");
}

#[tokio::test]
async fn adoption_checks_candidate_identity_as_well_as_unchanged_base() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s"); candidate(&h, "s", "candidate-new");
    assert!(command(&h, "revision.adopt", json!({"sessionId":"s","expectedCurrentRevisionId":"output-s",
        "candidateId":"candidate-old","requestId":"adopt-old"})).await.is_err());
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().id, "output-s");
    assert!(h.inner.lock().unwrap().candidate.is_some());
    command(&h, "revision.adopt", json!({"sessionId":"s","expectedCurrentRevisionId":"output-s",
        "candidateId":"candidate-new","requestId":"adopt-current"})).await.unwrap();
    let adopted = h.db.get_result("s").unwrap().output.unwrap();
    assert_ne!(adopted.id, "output-s"); assert_eq!(adopted.blocks[0].text, "候选：会议未确认前，不要订机票；先保留行程。");
    assert_eq!(adopted.base_revision_id.as_deref(), Some("output-s")); assert_eq!(adopted.kind, RevisionKind::Adopt);
    assert!(h.inner.lock().unwrap().candidate.is_none()); assert_eq!(usage_count(&h), 1);
}

#[tokio::test]
async fn history_rewrite_preflight_failure_does_not_leave_or_cancel_a_candidate_slot() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
    let p = json!({"sessionId":"s","sourceRevisionId":"source-s","baseRevisionId":"output-s","modeId":"light","runId":"failed-run","requestId":"failed-rewrite"});
    assert!(command(&h, "history.rewrite", p).await.is_err()); assert!(h.inner.lock().unwrap().candidate.is_none());
    let token = candidate(&h, "s", "existing-candidate");
    assert!(command(&h, "history.rewrite", json!({"sessionId":"s","sourceRevisionId":"source-s","baseRevisionId":"output-s",
        "modeId":"light","runId":"second-failed-run","requestId":"second-failed-rewrite"})).await.is_err());
    assert!(!token.is_cancelled()); assert_eq!(h.inner.lock().unwrap().candidate.as_ref().unwrap().run, "existing-candidate");
}

#[tokio::test]
async fn mode_preview_preflight_failure_does_not_leave_private_sample_in_pending_state() {
    let h = hub(); let p = json!({"draft":{"id":null,"name":"消息","baseMode":"light","rules":"保留原意。","expectedRevision":null},
        "sample":"私有示例，不应残留在请求缓存。","requestId":"failed-preview","runId":"preview-failed"});
    assert!(command(&h, "mode.preview", p).await.is_err()); assert!(h.inner.lock().unwrap().preview.is_none());
    let i = h.inner.lock().unwrap(); let entry = i.requests.get("main:failed-preview").unwrap();
    assert!(!entry.fingerprint.contains("私有示例")); assert!(entry.outcome.lock().unwrap().as_ref().unwrap().is_err());
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn concurrently_repeated_request_has_one_real_store_commit() {
    for _ in 0..12 {
        let h = hub(); let gate = Arc::new(tokio::sync::Barrier::new(3)); let mut jobs = vec![];
        for _ in 0..2 { let h = h.clone(); let gate = gate.clone();
            jobs.push(tokio::spawn(async move { gate.wait().await;
                command(&h, "preferences.update", json!({"expectedVersion":1,"requestId":"concurrent-once","patch":{"keepHistory":false}})).await
            })); }
        gate.wait().await;
        let first = tokio::time::timeout(Duration::from_secs(2), jobs.remove(0)).await.unwrap().unwrap().unwrap();
        let second = tokio::time::timeout(Duration::from_secs(2), jobs.remove(0)).await.unwrap().unwrap().unwrap();
        assert_eq!(first, second); assert_eq!(h.db.preferences().unwrap().version, 2);
    }
}

#[tokio::test]
async fn deleted_history_duplicate_edit_cannot_resurrect_cached_text() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
    let blocks = vec![OutputBlock { id: "block-0".into(), text: "私有手改文本。".into(), source_refs: vec!["item-s-0".into()] }];
    let edit = json!({"sessionId":"s","expectedCurrentRevisionId":"output-s","blocks":blocks,"requestId":"private-edit"});
    command(&h, "revision.edit", edit.clone()).await.unwrap();
    { let i = h.inner.lock().unwrap(); let record = i.requests.get("main:private-edit").unwrap();
      let outcome = record.outcome.lock().unwrap();
      assert!(matches!(outcome.as_ref().unwrap(), Ok(ReplayValue::Detail(id)) if id=="s"));
      assert!(!record.fingerprint.contains("私有手改文本")); }
    command(&h, "history.delete", json!({"sessionId":"s","requestId":"delete-private"})).await.unwrap();
    assert_eq!(command(&h, "revision.edit", edit).await.unwrap_err().code, "request_already_processed");
    assert!(h.db.get_result("s").is_err()); assert_eq!(usage_count(&h), 1);
}

#[tokio::test]
async fn history_off_replay_cache_cannot_recover_replaced_temporary_body() {
    let h = hub();
    { let mut prefs = h.db.preferences().unwrap(); prefs.keep_history = false;
      h.db.save_preferences(prefs, 1).unwrap(); }
    install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
    let blocks = vec![OutputBlock { id: "block-0".into(), text: "临时私有文本。".into(), source_refs: vec!["item-s-0".into()] }];
    let edit = json!({"sessionId":"s","expectedCurrentRevisionId":"output-s","blocks":blocks,"requestId":"temporary-edit"});
    command(&h, "revision.edit", edit.clone()).await.unwrap();
    install(&h, "next", SessionPhase::FinalizingAsr, true); ready(&h, "next");
    assert_eq!(command(&h, "revision.edit", edit).await.unwrap_err().code, "request_already_processed");
    assert!(h.db.get_result("s").is_err());
    assert_eq!(h.db.latest_temporary_result().unwrap().unwrap().session.seed.id, "next");
    assert_eq!(h.db.usage_today("2026-10-04").unwrap().count, 2);
    assert!(matches!(h.inner.lock().unwrap().requests.get("main:temporary-edit").unwrap().outcome.lock().unwrap().as_ref().unwrap(), Ok(ReplayValue::Detail(id)) if id=="s"));
}

#[tokio::test]
async fn deleting_ready_body_revokes_prepared_native_permission_but_waits_for_receipt() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
    let attempt = DeliveryAttempt { id: "delivery-s".into(), session_id: "s".into(), generation: 7,
        output_revision_id: "output-s".into(), target_fingerprint: "opaque".into(), claimed_at: now(), state: DeliveryState::Prepared, reason: None };
    let permit = Arc::new(platform::delivery::DeliveryPermit::from_committed_attempt(attempt.id.clone(), 7));
    h.db.prepare_delivery(attempt.clone()).unwrap();
    { let mut i = h.inner.lock().unwrap();
      let s = i.active.as_mut().unwrap(); s.delivery = Some(attempt); s.delivery_permit = Some(permit.clone()); s.delivery_worker_released = false; }
    command(&h, "history.delete", json!({"sessionId":"s","requestId":"delete-pending-delivery"})).await.unwrap();
    assert!(permit.consumed());
    assert!(matches!(view(&h).delivery_state, Some(DeliveryState::Skipped | DeliveryState::Unconfirmed)));
    assert!(h.db.get_result("s").is_err());
    assert!(matches!(h.db.delivery_attempt("delivery-s").unwrap().state,
        DeliveryState::Skipped | DeliveryState::Unconfirmed));
    assert_eq!(usage_count(&h), 1); assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().delivery_worker_released);
    assert_eq!(h.start_session().await.unwrap_err().code, "session_busy");
}

#[tokio::test]
async fn ipc_session_mutations_require_identity_instead_of_targeting_any_current_capture() {
    let h = hub(); install(&h, "s", SessionPhase::Recording, false); let seen = events(&h);
    for name in ["session.stop", "session.cancel", "session.selectMode"] {
        for p in [json!({"requestId":format!("{name}-missing-id"),"expectedGeneration":7,"modeId":"light"}),
                  json!({"requestId":format!("{name}-missing-generation"),"sessionId":"s","modeId":"light"})] {
            assert!(command(&h, name, p).await.is_err());
        }
    }
    assert_eq!(view(&h).state, SessionPhase::Recording); assert_eq!(view(&h).mode_snapshot.mode_id, "auto");
    assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled()); assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ready_waits_for_the_processing_worker_exit_before_new_capture() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
    h.inner.lock().unwrap().active.as_mut().unwrap().processing_worker_released = false;
    assert!(!view(&h).worker_released); assert_eq!(h.start_session().await.unwrap_err().code, "session_busy");
    h.inner.lock().unwrap().active.as_mut().unwrap().processing_worker_released = true;
    assert!(view(&h).worker_released); assert_ne!(h.start_session().await.unwrap_err().code, "session_busy");
}

#[tokio::test]
async fn native_operation_gate_blocks_new_capture_until_its_mutex_is_released() {
    let h = hub(); install(&h, "s", SessionPhase::FinalizingAsr, true); ready(&h, "s");
    let native_operation = h.native_io.lock().unwrap();
    assert_eq!(h.start_session().await.unwrap_err().code, "native_busy");
    drop(native_operation);
    assert_ne!(h.start_session().await.unwrap_err().code, "native_busy");
    assert_eq!(view(&h).session_id, "s");
}

#[tokio::test]
async fn shutdown_closes_start_gate_before_configuration_target_or_credential_access() {
    let h = hub();
    let epoch = h.lifecycle_epoch.load(Ordering::SeqCst);
    h.shutdown().unwrap();
    assert!(!h.capture_available.load(Ordering::SeqCst));
    assert!(h.lifecycle_epoch.load(Ordering::SeqCst) > epoch);
    // Missing region protects the test against native I/O even if the shutdown
    // gate regresses, and makes a wrong preflight order observable by its code.
    assert_eq!(h.start_session().await.unwrap_err().code, "system_unavailable");
    assert_eq!(command(&h, "session.start", json!({"requestId":"queued-toggle-after-quit"})).await.unwrap_err().code, "system_unavailable");
    assert!(h.inner.lock().unwrap().active.is_none());
}

#[tokio::test]
async fn late_locked_handler_cannot_override_a_newer_completed_resume_epoch() {
    let h = hub(); let seen = events(&h);
    let locked = h.reserve_lifecycle_epoch();
    let resumed = h.reserve_lifecycle_epoch(); assert!(resumed > locked);
    h.interrupt_session_at_epoch("Resumed", resumed).await.unwrap();
    assert!(h.capture_available.load(Ordering::SeqCst));
    let restored = h.snapshot("main").unwrap();
    h.interrupt_session_at_epoch("Locked", locked).await.unwrap();
    assert!(h.capture_available.load(Ordering::SeqCst));
    assert_eq!(h.lifecycle_epoch.load(Ordering::SeqCst), resumed);
    assert_eq!(h.snapshot("main").unwrap(), restored);
    let emitted = seen.lock().unwrap();
    assert_eq!(emitted.iter().filter(|event|event.kind=="interrupted").count(), 1);
    assert_eq!(emitted.iter().find(|event|event.kind=="interrupted").unwrap().payload["reason"], "Resumed");
}

fn preview_payload(run:&str)->Value {json!({"runId":run,"requestId":Uuid::new_v4().to_string(),
    "draft":{"id":null,"name":"消息","baseMode":"light","rules":"保留条件和否定。","expectedRevision":null},"sample":"会议未确认，不要订机票。"})}

#[tokio::test]
async fn preview_cancel_before_start_and_consumed_id_precede_credential_preflight() {
    let h=hub();command(&h,"mode.cancelPreview",json!({"runId":"late","requestId":"cancel-first"})).await.unwrap();
    assert_eq!(command(&h,"mode.preview",preview_payload("late")).await.unwrap_err().code,"operation_expired");
    assert!(h.inner.lock().unwrap().preview.is_none());assert!(h.inner.lock().unwrap().auxiliary_workers.is_empty());
    let first=command(&h,"mode.preview",preview_payload("failed")).await.unwrap_err();assert_ne!(first.code,"operation_expired");
    // region=None was asserted by the fixture: this cannot touch any stored Key.
    assert_eq!(command(&h,"mode.preview",preview_payload("failed")).await.unwrap_err().code,"operation_expired");
}

#[tokio::test]
async fn preview_old_cancel_and_failed_preflight_preserve_new_preview() {
    let h=hub();let token=CancellationToken::new();h.inner.lock().unwrap().preview=Some(("new".into(),token.clone()));
    command(&h,"mode.cancelPreview",json!({"runId":"old","requestId":"cancel-old"})).await.unwrap();
    assert!(!token.is_cancelled());assert_eq!(h.inner.lock().unwrap().preview.as_ref().unwrap().0,"new");
    assert!(command(&h,"mode.preview",preview_payload("preflight-fail")).await.is_err());
    assert!(!token.is_cancelled());assert_eq!(h.inner.lock().unwrap().preview.as_ref().unwrap().0,"new");
    command(&h,"mode.cancelPreview",json!({"runId":"new","requestId":"cancel-new"})).await.unwrap();
    assert!(token.is_cancelled());assert!(h.inner.lock().unwrap().preview.is_none());
}

#[tokio::test]
async fn preview_identity_is_required_and_shutdown_does_not_start_preflight() {
    let h=hub();let mut p=preview_payload("x");p.as_object_mut().unwrap().remove("runId");
    assert_eq!(command(&h,"mode.preview",p).await.unwrap_err().code,"invalid_payload");
    h.shutdown().unwrap();
    assert_eq!(command(&h,"mode.preview",preview_payload("after-quit")).await.unwrap_err().code,"system_unavailable");
    assert!(h.inner.lock().unwrap().used_operation_ids.is_empty());
}

#[tokio::test]
async fn candidate_cancel_before_start_invalidates_run_without_credentials() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");
    command(&h,"history.cancelCandidate",json!({"sessionId":"s","runId":"late","requestId":"candidate-cancel-first"})).await.unwrap();
    let error=command(&h,"history.rewrite",json!({"sessionId":"s","runId":"late","modeId":"light","requestId":"candidate-late"})).await.unwrap_err();
    assert_eq!(error.code,"operation_expired");assert!(h.inner.lock().unwrap().candidate.is_none());assert_eq!(usage_count(&h),1);
}

#[test]
fn shortcut_capture_cancel_before_begin_invalidates_id_without_native_ack() {
    let h=hub();h.cancel_shortcut_capture_with(&json!({"captureId":"late"}),||panic!("there is no native owner")).unwrap();
    assert_eq!(h.begin_shortcut_capture_with(&json!({"captureId":"late"}),||panic!("cancelled identity must not reach ON")).unwrap_err().code,"operation_expired");
    assert!(h.inner.lock().unwrap().shortcut_capture.is_none());
}

#[tokio::test]
async fn shortcut_old_cancel_cannot_release_new_capture_and_cached_begin_expires() {
    let h=hub();let seen=events(&h);
    // ACK closures exercise the actual identity controller without installing a
    // global keyboard hook. They are not native integration acceptance.
    h.begin_shortcut_capture_with(&json!({"captureId":"old"}),||Ok(())).unwrap();
    h.cancel_shortcut_capture_with(&json!({"captureId":"old"}),||Ok(())).unwrap();
    assert!(h.replay(ReplayValue::Capture("old".into())).await.is_err());
    h.begin_shortcut_capture_with(&json!({"captureId":"new"}),||Ok(())).unwrap();
    h.cancel_shortcut_capture_with(&json!({"captureId":"old"}),||panic!("stale cancel cannot send OFF")).unwrap();
    assert_eq!(h.inner.lock().unwrap().shortcut_capture.as_deref(),Some("new"));
    assert_eq!(h.replay(ReplayValue::Capture("new".into())).await.unwrap()["capturing"],true);
    h.cancel_shortcut_capture_with(&json!({"captureId":"new"}),||Ok(())).unwrap();
    assert_eq!(seen.lock().unwrap().iter().filter(|e|e.kind=="shortcut.captureCancelled").count(),2);
}

#[tokio::test]
async fn shortcut_off_ack_failure_retains_capture_gate_and_retry_releases_it() {
    let h=hub();h.begin_shortcut_capture_with(&json!({"captureId":"owner"}),||Ok(())).unwrap();
    let failed=h.cancel_shortcut_capture_with(&json!({"captureId":"owner"}),||Err(ApiError::new("ack_timeout","native","test ACK timeout")));
    assert_eq!(failed.unwrap_err().code,"ack_timeout");assert_eq!(h.inner.lock().unwrap().shortcut_capture.as_deref(),Some("owner"));
    assert_eq!(h.start_session().await.unwrap_err().code,"capture_busy");
    assert_eq!(h.toggle_from_hotkey().await.unwrap_err().code,"capture_busy");
    assert_eq!(h.begin_shortcut_capture_with(&json!({"captureId":"new"}),||panic!("capture admission closed")).unwrap_err().code,"capture_busy");
    h.cancel_shortcut_capture_with(&json!({"captureId":"owner"}),||Ok(())).unwrap();
    assert_ne!(h.start_session().await.unwrap_err().code,"capture_busy");
}

#[test]
fn shortcut_on_ack_failure_requires_explicit_off_ack_before_reuse() {
    let h=hub();assert!(h.begin_shortcut_capture_with(&json!({"captureId":"owner"}),||Err(ApiError::new("ack_timeout","native","test ON timeout"))).is_err());
    assert_eq!(h.inner.lock().unwrap().shortcut_capture.as_deref(),Some("owner"));
    h.cancel_shortcut_capture_with(&json!({"captureId":"owner"}),||Ok(())).unwrap();
    assert!(h.inner.lock().unwrap().shortcut_capture.is_none());
    assert_eq!(h.begin_shortcut_capture_with(&json!({"captureId":"owner"}),||panic!("consumed ID")).unwrap_err().code,"operation_expired");
}

#[test]
fn shortcut_capture_rejects_active_session_and_exit_before_native_access() {
    let h=hub();install(&h,"s",SessionPhase::Recording,true);
    assert_eq!(h.begin_shortcut_capture_with(&json!({"captureId":"during-recording"}),||panic!("recording admission closed")).unwrap_err().code,"capture_busy");
    h.shutdown().unwrap();assert_eq!(h.begin_shortcut_capture_with(&json!({"captureId":"after-exit"}),||panic!("exit admission closed")).unwrap_err().code,"system_unavailable");
    assert!(h.inner.lock().unwrap().shortcut_capture.is_none());
}

#[tokio::test]
async fn shortcut_save_checks_only_actual_changes_and_current_capture_identity() {
    let h=hub();h.begin_shortcut_capture_with(&json!({"captureId":"new"}),||Ok(())).unwrap();
    let error=command(&h,"preferences.update",json!({"captureId":"old","expectedVersion":1,"patch":{"shortcut":"shift_right"},"requestId":"stale-save"})).await.unwrap_err();
    assert_eq!(error.code,"capture_expired");assert_eq!(h.db.preferences().unwrap().version,1);
    // Full frontend patches carry unchanged shortcut even for language saves.
    let saved=command(&h,"preferences.update",json!({"expectedVersion":1,"patch":{"shortcut":"alt_right","language":"en"},"requestId":"language-save"})).await.unwrap();
    assert_eq!(saved["version"],2);assert_eq!(saved["language"],"en");
    assert_eq!(h.inner.lock().unwrap().shortcut_capture.as_deref(),Some("new"));
    h.cancel_shortcut_capture_with(&json!({"captureId":"new"}),||Ok(())).unwrap();
}

#[tokio::test]
async fn shortcut_reset_without_capture_id_remains_available() {
    let h=hub();command(&h,"preferences.update",json!({"expectedVersion":1,"patch":{"language":"en"},"requestId":"change-language"})).await.unwrap();
    let reset=command(&h,"preferences.reset",json!({"expectedVersion":2,"requestId":"reset"})).await.unwrap();
    assert_eq!(reset["language"],"auto");assert_eq!(reset["shortcut"],"alt_right");
}

#[test]
fn native_focus_boundary_is_atomic_and_stale_worker_cannot_cancel_new_capture() {
    let h=hub();h.begin_shortcut_capture_with(&json!({"captureId":"old"}),||Ok(())).unwrap();
    // Even with the admission mutex held by an ACK worker, the UI callback
    // returns immediately because it touches only atomics.
    let guard=h.inner.lock().unwrap();let blurred=h.shortcut_capture_focus_changed(false);drop(guard);
    assert_eq!(h.begin_shortcut_capture_with(&json!({"captureId":"hidden"}),||panic!("hidden admission closed")).unwrap_err().code,"capture_unavailable");
    h.shortcut_capture_focus_changed(true);h.cancel_shortcut_capture_with(&json!({"captureId":"old"}),||Ok(())).unwrap();
    h.begin_shortcut_capture_with(&json!({"captureId":"new"}),||Ok(())).unwrap();
    h.cancel_shortcut_capture_at_focus_epoch_with(blurred,||panic!("late focus worker cannot send OFF")).unwrap();
    assert_eq!(h.inner.lock().unwrap().shortcut_capture.as_deref(),Some("new"));
    let current=h.shortcut_capture_focus_changed(false);h.cancel_shortcut_capture_at_focus_epoch_with(current,||Ok(())).unwrap();
    assert!(h.inner.lock().unwrap().shortcut_capture.is_none());
}

#[test]
fn native_focus_off_failure_preserves_owner_until_actual_ack() {
    let h=hub();h.begin_shortcut_capture_with(&json!({"captureId":"owner"}),||Ok(())).unwrap();let epoch=h.shortcut_capture_focus_changed(false);
    assert!(h.cancel_shortcut_capture_at_focus_epoch_with(epoch,||Err(ApiError::new("ack_timeout","native","test OFF timeout"))).is_err());
    assert_eq!(h.inner.lock().unwrap().shortcut_capture.as_deref(),Some("owner"));
    h.cancel_shortcut_capture_at_focus_epoch_with(epoch,||Ok(())).unwrap();assert!(h.inner.lock().unwrap().shortcut_capture.is_none());
}

struct ResourceProbe {hub:Arc<AppHub>,key:String,release:oneshot::Receiver<()>,drop_observed:Arc<AtomicBool>}
impl Future for ResourceProbe {type Output=();fn poll(mut self:Pin<&mut Self>,cx:&mut std::task::Context<'_>)->std::task::Poll<()> {Pin::new(&mut self.release).poll(cx).map(|_|())}}
impl Drop for ResourceProbe {fn drop(&mut self){assert!(self.hub.inner.lock().unwrap().auxiliary_workers.contains_key(&self.key),"network future must drop before its lease");self.drop_observed.store(true,Ordering::SeqCst);}}
fn tracked_resource(h:&Arc<AppHub>,key:&str)->(AuxiliaryFuture<ResourceProbe>,oneshot::Sender<()>,CancellationToken,Arc<AtomicBool>){
    let (tx,rx)=oneshot::channel();let token=CancellationToken::new();let dropped=Arc::new(AtomicBool::new(false));
    h.inner.lock().unwrap().auxiliary_workers.insert(key.into(),token.clone());
    (AuxiliaryFuture{future:Some(Box::pin(ResourceProbe{hub:h.clone(),key:key.into(),release:rx,drop_observed:dropped.clone()})),lease:Some(AuxiliaryLease{hub:h.clone(),key:key.into()})},tx,token,dropped)
}

#[tokio::test]
async fn auxiliary_completion_and_abort_drop_actual_future_before_release() {
    let h=hub();let (future,release,_,dropped)=tracked_resource(&h,"preview:completed");let task=tokio::spawn(future);
    release.send(()).unwrap();task.await.unwrap();assert!(dropped.load(Ordering::SeqCst));assert!(h.inner.lock().unwrap().auxiliary_workers.is_empty());
    let (future,_release,_,dropped)=tracked_resource(&h,"candidate:s:aborted");let task=tokio::spawn(future);task.abort();assert!(task.await.unwrap_err().is_cancelled());
    assert!(dropped.load(Ordering::SeqCst));assert!(h.inner.lock().unwrap().auxiliary_workers.is_empty());
}

#[tokio::test]
async fn clearing_replaced_preview_slot_does_not_release_its_live_request() {
    let h=hub();let (future,release,old,dropped)=tracked_resource(&h,"preview:replaced");
    let current=CancellationToken::new();h.inner.lock().unwrap().preview=Some(("current".into(),current.clone()));let task=tokio::spawn(future);
    h.cancel_auxiliary();assert!(old.is_cancelled());assert!(current.is_cancelled());assert!(h.inner.lock().unwrap().preview.is_none());
    assert!(!dropped.load(Ordering::SeqCst));assert_eq!(h.inner.lock().unwrap().auxiliary_workers.len(),1);
    assert_eq!(h.start_session().await.unwrap_err().code,"processing_busy");release.send(()).unwrap();task.await.unwrap();
    assert!(h.inner.lock().unwrap().auxiliary_workers.is_empty());
}

#[tokio::test]
async fn shutdown_waits_for_all_cancelled_auxiliary_futures_to_actually_exit() {
    let h=hub();let (future,release,cancel,dropped)=tracked_resource(&h,"preview:old-replaced");let worker=tokio::spawn(future);
    let (other,release_other,other_cancel,_)=tracked_resource(&h,"candidate:s:pending");let other_worker=tokio::spawn(other);
    let closing_hub=h.clone();let closing=tokio::spawn(async move{closing_hub.shutdown_ready().await});
    tokio::time::timeout(Duration::from_secs(1),cancel.cancelled()).await.unwrap();assert!(other_cancel.is_cancelled());
    assert!(!closing.is_finished());assert!(!dropped.load(Ordering::SeqCst));
    release.send(()).unwrap();worker.await.unwrap();assert!(!closing.is_finished());
    release_other.send(()).unwrap();other_worker.await.unwrap();tokio::time::timeout(Duration::from_secs(1),closing).await.unwrap().unwrap().unwrap();
    assert!(h.inner.lock().unwrap().auxiliary_workers.is_empty());assert!(h.quitting.load(Ordering::SeqCst));
}

#[tokio::test]
async fn lock_waits_for_auxiliary_exit_and_resume_cannot_reopen_quitting_hub() {
    let h=hub();let (future,release,cancel,_)=tracked_resource(&h,"preview:lock");let worker=tokio::spawn(future);
    let locked=h.reserve_lifecycle_epoch();let interrupted_hub=h.clone();let interrupted=tokio::spawn(async move{interrupted_hub.interrupt_session_at_epoch("Locked",locked).await});
    tokio::time::timeout(Duration::from_secs(1),cancel.cancelled()).await.unwrap();assert!(!interrupted.is_finished());assert!(!h.capture_available.load(Ordering::SeqCst));
    release.send(()).unwrap();worker.await.unwrap();tokio::time::timeout(Duration::from_secs(1),interrupted).await.unwrap().unwrap().unwrap();
    assert!(!h.capture_available.load(Ordering::SeqCst));h.shutdown_ready().await.unwrap();let resumed=h.reserve_lifecycle_epoch();
    h.interrupt_session_at_epoch("Resumed",resumed).await.unwrap();assert!(!h.capture_available.load(Ordering::SeqCst));
}

fn source_only_candidate(h:&AppHub,id:&str,run:&str){
    let mut value=output(h,id);value.id=run.into();value.mode_snapshot=h.db.mode_snapshot("light",None).unwrap();
    value.resolved_mode=BaseMode::Light;value.kind=RevisionKind::Adopt;value.creation_reason=CreationReason::History;
    value.base_revision_id=None;value.parent_revision_id=None;
    h.inner.lock().unwrap().candidate=Some(CandidateSlot{personalization:None,key:id.into(),run:run.into(),base:String::new(),cancel:CancellationToken::new(),result:Some(HistoryCandidate{revision:value,review_issues:vec![]}),adopting:None});
}
fn first_adopt_payload(id:&str,run:&str)->Value{json!({"sessionId":id,"expectedCurrentRevisionId":"","candidateId":run,"requestId":Uuid::new_v4().to_string()})}

#[tokio::test]
async fn first_history_adoption_syncs_active_view_once_without_delivery_or_early_release() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
    h.fail("s",7,ApiError::new("rewrite_timeout","rewrite","test failure"),true);assert!(view(&h).can_retry_rewrite);
    source_only_candidate(&h,"s","first");let original_cancel=h.inner.lock().unwrap().active.as_ref().unwrap().cancel.clone();
    h.inner.lock().unwrap().active.as_mut().unwrap().processing_worker_released=false;
    let saved=command(&h,"revision.adopt",first_adopt_payload("s","first")).await.unwrap();
    assert_eq!(saved["output"]["modeSnapshot"]["modeId"],"light");assert_eq!(saved["canUndo"],false);
    let state=view(&h);assert_eq!(state.state,SessionPhase::Ready);assert!(state.error.is_none());assert!(state.error_code.is_none());
    assert!(!state.can_retry_rewrite);assert!(!state.can_use_source);assert!(!state.worker_released);assert!(original_cancel.is_cancelled());
    assert_eq!(state.mode_snapshot.mode_id,"auto","capture Mode remains frozen");assert_eq!(state.delivery_state,None);assert_eq!(usage_count(&h),1);
    let deliveries:u64=h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap();assert_eq!(deliveries,0);
    assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().output.as_ref().unwrap().id,saved["output"]["id"].as_str().unwrap());
    assert!(command(&h,"revision.adopt",first_adopt_payload("s","first")).await.is_err());assert_eq!(usage_count(&h),1);
    // A late pipeline completion is invalidated; its lease alone releases busy.
    assert!(h.commit_output("s",7,output(&h,"s")).is_err());
    let lease=ProcessingLease{hub:h.clone(),id:"s".into(),generation:7,trace:None};drop(lease);assert!(view(&h).worker_released);
}

#[tokio::test]
async fn first_history_adoption_db_failure_preserves_source_candidate_and_active_error() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");h.fail("s",7,ApiError::new("rewrite_timeout","rewrite","test failure"),true);
    source_only_candidate(&h,"s","first");fault(&h,"CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'fault'); END;");
    assert!(command(&h,"revision.adopt",first_adopt_payload("s","first")).await.is_err());
    assert_eq!(view(&h).state,SessionPhase::Failed);assert_eq!(view(&h).error_code.as_deref(),Some("rewrite_timeout"));assert_eq!(usage_count(&h),0);
    assert!(h.inner.lock().unwrap().candidate.is_some());assert!(h.db.get_result("s").unwrap().output.is_none());assert_eq!(h.db.get_result("s").unwrap().source,source("s"));
    fault(&h,"DROP TRIGGER fail_usage;");command(&h,"revision.adopt",first_adopt_payload("s","first")).await.unwrap();assert_eq!(usage_count(&h),1);
}

#[tokio::test]
async fn historical_first_adoption_does_not_change_an_unrelated_current_recording() {
    let h=hub();install(&h,"old",SessionPhase::FinalizingAsr,true);checkpoint(&h,"old");source_only_candidate(&h,"old","first");
    install(&h,"current",SessionPhase::Recording,true);let current_token=h.inner.lock().unwrap().active.as_ref().unwrap().cancel.clone();
    command(&h,"revision.adopt",first_adopt_payload("old","first")).await.unwrap();
    assert_eq!(view(&h).id,"current");assert_eq!(view(&h).state,SessionPhase::Recording);assert!(!current_token.is_cancelled());assert_eq!(usage_count(&h),1);
}

#[tokio::test]
async fn cancelled_or_deleted_source_only_candidate_cannot_resurrect_ready() {
    for deleted in [false,true]{let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");source_only_candidate(&h,"s","first");
        if deleted{command(&h,"history.delete",json!({"sessionId":"s","requestId":"delete"})).await.unwrap();}else{h.cancel_session(&identity("s")).unwrap();}
        assert!(command(&h,"revision.adopt",first_adopt_payload("s","first")).await.is_err());assert_eq!(usage_count(&h),0);assert_eq!(view(&h).state,SessionPhase::Cancelled);
        assert!(h.db.get_result("s").is_err());
    }
}

#[tokio::test]
async fn session_error_keeps_origin_identity_and_global_preflight_is_application_scoped() {
    let h=hub();install(&h,"old",SessionPhase::FinalizingAsr,true);h.fail("old",7,ApiError::new("rewrite_timeout","rewrite","test failure"),true);
    let produced=h.inner.lock().unwrap().active.as_ref().unwrap().error.clone().unwrap();install(&h,"new",SessionPhase::Recording,true);
    let payload=serde_json::to_value(produced).unwrap();assert_eq!(payload["scope"],"session");assert_eq!(payload["sessionId"],"old");assert_eq!(payload["generation"],7);
    let stale=command(&h,"session.stop",json!({"sessionId":"old","expectedGeneration":7,"requestId":"late-stop"})).await.unwrap_err();
    assert_eq!(stale.scope.as_deref(),Some("session"));assert_eq!(stale.session_id.as_deref(),Some("old"));assert_eq!(view(&h).state,SessionPhase::Recording);
    let h=hub();let failed=h.start_session().await.unwrap_err();assert_eq!(failed.scope.as_deref(),Some("application"));assert!(failed.session_id.is_none());
}

#[test]
fn ready_storage_failure_is_bound_to_the_producing_retry_generation() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);{let mut i=h.inner.lock().unwrap();i.active.as_mut().unwrap().generation=8;i.guards.insert("s".into(),Arc::new(CommitGuard::new("s".into(),8)));}
    let mut recovered=source("s");recovered.generation=8;h.checkpoint("s",8,recovered).unwrap();
    fault(&h,"CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'fault'); END;");
    let failure=h.commit_output("s",8,output(&h,"s")).unwrap_err();
    assert_eq!(failure.scope.as_deref(),Some("session"));assert_eq!(failure.session_id.as_deref(),Some("s"));assert_eq!(failure.generation,Some(8));
    assert_eq!(failure.commit_stage.as_deref(),Some("ready"));assert!(failure.can_retry_save);assert_eq!(usage_count(&h),0);
}

#[test]
fn recording_limit_warns_from_actual_sample_time_then_preserves_partial_without_ready() {
    let h=hub();install(&h,"limit",SessionPhase::Recording,false);
    h.inner.lock().unwrap().active.as_mut().unwrap().captions.push(CaptionItem{id:"pending".into(),revision:1,capture_order:0,text:"如果没有确认，不要发邀请。".into(),is_final:false});
    assert!(!h.recording_progress("limit",7,569_999));
    assert_eq!(view(&h).recording_limit_remaining_ms,None);
    assert!(!h.recording_progress("limit",7,570_000));
    assert_eq!(view(&h).recording_limit_remaining_ms,Some(30_000));
    assert!(!h.recording_progress("limit",7,599_999));
    assert_eq!(view(&h).recording_limit_remaining_ms,Some(1));
    assert!(h.recording_progress("limit",7,600_000));
    let result=view(&h);assert_eq!(result.state,SessionPhase::Incomplete);
    assert_eq!(result.error_code.as_deref(),Some("recording_limit"));
    assert!(result.partial.contains("不要发邀请"));assert!(result.can_copy_recovery);
    assert!(!result.worker_released);assert!(!result.can_retry_asr);assert!(!result.can_use_source);
    assert_eq!(usage_count(&h),0);assert!(h.db.get_result("limit").is_err());
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().busy());
    assert!(!h.recording_progress("limit",7,600_100));
}

fn file_hub()->(Arc<AppHub>,PathBuf) {
    let mut value=hub();
    let directory=value.data_dir.join("db-worker");
    std::fs::create_dir_all(&directory).unwrap();let path=directory.join(format!("{}.sqlite",Uuid::new_v4()));
    Arc::get_mut(&mut value).unwrap().db=DbClient::open(&path).unwrap();
    assert!(value.db.snapshot().provider_config.region.is_none());(value,path)
}
async fn wait_until(mut ready:impl FnMut()->bool){
    let deadline=Instant::now()+Duration::from_secs(2);
    while !ready(){assert!(Instant::now()<deadline,"owner did not reach the intended boundary");tokio::time::sleep(Duration::from_millis(5)).await;}
}
fn second_writer(path:&Path)->rusqlite::Connection {
    let writer=rusqlite::Connection::open(path).unwrap();writer.execute_batch("BEGIN IMMEDIATE;").unwrap();writer
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn sqlite_busy_checkpoint_does_not_hold_control_or_snapshot_and_cancel_prevents_ready() {
    let(h,path)=file_hub();install(&h,"blocked",SessionPhase::FinalizingAsr,true);let writer=second_writer(&path);
    let job_hub=h.clone();let checkpoint=tokio::spawn(async move{job_hub.checkpoint_async("blocked",7,source("blocked")).await});
    wait_until(||h.db.outstanding()>0).await;tokio::time::sleep(Duration::from_millis(30)).await;
    let began=Instant::now();let snapshot=h.snapshot("main").unwrap();h.stop_with_capture(&identity("blocked"),||None).unwrap();
    assert!(began.elapsed()<Duration::from_millis(250));assert_eq!(snapshot["session"]["sessionId"],"blocked");
    let job_hub=h.clone();let cancelled=tokio::spawn(async move{job_hub.cancel_session_async(&identity("blocked")).await});
    wait_until(||h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled()).await;
    assert!(view(&h).cancel_pending);assert!(!view(&h).worker_released);assert!(!cancelled.is_finished());
    // This is genuine SQLite contention in a separate connection, not a fake
    // DB provider or a scheduler sleep substituted for storage work.
    writer.execute_batch("ROLLBACK;").unwrap();checkpoint.await.unwrap().unwrap_err();
    assert_eq!(cancelled.await.unwrap().unwrap()["status"],"cancelled");
    assert_eq!(usage_count(&h),0);assert!(h.db.get_result("blocked").is_err());assert_eq!(view(&h).state,SessionPhase::Cancelled);
    h.shutdown_ready().await.unwrap();assert!(h.db.owner_released());
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn aborted_ipc_observer_still_reconciles_late_preferences_commit_and_deduplication() {
    let(h,path)=file_hub();let writer=second_writer(&path);
    let payload=json!({"expectedVersion":1,"patch":{"keepHistory":false},"requestId":"abort-prefs"});let request=payload.clone();let worker=h.clone();
    let observer=tokio::spawn(async move{command(&worker,"preferences.update",request).await});
    wait_until(||h.db.outstanding()>0).await;observer.abort();let _=observer.await;
    assert_eq!(h.snapshot("main").unwrap()["preferences"]["version"],1);assert!(h.inner.lock().unwrap().settings_pending.is_some());
    writer.execute_batch("ROLLBACK;").unwrap();wait_until(||h.db_command_workers.load(Ordering::SeqCst)==0).await;
    assert_eq!(h.db.snapshot().preferences.version,2);assert!(!h.db.snapshot().preferences.keep_history);assert!(h.inner.lock().unwrap().settings_pending.is_none());
    assert_eq!(command(&h,"preferences.update",payload).await.unwrap()["version"],2);
    h.shutdown_ready().await.unwrap();assert!(h.db.owner_released());
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn cancel_queued_adoption_revokes_only_its_child_and_later_run_can_adopt_source_once() {
    let(h,path)=file_hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");h.fail("s",7,ApiError::new("rewrite_timeout","rewrite","fixed fixture failure"),true);
    source_only_candidate(&h,"s","old");let writer=second_writer(&path);let worker=h.clone();
    let adopting=tokio::spawn(async move{command(&worker,"revision.adopt",first_adopt_payload("s","old")).await});
    wait_until(||h.inner.lock().unwrap().candidate.as_ref().is_some_and(|c|c.adopting.is_some())&&h.db.outstanding()>0).await;
    let cancelled=command(&h,"history.cancelCandidate",json!({"sessionId":"s","runId":"old","requestId":"cancel-adopt"})).await.unwrap();
    assert_eq!(cancelled["status"],"cancelled");assert!(!h.record_guard("s",7).is_cancelled());
    writer.execute_batch("ROLLBACK;").unwrap();assert_eq!(adopting.await.unwrap().unwrap_err().code,"cancelled");
    assert_eq!(usage_count(&h),0);assert!(h.db.get_result("s").unwrap().output.is_none());
    source_only_candidate(&h,"s","new");command(&h,"revision.adopt",first_adopt_payload("s","new")).await.unwrap();
    assert_eq!(usage_count(&h),1);assert_eq!(view(&h).state,SessionPhase::Ready);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    h.shutdown_ready().await.unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn abort_first_adoption_observer_does_not_lose_ready_active_view_or_once_usage() {
    let(h,path)=file_hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");h.fail("s",7,ApiError::new("rewrite_timeout","rewrite","fixed fixture failure"),true);
    source_only_candidate(&h,"s","adopt");let writer=second_writer(&path);let worker=h.clone();
    let observer=tokio::spawn(async move{command(&worker,"revision.adopt",first_adopt_payload("s","adopt")).await});
    wait_until(||h.db.outstanding()>0).await;observer.abort();let _=observer.await;writer.execute_batch("ROLLBACK;").unwrap();
    wait_until(||h.db_command_workers.load(Ordering::SeqCst)==0).await;
    assert_eq!(view(&h).state,SessionPhase::Ready);assert_eq!(usage_count(&h),1);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    assert_eq!(h.db.get_result("s").unwrap().source,source("s"));h.shutdown_ready().await.unwrap();
}

#[test]
fn native_capture_ack_never_holds_session_control_or_snapshot_lock() {
    let h=hub();let ready=h.clone();h.begin_shortcut_capture_with(&json!({"captureId":"independent"}),move||{
        assert!(ready.inner.try_lock().is_ok());assert!(ready.snapshot("main").is_ok());Ok(())
    }).unwrap();let ready=h.clone();h.cancel_shortcut_capture_with(&json!({"captureId":"independent"}),move||{
        assert!(ready.inner.try_lock().is_ok());assert!(ready.snapshot("main").is_ok());Ok(())
    }).unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn shutdown_waits_for_detached_database_owner_then_closes_actual_connection() {
    let(h,path)=file_hub();let writer=second_writer(&path);let worker=h.clone();
    let observer=tokio::spawn(async move{command(&worker,"preferences.update",json!({"expectedVersion":1,"patch":{"language":"en"},"requestId":"quit-owner"})).await});
    wait_until(||h.db.outstanding()>0).await;observer.abort();let _=observer.await;
    let worker=h.clone();let quitting=tokio::spawn(async move{worker.shutdown_ready().await});wait_until(||h.quitting.load(Ordering::SeqCst)).await;
    assert!(!quitting.is_finished());assert!(!h.db.owner_released());assert_eq!(h.start_session().await.unwrap_err().code,"system_unavailable");
    writer.execute_batch("ROLLBACK;").unwrap();tokio::time::timeout(Duration::from_secs(3),quitting).await.unwrap().unwrap().unwrap();
    assert_eq!(h.db.snapshot().preferences.language,"en");assert_eq!(h.db.outstanding(),0);assert!(h.db.owner_released());
    assert_eq!(h.db.submit(DbCommand::Preferences,None,DbPriority::Normal).err().unwrap().code(),"db_stopped");
    // A later maintenance ACK failure may retry shutdown after the connection
    // has actually closed. No new ClearTemporary job is sent to a dead owner.
    h.shutdown_ready().await.unwrap();assert!(h.db.owner_released());assert_eq!(h.db.outstanding(),0);
}

#[tokio::test]
async fn database_failure_compensates_native_preferences_without_holding_control() {
    let h=hub();let calls=Arc::new(Mutex::new(Vec::new()));let observed=calls.clone();let owner=Arc::downgrade(&h);
    // Controller seam for native ACK, with real SQLite constraint failure.
    // This does not register autostart or a global keyboard hook.
    h.set_native_actions(Arc::new(move|name,value|{let hub=owner.upgrade().unwrap();assert!(hub.inner.try_lock().is_ok());assert!(hub.snapshot("main").is_ok());assert_eq!(name,"autostart");observed.lock().unwrap().push(value);Ok(())}));
    fault(&h,"CREATE TRIGGER reject_preferences BEFORE UPDATE ON preferences BEGIN SELECT RAISE(ABORT,'fixed fault'); END;");
    assert!(command(&h,"preferences.update",json!({"expectedVersion":1,"patch":{"launchAtLogin":true},"requestId":"rollback-native"})).await.is_err());
    assert_eq!(*calls.lock().unwrap(),vec![true,false]);assert!(!h.db.snapshot().preferences.launch_at_login);assert!(h.inner.lock().unwrap().settings_pending.is_none());
    assert!(h.capture_available.load(Ordering::SeqCst));
}

#[test]
fn capture_cancel_revokes_its_preferences_commit_guard_before_native_off_wait() {
    let h=hub();h.begin_shortcut_capture_with(&json!({"captureId":"draft"}),||Ok(())).unwrap();
    let commit=h.inner.lock().unwrap().capture_commit_guard.as_ref().unwrap().operation();
    let mut next=h.db.snapshot().preferences;next.shortcut="shift_right".into();
    let io=h.shortcut_io.lock().unwrap();let owner=h.clone();let worker=std::thread::spawn(move||owner.cancel_shortcut_capture_with(&json!({"captureId":"draft"}),||Ok(())));
    let deadline=Instant::now()+Duration::from_secs(1);while !commit.is_cancelled(){assert!(Instant::now()<deadline);std::thread::yield_now();}
    // OFF has not received its ACK, but cancellation intent already prevents
    // a queued shortcut change from committing after focus/capture was lost.
    assert!(!worker.is_finished());let saved=h.db.submit(DbCommand::SavePreferences{next,expected_version:1},Some(commit),DbPriority::Normal).unwrap().wait_blocking();
    assert_eq!(saved.unwrap_err().code(),"cancelled");assert_eq!(h.db.snapshot().preferences.shortcut,"alt_right");drop(io);worker.join().unwrap().unwrap();
}

#[tokio::test]
async fn committed_first_adoption_receipt_after_delete_keeps_usage_without_reviving_active_body() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");source_only_candidate(&h,"s","late-receipt");
    let output=h.inner.lock().unwrap().candidate.as_ref().unwrap().result.clone().unwrap();
    let detail=db_detail(h.db_write(DbCommand::Adopt{session_id:"s".into(),expected_current_revision_id:String::new(),candidate:output.revision},h.record_guard("s",7).operation(),DbPriority::Normal).await.unwrap()).unwrap();
    // Delay only the engine's receipt application, using an actually committed
    // result from the production actor. A newer delete completes first.
    command(&h,"history.delete",json!({"sessionId":"s","requestId":"delete-before-receipt"})).await.unwrap();
    assert!(!h.sync_first_adoption("s",&detail));assert_eq!(view(&h).state,SessionPhase::Cancelled);assert!(!view(&h).can_copy_recovery);
    assert_eq!(usage_count(&h),1);assert!(h.db.get_result("s").is_err());
}

fn delivery_target_fixture()->platform::delivery::TargetSnapshot {
    platform::delivery::TargetSnapshot{hwnd:123,process_id:456,focused_hwnd:123,runtime_id:vec![1,2],selection:Some((0,0)),process_created_at:789}
}
fn delivery_anchor_fixture()->platform::delivery::ForegroundAnchor {
    let target=delivery_target_fixture();platform::delivery::ForegroundAnchor{hwnd:target.hwnd,process_id:target.process_id,
        process_created_at:target.process_created_at,focused_hwnd:target.focused_hwnd,selection:target.selection}
}
fn enable_fixture_delivery(h:&AppHub){let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.target=Some(delivery_target_fixture());s.prefs.delivery_mode=DeliveryMode::Direct;s.delivery_worker_released=false;}
fn edit_payload(id:&str,current:&str,text:&str)->Value{json!({"sessionId":id,"expectedCurrentRevisionId":current,"requestId":Uuid::new_v4().to_string(),"blocks":[{"id":"block-0","text":text,"sourceRefs":[format!("item-{id}-0"),format!("item-{id}-1")]}]})}
fn fake_delivery(h:Arc<AppHub>,copies:Arc<std::sync::atomic::AtomicUsize>,chords:Arc<std::sync::atomic::AtomicUsize>)->tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move||h.initial_delivery_with("s",7,|_|true,|permit,target,text,gate|platform::delivery::dispatch_with_test_operations(permit,target,text,gate,
        |_|{copies.fetch_add(1,Ordering::SeqCst);true},||{chords.fetch_add(1,Ordering::SeqCst);4})))
}
async fn wait_prepared(h:&AppHub){wait_until(||h.inner.lock().unwrap().active.as_ref().unwrap().delivery.as_ref().is_some_and(|d|d.state==DeliveryState::Prepared)).await;}
fn current_delivery(h:&AppHub)->DeliveryAttempt{let id=h.inner.lock().unwrap().active.as_ref().unwrap().delivery.as_ref().unwrap().id.clone();h.db.delivery_attempt(&id).unwrap()}

fn delivery_native_calls(h:&AppHub)->Arc<Mutex<Vec<(String,bool)>>> {
    let calls=Arc::new(Mutex::new(Vec::new()));let sink=calls.clone();
    h.set_native_actions(Arc::new(move|name,value|{sink.lock().unwrap().push((name.into(),value));Ok(())}));calls
}

#[test]
fn delivery_stop_freezes_latest_caret_once_and_cuts_audio_before_inspection(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
    let mut final_target=delivery_target_fixture();final_target.selection=Some((4,9));
    h.stop_with_capture(&identity("s"),||{
        let i=h.inner.try_lock().expect("native inspection must not hold session lock");let s=i.active.as_ref().unwrap();
        assert!(s.stop.load(Ordering::SeqCst));assert_eq!(s.phase,SessionPhase::FinalizingAsr);
        assert!(s.target_capture_pending);assert!(s.target.is_none());assert!(!s.view().worker_released);
        Some(final_target.clone())
    }).unwrap();
    h.stop_with_capture(&identity("s"),||panic!("duplicate stop must not freeze a different caret")).unwrap();
    let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();
    assert_eq!(s.target,Some(final_target));assert!(!s.target_capture_pending);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_ready_waits_for_stop_target_before_claim_and_dispatch(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
    let(id,generation)=h.begin_stop_target(&identity("s"),Instant::now()).unwrap().unwrap();
    let entered=Arc::new(AtomicBool::new(false));let entered2=entered.clone();
    let(release_tx,release_rx)=std::sync::mpsc::sync_channel(1);let stop_hub=h.clone();
    let capture=tokio::task::spawn_blocking(move||stop_hub.resolve_stop_target(&id,generation,||{
        entered2.store(true,Ordering::SeqCst);release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut target=delivery_target_fixture();target.selection=Some((8,8));Some(target)
    }));
    wait_until(||entered.load(Ordering::SeqCst)).await;ready(&h,"s");
    let copies=Arc::new(std::sync::atomic::AtomicUsize::new(0));let chords=Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let copies2=copies.clone();let chords2=chords.clone();let job_hub=h.clone();
    let delivery=tokio::task::spawn_blocking(move||job_hub.initial_delivery_with("s",7,
        |target|{assert_eq!(target.selection,Some((8,8)));true},
        |permit,target,text,gate|platform::delivery::dispatch_with_test_operations(permit,target,text,gate,
            |_|{copies2.fetch_add(1,Ordering::SeqCst);true},||{chords2.fetch_add(1,Ordering::SeqCst);4})));
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!delivery.is_finished());assert!(!view(&h).worker_released);assert_eq!(copies.load(Ordering::SeqCst),0);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    assert_eq!(h.start_session().await.unwrap_err().code,"session_busy");
    release_tx.send(()).unwrap();capture.await.unwrap();delivery.await.unwrap();
    assert_eq!(copies.load(Ordering::SeqCst),1);assert_eq!(chords.load(Ordering::SeqCst),1);
    assert!(view(&h).worker_released);assert_eq!(current_delivery(&h).state,DeliveryState::Unconfirmed);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM usage_events").unwrap(),1);
}

#[test]
fn delivery_missing_or_changed_target_records_skip_without_focus_steal_or_retry(){
    for changed in [false,true]{
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");
        {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.prefs.delivery_mode=DeliveryMode::Direct;s.delivery_worker_released=false;if changed{s.target=Some(delivery_target_fixture());}}
        let calls=delivery_native_calls(&h);
        h.initial_delivery_with("s",7,|_|false,|_,_,_,_|panic!("unverified target must not reach native dispatch"));
        let expected=if changed{"targetChangedBeforeDelivery"}else{"targetUnavailableAtStop"};
        assert_eq!(view(&h).state,SessionPhase::Ready);assert_eq!(view(&h).delivery_state,Some(DeliveryState::Skipped));
        assert_eq!(view(&h).delivery_failure_reason.as_deref(),Some(expected));assert_eq!(current_delivery(&h).reason.as_deref(),Some(expected));
        assert!(calls.lock().unwrap().contains(&("recording".into(),true)));
        assert!(!calls.lock().unwrap().iter().any(|(name,_)|name=="showMain"));
        h.initial_delivery_with("s",7,|_|panic!("terminal decision must not inspect again"),|_,_,_,_|panic!("terminal decision must not input again"));
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),1);
        assert_eq!(usage_count(&h),1);assert!(view(&h).can_copy_recovery);assert!(view(&h).worker_released);
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_stop_permission_failure_preserves_reason_and_once_only_skip(){
    for fail_anchor in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
        h.stop_session_at_with_target_result(&identity("s"),Instant::now(),
            ||if fail_anchor{Err(platform::PlatformError::TargetHigherIntegrity)}else{Ok(Some(delivery_anchor_fixture()))},
            move |_|{assert!(!fail_anchor,"anchor failure must not inspect another surface");Err(platform::PlatformError::TargetHigherIntegrity)}).await.unwrap();
        {let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert!(s.stop.load(Ordering::SeqCst));assert!(!s.target_capture_pending);assert!(s.target.is_none());assert_eq!(s.target_capture_failure_reason.as_deref(),Some("targetHigherIntegrity"));}
        ready(&h,"s");
        h.initial_delivery_with("s",7,|_|panic!("failed capture cannot validate a target"),|_,_,_,_|panic!("permission failure cannot mutate clipboard or inject input"));
        assert_eq!(current_delivery(&h).state,DeliveryState::Skipped);
        assert_eq!(current_delivery(&h).reason.as_deref(),Some("targetHigherIntegrity"));
        assert_eq!(view(&h).delivery_failure_reason.as_deref(),Some("targetHigherIntegrity"));
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM usage_events").unwrap(),1);assert!(view(&h).can_copy_recovery);
        h.initial_delivery_with("s",7,|_|panic!("terminal skip must not inspect again"),|_,_,_,_|panic!("terminal skip must not dispatch again"));
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),1);
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_typed_stop_failures_keep_reason_and_result_without_native_input(){
    use platform::{PlatformError, TargetCaptureFailure};
    for (failure,expected) in [
        (TargetCaptureFailure::StopAnchorUnavailable,"stopAnchorUnavailable"),
        (TargetCaptureFailure::StopFocusChanged,"stopFocusChanged"),
        (TargetCaptureFailure::TargetNotEditable,"targetNotEditable"),
        (TargetCaptureFailure::TargetCapabilityUnsupported,"targetCapabilityUnsupported"),
        (TargetCaptureFailure::TargetInspectionTimeout,"targetInspectionTimeout"),
    ] {
        for at_anchor in [true,false] {
            let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
            h.stop_session_at_with_target_result(&identity("s"),Instant::now(),
                ||if at_anchor{Err(PlatformError::TargetCapture(failure))}else{Ok(Some(delivery_anchor_fixture()))},
                move |_|{assert!(!at_anchor,"failed anchor must not inspect another target");Err(PlatformError::TargetCapture(failure))}).await.unwrap();
            {let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();
                assert!(s.stop.load(Ordering::SeqCst));assert!(!s.target_capture_pending);
                assert!(s.target.is_none());assert_eq!(s.target_capture_failure_reason.as_deref(),Some(expected));}
            ready(&h,"s");
            h.initial_delivery_with("s",7,|_|panic!("failed target must not validate"),|_,_,_,_|panic!("failed target must not copy or paste"));
            assert_eq!(current_delivery(&h).state,DeliveryState::Skipped);
            assert_eq!(current_delivery(&h).reason.as_deref(),Some(expected));
            assert_eq!(view(&h).delivery_failure_reason.as_deref(),Some(expected));
            assert_eq!(view(&h).state,SessionPhase::Ready);assert!(view(&h).can_copy_recovery);
            // stop_session records today's real ended_at, unlike the fixture's
            // original date. Count the once-only event across all dates.
            assert_eq!(h.db.query_count("SELECT COUNT(*) FROM usage_events").unwrap(),1);
            h.initial_delivery_with("s",7,|_|panic!("terminal skip must not inspect again"),|_,_,_,_|panic!("terminal skip must not paste again"));
            assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),1);
        }
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_stop_control_classifies_agreed_codes_without_parsing_private_messages(){
    for (code,expected) in [
        ("stop_target_changed","stopFocusChanged"),
        ("stop_target_unavailable","stopAnchorUnavailable"),
        ("stop_anchor_unavailable","stopAnchorUnavailable"),
        ("stop_focus_changed","stopFocusChanged"),
        ("target_not_editable","targetNotEditable"),
        ("target_capability_unsupported","targetCapabilityUnsupported"),
        ("window_timeout","targetInspectionTimeout"),
        ("target_inspection_timeout","targetInspectionTimeout"),
        ("unexpected_code","targetUnavailableAtStop"),
    ] {
        let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
        h.set_stop_target_control(Arc::new(move |_|Err(ApiError::new(code,"native","private title / body; stop_target_changed; target_higher_integrity"))));
        h.stop_session_at_with_target_result(&identity("s"),Instant::now(),||Ok(Some(delivery_anchor_fixture())),
            |_|panic!("failed stop coordinator must not inspect another target")).await.unwrap();
        ready(&h,"s");h.initial_delivery_with("s",7,|_|panic!("no target"),|_,_,_,_|panic!("no native input"));
        assert_eq!(current_delivery(&h).reason.as_deref(),Some(expected));
        let live=serde_json::to_string(&view(&h)).unwrap();
        assert!(!live.contains("private title / body"));
        assert_eq!(view(&h).state,SessionPhase::Ready);assert!(view(&h).can_copy_recovery);
    }
}

#[test]
fn typed_target_error_survives_desktop_api_boundary_without_provider_details(){
    use platform::{PlatformError,TargetCaptureFailure};
    for failure in [TargetCaptureFailure::StopAnchorUnavailable,TargetCaptureFailure::StopFocusChanged,
        TargetCaptureFailure::TargetNotEditable,TargetCaptureFailure::TargetCapabilityUnsupported,
        TargetCaptureFailure::TargetInspectionTimeout] {
        let api:ApiError=PlatformError::TargetCapture(failure).into();
        assert_eq!(api.code,failure.api_code());assert_eq!(api.stage,"native");
        assert!(matches!(stop_control_failure(api),PlatformError::TargetCapture(restored) if restored==failure));
    }
}

#[test]
fn late_stop_permission_error_cannot_pollute_a_new_generation(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
    let(id,generation)=h.begin_stop_target(&identity("s"),Instant::now()).unwrap().unwrap();
    h.resolve_stop_target_result(&id,generation,||{
        install(&h,"new",SessionPhase::Recording,true);
        Err(platform::PlatformError::TargetHigherIntegrity)
    });
    let i=h.inner.lock().unwrap();let current=i.active.as_ref().unwrap();
    assert_eq!(current.id,"new");assert!(current.target_capture_failure_reason.is_none());assert!(current.delivery_failure_reason.is_none());
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_stop_mode_return_preserves_permission_reason_without_target_inspection(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
    h.set_stop_target_control(Arc::new(|_|Err(platform::PlatformError::TargetHigherIntegrity.into())));
    h.stop_session_at_with_target_result(&identity("s"),Instant::now(),||Ok(Some(delivery_anchor_fixture())),
        |_|panic!("failed mode return must not inspect another input target")).await.unwrap();
    ready(&h,"s");h.initial_delivery_with("s",7,|_|panic!("unverified target cannot validate"),|_,_,_,_|panic!("unverified target cannot inject"));
    assert_eq!(current_delivery(&h).state,DeliveryState::Skipped);assert_eq!(current_delivery(&h).reason.as_deref(),Some("targetHigherIntegrity"));
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM usage_events").unwrap(),1);assert!(view(&h).can_copy_recovery);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn restore_at_scalar_caret_after_emoji_preserves_other_text_and_undo(){
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");
    let original_source=serde_json::to_value(h.db.get_result("s").unwrap().source).unwrap();
    let edited:HistoryDetail=serde_json::from_value(command(&h,"revision.edit",edit_payload("s","output-s","😀订机票前核对会议。")).await.unwrap()).unwrap();
    let edited_id=edited.output.unwrap().id;
    let restored:HistoryDetail=serde_json::from_value(command(&h,"revision.restore",json!({"sessionId":"s","expectedCurrentRevisionId":edited_id,"requestId":"caret-after-emoji","patch":{"blockId":"block-0","start":1,"end":1,"sourceSelections":[{"itemId":"item-s-0","start":8,"end":10}]}})).await.unwrap()).unwrap();
    let result=restored.output.unwrap();assert_eq!(result.blocks[0].text,"😀不要订机票前核对会议。");
    assert_eq!(serde_json::to_value(restored.source).unwrap(),original_source);assert!(restored.can_undo);
    let undone:HistoryDetail=serde_json::from_value(command(&h,"revision.undo",json!({"sessionId":"s","expectedCurrentRevisionId":result.id,"requestId":"caret-undo"})).await.unwrap()).unwrap();
    assert_eq!(undone.output.unwrap().blocks[0].text,"😀订机票前核对会议。");assert_eq!(serde_json::to_value(undone.source).unwrap(),original_source);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);assert_eq!(usage_count(&h),1);
}

#[test]
fn delivery_stop_target_failure_or_panicked_probe_clears_lease_and_old_caret(){
    for panic_probe in [false,true]{
        let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
        let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||h.stop_with_capture(&identity("s"),||{
            if panic_probe{panic!("synthetic inspection failure");}None
        })));
        assert_eq!(result.is_err(),panic_probe);
        assert!(h.inner.try_lock().is_ok());let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();
        assert!(s.target.is_none());assert!(!s.target_capture_pending);assert!(s.stop.load(Ordering::SeqCst));
    }
}

#[test]
fn delivery_late_stop_capture_cannot_repopulate_cancelled_or_replaced_session(){
    for replace in [false,true]{
        let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
        let(id,generation)=h.begin_stop_target(&identity("s"),Instant::now()).unwrap().unwrap();
        h.resolve_stop_target(&id,generation,||{
            if replace{install(&h,"new",SessionPhase::Recording,true);}else{h.cancel_session(&identity("s")).unwrap();}
            Some(delivery_target_fixture())
        });
        let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert!(s.target.is_none());assert!(!s.target_capture_pending);
        assert_eq!(s.id,if replace{"new"}else{"s"});assert_eq!(s.phase,if replace{SessionPhase::Recording}else{SessionPhase::Cancelled});
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_cancel_while_target_pending_ends_wait_without_input_or_ledger(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
    let(id,generation)=h.begin_stop_target(&identity("s"),Instant::now()).unwrap().unwrap();ready(&h,"s");
    let copies=Arc::new(std::sync::atomic::AtomicUsize::new(0));let chords=Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let delivery=fake_delivery(h.clone(),copies.clone(),chords.clone());tokio::time::sleep(Duration::from_millis(50)).await;
    h.cancel_session(&identity("s")).unwrap();tokio::time::timeout(Duration::from_secs(1),delivery).await.unwrap().unwrap();
    assert!(!view(&h).worker_released);assert_eq!(copies.load(Ordering::SeqCst),0);assert_eq!(chords.load(Ordering::SeqCst),0);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    h.resolve_stop_target(&id,generation,||Some(delivery_target_fixture()));
    assert!(view(&h).worker_released);assert!(h.inner.lock().unwrap().active.as_ref().unwrap().target.is_none());assert_eq!(h.db.query_count("SELECT COUNT(*) FROM usage_events").unwrap(),1);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_stop_cuts_audio_before_mode_close_and_failed_close_prevents_target_capture(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
    let called=Arc::new(AtomicBool::new(false));let observed=called.clone();let weak=Arc::downgrade(&h);
    h.set_stop_target_control(Arc::new(move|anchor|{
        let owner=weak.upgrade().unwrap();let i=owner.inner.try_lock().expect("UI callback must not hold session lock");let s=i.active.as_ref().unwrap();
        assert!(s.stop.load(Ordering::SeqCst));assert!(s.target_capture_pending);assert_eq!(anchor,delivery_anchor_fixture());
        observed.store(true,Ordering::SeqCst);Err(ApiError::new("mode_close_failed","native","synthetic UI failure"))
    }));
    h.stop_session_at_with_target(&identity("s"),Instant::now(),||Some(delivery_anchor_fixture()),|_|panic!("failed UI close must not inspect an external surface")).await.unwrap();assert!(called.load(Ordering::SeqCst));
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().target.is_none());assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().target_capture_pending);
    ready(&h,"s");h.initial_delivery_with("s",7,|_|panic!("failed menu close cannot yield a target"),|_,_,_,_|panic!("failed menu close cannot input"));
    assert_eq!(current_delivery(&h).reason.as_deref(),Some("targetUnavailableAtStop"));assert_eq!(h.db.query_count("SELECT COUNT(*) FROM usage_events").unwrap(),1);
}

#[test]
fn delivery_failed_ledger_keeps_result_and_never_reopens_automatic_input(){
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");enable_fixture_delivery(&h);
    let calls=delivery_native_calls(&h);
    fault(&h,"CREATE TRIGGER reject_delivery BEFORE INSERT ON delivery_attempts BEGIN SELECT RAISE(ABORT,'synthetic delivery storage fault'); END;");
    h.initial_delivery_with("s",7,|_|true,|_,_,_,_|panic!("input requires a committed ledger"));
    assert_eq!(view(&h).state,SessionPhase::Ready);assert_eq!(view(&h).delivery_state,Some(DeliveryState::Failed));
    assert_eq!(view(&h).delivery_failure_reason.as_deref(),Some("deliveryLedgerUnavailable"));
    assert!(calls.lock().unwrap().contains(&("recording".into(),true)));assert!(!calls.lock().unwrap().iter().any(|(name,_)|name=="showMain"));
    fault(&h,"DROP TRIGGER reject_delivery;");
    h.initial_delivery_with("s",7,|_|panic!("failed original attempt must not inspect again"),|_,_,_,_|panic!("failed original attempt must not retry input"));
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);assert_eq!(usage_count(&h),1);assert!(view(&h).can_copy_recovery);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_stop_anchor_and_verified_mode_return_are_resolved_before_accessibility_worker(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
    let order=Arc::new(Mutex::new(Vec::new()));let observed=order.clone();let weak=Arc::downgrade(&h);
    let original_anchor=delivery_anchor_fixture();let mut restored_anchor=original_anchor;restored_anchor.hwnd=321;restored_anchor.focused_hwnd=322;restored_anchor.selection=Some((5,5));
    h.set_stop_target_control(Arc::new(move|anchor|{
        let owner=weak.upgrade().unwrap();let i=owner.inner.try_lock().unwrap();assert!(i.active.as_ref().unwrap().stop.load(Ordering::SeqCst));
        assert_eq!(anchor,original_anchor);observed.lock().unwrap().push("mode-return");Ok(restored_anchor)
    }));
    let anchor_order=order.clone();let capture_order=order.clone();let check_hub=h.clone();
    h.stop_session_at_with_target(&identity("s"),Instant::now(),move||{
        assert!(check_hub.inner.try_lock().unwrap().active.as_ref().unwrap().stop.load(Ordering::SeqCst));
        anchor_order.lock().unwrap().push("stop-anchor");Some(original_anchor)
    },move|anchor|{
        assert_eq!(anchor,restored_anchor);capture_order.lock().unwrap().push("inspection");
        let mut target=delivery_target_fixture();target.hwnd=anchor.hwnd;target.focused_hwnd=anchor.focused_hwnd;target.selection=anchor.selection;Some(target)
    }).await.unwrap();
    assert_eq!(*order.lock().unwrap(),vec!["stop-anchor","mode-return","inspection"]);
    assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().target.as_ref().unwrap().hwnd,restored_anchor.hwnd);
    h.stop_session_at_with_target(&identity("s"),Instant::now(),||panic!("duplicate stop must not freeze another window"),|_|panic!("duplicate stop must not inspect again")).await.unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_stop_anchor_missing_or_focus_switched_during_ui_handoff_cannot_adopt_a_new_window(){
    for anchor_missing in [false,true]{
        let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
        h.set_stop_target_control(Arc::new(|_|Err(ApiError::new("stop_target_changed","native","synthetic focus switch"))));
        h.stop_session_at_with_target(&identity("s"),Instant::now(),||if anchor_missing{None}else{Some(delivery_anchor_fixture())},
            |_|panic!("no anchor or a changed foreground must not inspect the new window")).await.unwrap();
        assert!(h.inner.lock().unwrap().active.as_ref().unwrap().target.is_none());assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().target_capture_pending);
        ready(&h,"s");h.initial_delivery_with("s",7,|_|panic!("no target to validate"),|_,_,_,_|panic!("focus switch must not dispatch input"));
        assert_eq!(current_delivery(&h).state,DeliveryState::Skipped);
        assert_eq!(current_delivery(&h).reason.as_deref(),Some(if anchor_missing{"stopAnchorUnavailable"}else{"stopFocusChanged"}));
        assert_eq!(view(&h).delivery_failure_reason.as_deref(),Some(if anchor_missing{"stopAnchorUnavailable"}else{"stopFocusChanged"}));
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_aborted_stop_observer_and_cancel_do_not_release_a_running_target_worker(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
    let entered=Arc::new(AtomicBool::new(false));let entered2=entered.clone();let stop_hub=h.clone();
    let(release_tx,release_rx)=std::sync::mpsc::sync_channel(1);
    let observer=tokio::spawn(async move{stop_hub.stop_session_at_with_target(&identity("s"),Instant::now(),||Some(delivery_anchor_fixture()),move|_|{
        entered2.store(true,Ordering::SeqCst);release_rx.recv_timeout(Duration::from_secs(5)).unwrap();Some(delivery_target_fixture())
    }).await});
    wait_until(||entered.load(Ordering::SeqCst)).await;ready(&h,"s");observer.abort();assert!(observer.await.unwrap_err().is_cancelled());
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().target_capture_pending);assert!(!view(&h).worker_released);
    let copies=Arc::new(std::sync::atomic::AtomicUsize::new(0));let chords=Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let delivery=fake_delivery(h.clone(),copies.clone(),chords.clone());tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);assert!(!delivery.is_finished());
    h.cancel_session(&identity("s")).unwrap();tokio::time::timeout(Duration::from_secs(1),delivery).await.unwrap().unwrap();
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().target_capture_pending);assert!(!view(&h).worker_released);
    release_tx.send(()).unwrap();wait_until(||!h.inner.lock().unwrap().active.as_ref().unwrap().target_capture_pending).await;
    assert!(view(&h).worker_released);assert!(h.inner.lock().unwrap().active.as_ref().unwrap().target.is_none());
    assert_eq!(copies.load(Ordering::SeqCst),0);assert_eq!(chords.load(Ordering::SeqCst),0);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM usage_events").unwrap(),1);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delivery_stop_sync_anchor_or_ui_panic_releases_pre_worker_lease(){
    for panic_anchor in [false,true]{
        let h=hub();install(&h,"s",SessionPhase::Recording,true);enable_fixture_delivery(&h);
        if !panic_anchor{h.set_stop_target_control(Arc::new(|_|panic!("synthetic synchronous UI failure")));}
        let worker=h.clone();let job=tokio::spawn(async move{worker.stop_session_at_with_target(&identity("s"),Instant::now(),
            move||{if panic_anchor{panic!("synthetic anchor failure");}Some(delivery_anchor_fixture())},
            |_|panic!("pre-worker failure must never inspect a target")).await});
        assert!(job.await.unwrap_err().is_panic());assert!(h.inner.try_lock().is_ok());
        let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert!(!s.target_capture_pending);assert!(s.target.is_none());assert!(s.stop.load(Ordering::SeqCst));
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn prepared_delivery_is_revoked_by_adopt_edit_restore_and_undo_without_new_input(){
    for action in ["adopt","edit_restore_undo"]{
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");enable_fixture_delivery(&h);
        let copies=Arc::new(std::sync::atomic::AtomicUsize::new(0));let chords=Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let native=h.native_io.lock().unwrap();let delivery=fake_delivery(h.clone(),copies.clone(),chords.clone());wait_prepared(&h).await;
        if action=="adopt"{candidate(&h,"s","prepared-adopt");command(&h,"revision.adopt",json!({"sessionId":"s","expectedCurrentRevisionId":"output-s","candidateId":"prepared-adopt","requestId":"prepared-adopt-request"})).await.unwrap();}
        else{
            let edited:HistoryDetail=serde_json::from_value(command(&h,"revision.edit",edit_payload("s","output-s","会议未确认前不要订票，先保留行程。")).await.unwrap()).unwrap();let edited_id=edited.output.unwrap().id;
            let restored:HistoryDetail=serde_json::from_value(command(&h,"revision.restore",json!({"sessionId":"s","expectedCurrentRevisionId":edited_id,"requestId":"prepared-restore","patch":{"blockId":"block-0","start":0,"end":2,"sourceSelections":[{"itemId":"item-s-0","start":0,"end":2}]}})).await.unwrap()).unwrap();
            command(&h,"revision.undo",json!({"sessionId":"s","expectedCurrentRevisionId":restored.output.unwrap().id,"requestId":"prepared-undo"})).await.unwrap();
        }
        let actual=h.db.get_result("s").unwrap();assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().output.as_ref().unwrap().id,actual.output.unwrap().id);
        drop(native);delivery.await.unwrap();assert_eq!(copies.load(Ordering::SeqCst),0);assert_eq!(chords.load(Ordering::SeqCst),0);
        assert_eq!(current_delivery(&h).state,DeliveryState::Skipped);assert_eq!(usage_count(&h),1);assert_eq!(h.db.get_result("s").unwrap().source,source("s"));
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn version_intent_covers_a_prepared_commit_blocked_before_permit_installation(){
    let(h,path)=file_hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");enable_fixture_delivery(&h);
    let writer=second_writer(&path);let copies=Arc::new(std::sync::atomic::AtomicUsize::new(0));let chords=Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let delivery=fake_delivery(h.clone(),copies.clone(),chords.clone());wait_until(||h.db.outstanding()>0).await;
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().delivery_permit.is_none());
    let worker=h.clone();let edit=tokio::spawn(async move{command(&worker,"revision.edit",edit_payload("s","output-s","版本动作先撤销旧输入。")).await});
    wait_until(||h.inner.lock().unwrap().active.as_ref().unwrap().original_delivery_revoked).await;
    let before=Instant::now();assert!(h.snapshot("main").is_ok());assert!(h.inner.try_lock().is_ok());assert!(before.elapsed()<Duration::from_millis(100));
    writer.execute_batch("COMMIT;").unwrap();edit.await.unwrap().unwrap();delivery.await.unwrap();
    assert_eq!(copies.load(Ordering::SeqCst),0);assert_eq!(chords.load(Ordering::SeqCst),0);assert_eq!(current_delivery(&h).state,DeliveryState::Skipped);assert_eq!(usage_count(&h),1);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn failed_version_write_does_not_reopen_a_revoked_original_delivery(){
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");enable_fixture_delivery(&h);
    fault(&h,"CREATE TRIGGER reject_edit BEFORE INSERT ON outputs WHEN NEW.kind='edit' BEGIN SELECT RAISE(ABORT,'version failure'); END;");
    let copies=Arc::new(std::sync::atomic::AtomicUsize::new(0));let chords=Arc::new(std::sync::atomic::AtomicUsize::new(0));let native=h.native_io.lock().unwrap();
    let delivery=fake_delivery(h.clone(),copies.clone(),chords.clone());wait_prepared(&h).await;
    assert!(command(&h,"revision.edit",edit_payload("s","output-s","保存失败，不自动重开输入。")).await.is_err());
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().id,"output-s");drop(native);delivery.await.unwrap();
    fake_delivery(h.clone(),copies.clone(),chords.clone()).await.unwrap();
    assert_eq!(copies.load(Ordering::SeqCst),0);assert_eq!(chords.load(Ordering::SeqCst),0);assert_eq!(current_delivery(&h).state,DeliveryState::Skipped);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),1);assert_eq!(usage_count(&h),1);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn version_action_after_clipboard_write_skips_input_but_records_changed_clipboard(){
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");enable_fixture_delivery(&h);
    let copied=Arc::new(AtomicBool::new(false));let chords=Arc::new(std::sync::atomic::AtomicUsize::new(0));let(release_tx,release_rx)=std::sync::mpsc::sync_channel(1);let release=Mutex::new(release_rx);
    let worker=h.clone();let entered=copied.clone();let dispatched=chords.clone();
    let delivery=tokio::task::spawn_blocking(move||worker.initial_delivery_with("s",7,|_|true,|permit,target,text,gate|platform::delivery::dispatch_with_test_operations(permit,target,text,gate,
        |_|{entered.store(true,Ordering::SeqCst);release.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();true},||{dispatched.fetch_add(1,Ordering::SeqCst);4})));
    wait_until(||copied.load(Ordering::SeqCst)).await;
    tokio::time::timeout(Duration::from_secs(1),command(&h,"revision.edit",edit_payload("s","output-s","剪贴板之后、输入之前改版本。"))).await.unwrap().unwrap();
    release_tx.send(()).unwrap();delivery.await.unwrap();let receipt=current_delivery(&h);
    assert_eq!(receipt.state,DeliveryState::Skipped);assert_eq!(receipt.reason.as_deref(),Some("clipboardChangedInjectionSkipped"));assert_eq!(chords.load(Ordering::SeqCst),0);assert_eq!(usage_count(&h),1);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn already_started_native_input_keeps_actual_failure_partial_and_unconfirmed_receipts(){
    for count in [0,2,4]{
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");enable_fixture_delivery(&h);
        let injecting=Arc::new(AtomicBool::new(false));let(release_tx,release_rx)=std::sync::mpsc::sync_channel(1);let release=Mutex::new(release_rx);let worker=h.clone();let entered=injecting.clone();
        let delivery=tokio::task::spawn_blocking(move||worker.initial_delivery_with("s",7,|_|true,|permit,target,text,gate|platform::delivery::dispatch_with_test_operations(permit,target,text,gate,
            |_|true,||{entered.store(true,Ordering::SeqCst);release.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();count})));
        wait_until(||injecting.load(Ordering::SeqCst)).await;
        tokio::time::timeout(Duration::from_secs(1),command(&h,"revision.edit",edit_payload("s","output-s","输入已开始，不能承诺撤回。"))).await.unwrap().unwrap();
        release_tx.send(()).unwrap();delivery.await.unwrap();let receipt=current_delivery(&h);
        assert_eq!(receipt.state,if count==0{DeliveryState::Failed}else{DeliveryState::Unconfirmed});
        assert_eq!(receipt.reason.as_deref(),Some(if count==0{"clipboardChangedDispatchFailed"}else if count==4{"inputDispatchedUnconfirmed"}else{"partialInputUnconfirmed"}));assert_eq!(usage_count(&h),1);
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn editing_another_history_record_does_not_revoke_current_prepared_input(){
    let h=hub();install(&h,"other",SessionPhase::FinalizingAsr,true);ready(&h,"other");install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");enable_fixture_delivery(&h);
    let copies=Arc::new(std::sync::atomic::AtomicUsize::new(0));let chords=Arc::new(std::sync::atomic::AtomicUsize::new(0));let native=h.native_io.lock().unwrap();
    let delivery=fake_delivery(h.clone(),copies.clone(),chords.clone());wait_prepared(&h).await;
    command(&h,"revision.edit",edit_payload("other","output-other","只改另一个记录。")).await.unwrap();drop(native);delivery.await.unwrap();
    assert_eq!(copies.load(Ordering::SeqCst),1);assert_eq!(chords.load(Ordering::SeqCst),1);assert_eq!(current_delivery(&h).state,DeliveryState::Unconfirmed);
    assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().output.as_ref().unwrap().id,"output-s");assert_eq!(usage_count(&h),2);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn delayed_version_receipt_cannot_replace_a_newer_matching_active_version(){
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");let guard=h.history_guard("s").await.unwrap();
    let first=h.reserve_revision_intent("s");let detail1=db_detail(h.db_write(DbCommand::Edit{session_id:"s".into(),expected_current_revision_id:"output-s".into(),blocks:output(&h,"s").blocks,created_at:now()},guard.operation(),DbPriority::Normal).await.unwrap()).unwrap();
    let second=h.reserve_revision_intent("s");let mut blocks=detail1.output.as_ref().unwrap().blocks.clone();blocks[0].text="最新的版本".into();
    let detail2=db_detail(h.db_write(DbCommand::Edit{session_id:"s".into(),expected_current_revision_id:detail1.output.as_ref().unwrap().id.clone(),blocks,created_at:now()},guard.operation(),DbPriority::Normal).await.unwrap()).unwrap();
    h.sync_revision_receipt(second,&detail2);h.sync_revision_receipt(first,&detail1);
    assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().output.as_ref().unwrap().id,detail2.output.as_ref().unwrap().id);assert_eq!(usage_count(&h),1);
}

#[test]
fn stale_limit_or_explicit_stop_cannot_override_current_recording_intent() {
    let h=hub();install(&h,"new",SessionPhase::Recording,true);
    assert!(!h.recording_progress("old",7,600_000));
    assert!(!h.recording_progress("new",6,600_000));
    assert_eq!(view(&h).state,SessionPhase::Recording);
    assert_eq!(view(&h).recording_limit_remaining_ms,None);
    h.stop_with_capture(&identity("new"),||None).unwrap();
    assert!(!h.recording_progress("new",7,600_000));
    assert_eq!(view(&h).state,SessionPhase::FinalizingAsr);
    assert!(view(&h).error_code.is_none());
    let h=hub();install(&h,"cancel",SessionPhase::Recording,true);
    h.cancel_session(&identity("cancel")).unwrap();
    assert!(!h.recording_progress("cancel",7,600_000));
    assert_eq!(view(&h).state,SessionPhase::Cancelled);
}
