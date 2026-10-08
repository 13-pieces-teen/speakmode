//! Real actor/SQLite regressions. All files are UUID fixtures under core/harness;
//! these tests never open the desktop profile or call audio, cloud, or native input.
use super::*;
use super::commit_guard::{CancelDecision, CommitGuard};
use super::db_worker::{DbClient, DbCommand, DbOptions, DbPriority, DbTicket, DbValue};
use rusqlite::Connection;
use std::{fs, path::PathBuf, sync::Arc, time::Duration};
use tokio::time::{sleep, timeout};
use uuid::Uuid;

const DATE: &str = "2026-10-04";
const WAIT: Duration = Duration::from_secs(6);

struct DbPath(PathBuf);
impl DbPath {
    fn new() -> Self {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let base = if manifest.ends_with("harness") { manifest.join("test-data") }
            else { manifest.join("src/core/harness/test-data") };
        fs::create_dir_all(&base).unwrap();
        Self(base.join(format!("db-worker-{}.sqlite", Uuid::new_v4())))
    }
    fn connect(&self) -> Connection { Connection::open(&self.0).unwrap() }
}
impl Drop for DbPath {
    fn drop(&mut self) {
        // Only explicit files belonging to this UUID fixture; no recursive deletion.
        for path in [self.0.clone(), self.0.with_extension("sqlite-journal"),
                     self.0.with_extension("sqlite-wal"), self.0.with_extension("sqlite-shm")] {
            let _ = fs::remove_file(path);
        }
    }
}
struct Harness { db: DbClient, file: DbPath }
impl Harness {
    fn new(options: DbOptions) -> Self {
        let file = DbPath::new();
        let db = DbClient::open_with_options(&file.0, options).unwrap();
        Self { db, file }
    }
}
impl Drop for Harness {
    fn drop(&mut self) { self.db.request_shutdown(); }
}
fn options() -> DbOptions {
    DbOptions { normal_capacity: 1, critical_capacity: 1,
        byte_budget: 1024 * 1024, idle_refresh: Duration::from_secs(60) }
}
fn guard(id: &str) -> Arc<CommitGuard> { Arc::new(CommitGuard::new(id.into(), 7)) }
fn seed(id: &str, storage_policy: StoragePolicy) -> SessionSeed {
    SessionSeed { id: id.into(), generation: 7,
        started_at: "2026-10-04T23:59:45+08:00".into(),
        ended_at: "2026-10-04T23:59:57+08:00".into(), capture_duration_ms: 12_000,
        end_local_date: DATE.into(), time_zone: "Asia/Shanghai".into(), storage_policy,
        mode_snapshot: ModeSnapshot { mode_id: "auto".into(), name: "自动".into(),
            base_mode: BaseMode::Auto, rules: String::new(), revision: 1 } }
}
fn source(id: &str) -> SourceRevision {
    SourceRevision { id: format!("source-{id}"), session_id: id.into(),
        asr_attempt_id: format!("attempt-{id}"), generation: 7, provider: "aliyun".into(),
        model: "actor-test".into(), adapter_version: "test-v1".into(), complete: true,
        items: vec![
            SourceItem { id: format!("item-{id}-1"), provider_item_id: "provider-1".into(),
                capture_order: 1, text: "如果会议未确认，不要订机票。Hello 12 🙂".into(),
                is_final: true, start_ms: Some(0), end_ms: Some(7000) },
            SourceItem { id: format!("item-{id}-2"), provider_item_id: "provider-2".into(),
                capture_order: 2, text: "先保留行程。".into(), is_final: true,
                start_ms: Some(7000), end_ms: Some(12000) }],
        created_at: "2026-10-04T23:59:57+08:00".into() }
}
fn output(id: &str) -> OutputRevision {
    OutputRevision { id: format!("output-{id}"), session_id: id.into(),
        parent_revision_id: None, base_revision_id: None, source_revision_id: format!("source-{id}"),
        mode_snapshot: seed(id, StoragePolicy::History).mode_snapshot, resolved_mode: BaseMode::Light,
        blocks: vec![
            OutputBlock { id: "block-1".into(), text: "会议未确认前，不要订机票。Hello 12 🙂".into(),
                source_refs: vec![format!("item-{id}-1")] },
            OutputBlock { id: "block-2".into(), text: "先保留行程。".into(),
                source_refs: vec![format!("item-{id}-2")] }],
        kind: RevisionKind::Initial, creation_reason: CreationReason::NormalInitial,
        created_at: "2026-10-05T00:00:01+08:00".into() }
}
fn delivery(id: &str) -> DeliveryAttempt {
    DeliveryAttempt { id: format!("delivery-{id}"), session_id: id.into(), generation: 7,
        output_revision_id: format!("output-{id}"), target_fingerprint: "isolated-test-target".into(),
        claimed_at: "now".into(), state: DeliveryState::Prepared, reason: None }
}
fn as_detail(value: DbValue) -> HistoryDetail {
    match value { DbValue::Detail(detail) => detail, other => panic!("expected detail, got {other:?}") }
}
async fn wait(ticket: &DbTicket) -> StoreResult<DbValue> {
    timeout(WAIT, ticket.wait()).await.expect("DB ticket did not reach a terminal result")
}
async fn invoke(db: &DbClient, command: DbCommand, gate: Option<&Arc<CommitGuard>>) -> StoreResult<DbValue> {
    wait(&db.submit(command, gate.cloned(), DbPriority::Normal)?).await
}
async fn until(mut predicate: impl FnMut() -> bool) {
    timeout(Duration::from_millis(1500), async {
        while !predicate() { sleep(Duration::from_millis(1)).await; }
    }).await.expect("actor did not reach the expected observable phase");
}
async fn checkpoint(db: &DbClient, id: &str, policy: StoragePolicy, gate: &Arc<CommitGuard>) -> HistoryDetail {
    as_detail(invoke(db, DbCommand::Checkpoint { seed: seed(id, policy), source: source(id) }, Some(gate)).await.unwrap())
}
async fn ready(db: &DbClient, id: &str, policy: StoragePolicy, gate: &Arc<CommitGuard>) -> HistoryDetail {
    checkpoint(db, id, policy, gate).await;
    as_detail(invoke(db, DbCommand::Ready { output: output(id) }, Some(gate)).await.unwrap())
}
async fn usage(db: &DbClient) -> DailyUsage {
    match invoke(db, DbCommand::UsageToday { date: DATE.into() }, None).await.unwrap() {
        DbValue::Usage(value) => value, other => panic!("expected usage, got {other:?}") }
}
async fn stop(db: &DbClient) {
    let ticket = db.submit(DbCommand::Shutdown, None, DbPriority::Critical).unwrap();
    assert!(matches!(wait(&ticket).await.unwrap(), DbValue::Unit));
    assert!(db.owner_released(), "shutdown receipt must follow Store drop");
    assert_eq!(db.outstanding(), 0);
    assert_eq!(db.queued_bytes(), 0);
}
fn count(connection: &Connection, table: &str) -> u64 {
    connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap()
}
fn write_lock(file: &DbPath) -> Connection {
    let connection = file.connect();
    connection.execute_batch("BEGIN IMMEDIATE;").unwrap();
    connection
}
fn read_lock(file: &DbPath) -> Connection {
    let connection = file.connect();
    let journal: String = connection.pragma_query_value(None, "journal_mode", |r| r.get(0)).unwrap();
    assert_eq!(journal.to_ascii_lowercase(), "delete", "COMMIT-lock fixture requires rollback journal");
    connection.execute_batch("BEGIN;").unwrap();
    let _ = count(&connection, "preferences");
    connection
}
// With normal_capacity=1, the second accepted normal request proves the first
// has left the queue. The first performs real SQL under a second-connection lock.
async fn queue_behind_active(db: &DbClient) -> DbTicket {
    timeout(Duration::from_millis(1500), async {
        loop {
            match db.submit(DbCommand::Preferences, None, DbPriority::Normal) {
                Ok(ticket) => return ticket,
                Err(StoreError::Busy) => sleep(Duration::from_millis(1)).await,
                Err(error) => panic!("unexpected admission error: {error:?}"),
            }
        }
    }).await.expect("worker never dequeued the blocked write")
}
async fn cancel_promptly(gate: Arc<CommitGuard>) -> CancelDecision {
    // This bounds the control decision only, never final SQLite cleanup latency.
    timeout(Duration::from_millis(500), tokio::task::spawn_blocking(move || gate.cancel()))
        .await.expect("cancel waited for the SQLite finalizer").unwrap()
}

#[tokio::test]
async fn bootstrap_rejects_invalid_bounds_preserves_failed_migrations_and_recovers_without_replay() {
    for index in 0..4 {
        let mut opts = options();
        match index { 0 => opts.normal_capacity = 0, 1 => opts.critical_capacity = 0,
            2 => opts.byte_budget = 0, _ => opts.idle_refresh = Duration::ZERO }
        assert!(matches!(DbClient::in_memory_with_options(opts), Err(StoreError::Invalid(_))));
    }
    for future_schema in [false, true] {
        let file = DbPath::new();
        let connection = file.connect();
        connection.execute_batch("CREATE TABLE retained_marker(value TEXT); INSERT INTO retained_marker VALUES('keep me');").unwrap();
        if future_schema { connection.pragma_update(None, "user_version", 999).unwrap(); }
        else { connection.execute_batch("CREATE TABLE preferences(deliberate_schema_conflict TEXT);").unwrap(); }
        drop(connection);
        assert!(DbClient::open(&file.0).is_err());
        let connection = file.connect();
        assert_eq!(count(&connection, "retained_marker"), 1);
        assert_eq!(connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), if future_schema { 999 } else { 0 });
        assert_eq!(connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='sources'", [], |r| r.get::<_, u64>(0)).unwrap(), 0);
    }
    let file = DbPath::new();
    {
        let mut store = Store::open(&file.0).unwrap();
        store.checkpoint_source(seed("waiting", StoragePolicy::History), source("waiting")).unwrap();
        store.checkpoint_source(seed("sent", StoragePolicy::History), source("sent")).unwrap();
        store.commit_ready(output("sent")).unwrap();
        store.prepare_delivery(delivery("sent")).unwrap();
        store.transition_delivery("delivery-sent", DeliveryState::Prepared, DeliveryState::Dispatched, None).unwrap();
    }
    let db = DbClient::open(&file.0).unwrap();
    let waiting = as_detail(invoke(&db, DbCommand::GetResult { session_id: "waiting".into() }, None).await.unwrap());
    assert_eq!(waiting.session.status, StoredSessionStatus::Interrupted);
    assert!(waiting.output.is_none());
    match invoke(&db, DbCommand::DeliveryAttempt { attempt_id: "delivery-sent".into() }, None).await.unwrap() {
        DbValue::Delivery(value) => assert_eq!(value.state, DeliveryState::Unconfirmed),
        other => panic!("unexpected reply: {other:?}"),
    }
    assert_eq!(usage(&db).await.count, 1);
    stop(&db).await;
}

#[tokio::test]
async fn committed_write_keeps_its_receipt_and_last_cache_when_refresh_fails_then_recovers() {
    const HISTORICAL_DATE: &str = "2000-01-01";
    let h = Harness::new(options()); let gate = guard("cache-refresh");
    let mut metadata = seed("cache-refresh", StoragePolicy::History);
    metadata.end_local_date = HISTORICAL_DATE.into();
    invoke(&h.db, DbCommand::Checkpoint { seed: metadata, source: source("cache-refresh") }, Some(&gate)).await.unwrap();
    invoke(&h.db, DbCommand::Ready { output: output("cache-refresh") }, Some(&gate)).await.unwrap();
    let before = match invoke(&h.db, DbCommand::ReadSnapshot { date: HISTORICAL_DATE.into() }, None).await.unwrap() {
        DbValue::Snapshot(value) => value, other => panic!("{other:?}"),
    };
    assert_eq!(before.usage.date, HISTORICAL_DATE); assert_eq!(before.usage.count, 1);
    // The DDL succeeds, but the subsequent snapshot SELECT cannot find preferences.
    // Never relabel this committed command as failed or publish fabricated zero Usage.
    let receipt = invoke(&h.db, DbCommand::TestExecuteBatch {
        sql: "ALTER TABLE preferences RENAME TO saved_preferences;".into() }, None).await.unwrap();
    assert!(matches!(receipt, DbValue::Unit)); assert!(h.db.cache_error().is_some());
    let retained = h.db.snapshot();
    assert_eq!(retained.revision, before.revision); assert_eq!(retained.usage, before.usage);
    assert_eq!(retained.preferences, before.preferences);
    let verify = h.file.connect(); assert_eq!(count(&verify, "saved_preferences"), 1); drop(verify);
    assert!(invoke(&h.db, DbCommand::ReadSnapshot { date: HISTORICAL_DATE.into() }, None).await.is_err());
    assert_eq!(h.db.snapshot().usage, before.usage);
    assert!(matches!(invoke(&h.db, DbCommand::TestExecuteBatch {
        sql: "ALTER TABLE saved_preferences RENAME TO preferences;".into() }, None).await.unwrap(), DbValue::Unit));
    assert!(h.db.cache_error().is_none()); assert!(h.db.snapshot().revision > before.revision);
    stop(&h.db).await;

    // Request an existing historical day, then let the real idle refresh switch back
    // to today's date. This exercises rollover without changing the machine clock.
    let mut opts = options(); opts.idle_refresh = Duration::from_millis(20);
    let db = DbClient::open_with_options(&h.file.0, opts).unwrap();
    let historical = match invoke(&db, DbCommand::ReadSnapshot { date: HISTORICAL_DATE.into() }, None).await.unwrap() {
        DbValue::Snapshot(value) => value, other => panic!("{other:?}"),
    };
    assert_eq!(historical.usage, before.usage);
    until(|| db.snapshot().usage.date == chrono::Local::now().format("%Y-%m-%d").to_string()).await;
    assert_eq!(db.snapshot().usage.count, 0); assert!(db.cache_error().is_none());
    stop(&db).await;
}

#[tokio::test]
async fn external_writer_blocks_ready_but_not_cancel_intent_or_cached_snapshot() {
    let h = Harness::new(options());
    let gate = guard("cancel-before-ready");
    checkpoint(&h.db, "cancel-before-ready", StoragePolicy::History, &gate).await;
    let lock = write_lock(&h.file);
    let pending = h.db.submit(DbCommand::Ready { output: output("cancel-before-ready") }, Some(gate.clone()), DbPriority::Normal).unwrap();
    let queued = queue_behind_active(&h.db).await;
    assert!(!pending.is_complete());
    assert_eq!(cancel_promptly(gate.clone()).await, CancelDecision::Accepted);
    let cache = h.db.snapshot();
    assert_eq!(cache.preferences.version, 1);
    assert!(!pending.is_complete(), "control/cache access must not wait for the held SQL lock");
    let cleanup = h.db.submit(DbCommand::CancelUnfinished { session_id: "cancel-before-ready".into() }, Some(gate.clone()), DbPriority::Critical).unwrap();
    lock.execute_batch("ROLLBACK;").unwrap();
    assert!(matches!(wait(&pending).await, Err(StoreError::Cancelled)));
    wait(&cleanup).await.unwrap(); wait(&queued).await.unwrap();
    assert_eq!(usage(&h.db).await.count, 0);
    let verify = h.file.connect();
    for table in ["sessions", "sources", "source_items", "outputs", "usage_events"] { assert_eq!(count(&verify, table), 0, "{table}"); }
    assert_eq!(count(&verify, "content_tombstones"), 1);
    drop(verify); drop(lock); stop(&h.db).await;
}

#[tokio::test]
async fn completed_checkpoint_is_still_cancellable_and_late_ready_cannot_revive_it() {
    let h = Harness::new(options()); let gate = guard("checkpoint-only");
    let saved = checkpoint(&h.db, "checkpoint-only", StoragePolicy::History, &gate).await;
    assert!(saved.output.is_none()); assert!(!gate.ready_committed());
    assert_eq!(gate.cancel(), CancelDecision::Accepted);
    invoke(&h.db, DbCommand::CancelUnfinished { session_id: "checkpoint-only".into() }, Some(&gate)).await.unwrap();
    assert!(matches!(invoke(&h.db, DbCommand::Ready { output: output("checkpoint-only") }, Some(&gate)).await, Err(StoreError::Cancelled)));
    assert!(matches!(invoke(&h.db, DbCommand::Checkpoint { seed: seed("checkpoint-only", StoragePolicy::History), source: source("checkpoint-only") }, Some(&guard("checkpoint-only"))).await, Err(StoreError::Cancelled)));
    assert_eq!(usage(&h.db).await.count, 0); stop(&h.db).await;
}

#[tokio::test]
async fn ready_winner_keeps_once_usage_and_cancel_cannot_claim_content_was_deleted() {
    let h = Harness::new(options()); let gate = guard("ready-first");
    let first = ready(&h.db, "ready-first", StoragePolicy::History, &gate).await;
    invoke(&h.db, DbCommand::Ready { output: output("ready-first") }, Some(&gate)).await.unwrap();
    assert!(gate.ready_committed()); assert_eq!(gate.cancel(), CancelDecision::AlreadyCompleted);
    assert!(matches!(invoke(&h.db, DbCommand::CancelUnfinished { session_id: "ready-first".into() }, Some(&gate)).await, Err(StoreError::AlreadyCompleted)));
    assert_eq!(as_detail(invoke(&h.db, DbCommand::GetResult { session_id: "ready-first".into() }, None).await.unwrap()), first);
    assert_eq!(usage(&h.db).await.count, 1); stop(&h.db).await;
}

#[tokio::test]
async fn real_commit_lock_distinguishes_checkpoint_from_ready_when_cancel_is_pending() {
    for is_ready in [false, true] {
        let h = Harness::new(options()); let gate = guard("finalizing");
        if is_ready { checkpoint(&h.db, "finalizing", StoragePolicy::History, &gate).await; }
        let lock = read_lock(&h.file);
        let command = if is_ready { DbCommand::Ready { output: output("finalizing") } }
            else { DbCommand::Checkpoint { seed: seed("finalizing", StoragePolicy::History), source: source("finalizing") } };
        let pending = h.db.submit(command, Some(gate.clone()), DbPriority::Normal).unwrap();
        until(|| gate.finalizing()).await;
        assert!(!pending.is_complete());
        assert_eq!(cancel_promptly(gate.clone()).await, CancelDecision::Pending);
        assert!(gate.pending()); assert!(!gate.is_cancelled());
        lock.execute_batch("ROLLBACK;").unwrap();
        wait(&pending).await.unwrap();
        if is_ready {
            assert!(gate.ready_committed()); assert_eq!(gate.cancel(), CancelDecision::AlreadyCompleted);
            assert_eq!(usage(&h.db).await.count, 1);
        } else {
            assert!(!gate.ready_committed()); assert!(gate.is_cancelled());
            assert_eq!(gate.cancel(), CancelDecision::Accepted);
            invoke(&h.db, DbCommand::CancelUnfinished { session_id: "finalizing".into() }, Some(&gate)).await.unwrap();
            assert_eq!(usage(&h.db).await.count, 0);
        }
        drop(lock); stop(&h.db).await;
    }
}

#[tokio::test]
async fn full_normal_and_critical_queues_leave_one_coalesced_cleanup_lane() {
    let h = Harness::new(options()); let gate = guard("reserved-cleanup");
    checkpoint(&h.db, "reserved-cleanup", StoragePolicy::History, &gate).await;
    let lock = write_lock(&h.file);
    let pending = h.db.submit(DbCommand::Ready { output: output("reserved-cleanup") }, Some(gate.clone()), DbPriority::Normal).unwrap();
    let normal = queue_behind_active(&h.db).await;
    assert!(matches!(h.db.submit(DbCommand::Preferences, None, DbPriority::Normal), Err(StoreError::Busy)));
    let critical = h.db.submit(DbCommand::Preferences, None, DbPriority::Critical).unwrap();
    assert!(matches!(h.db.submit(DbCommand::Preferences, None, DbPriority::Critical), Err(StoreError::Busy)));
    assert_eq!(gate.cancel(), CancelDecision::Accepted);
    let cleanup = h.db.submit(DbCommand::CancelUnfinished { session_id: "reserved-cleanup".into() }, Some(gate.clone()), DbPriority::Critical).unwrap();
    let outstanding = h.db.outstanding();
    let duplicate = h.db.submit(DbCommand::CancelUnfinished { session_id: "reserved-cleanup".into() }, Some(gate.clone()), DbPriority::Normal).unwrap();
    assert_eq!(h.db.outstanding(), outstanding, "duplicate cleanup must share the existing receipt");
    assert!(matches!(h.db.submit(DbCommand::CancelUnfinished { session_id: "different-session".into() }, Some(guard("different-session")), DbPriority::Critical), Err(StoreError::Busy)));
    lock.execute_batch("ROLLBACK;").unwrap();
    assert!(matches!(wait(&pending).await, Err(StoreError::Cancelled)));
    for ticket in [cleanup, duplicate, critical, normal] { wait(&ticket).await.unwrap(); }
    until(|| h.db.outstanding() == 0 && h.db.queued_bytes() == 0).await;
    drop(lock); stop(&h.db).await;
}

#[tokio::test]
async fn byte_budget_survives_aborted_observers_until_the_real_write_finishes() {
    let file = DbPath::new();
    { let mut store = Store::open(&file.0).unwrap(); store.checkpoint_source(seed("orphan", StoragePolicy::History), source("orphan")).unwrap(); }
    let command = DbCommand::Ready { output: output("orphan") };
    let bytes = serde_json::to_vec(&command).unwrap().len();
    let mut opts = options(); opts.byte_budget = bytes;
    let h = Harness { db: DbClient::open_with_options(&file.0, opts).unwrap(), file };
    let gate = guard("orphan"); let lock = write_lock(&h.file);
    let ticket = h.db.submit(command, Some(gate.clone()), DbPriority::Normal).unwrap();
    let observer = ticket.clone();
    let (begun, started) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(async move { let _ = begun.send(()); observer.wait().await });
    started.await.unwrap(); waiter.abort(); let _ = waiter.await; drop(ticket);
    assert_eq!(h.db.queued_bytes(), bytes); assert_eq!(h.db.outstanding(), 1);
    assert!(matches!(h.db.submit(DbCommand::Preferences, None, DbPriority::Critical), Err(StoreError::Busy)));
    assert!(matches!(h.db.submit(DbCommand::HistoryList { query: "x".repeat(bytes + 1), cursor: 0, limit: 20 }, None, DbPriority::Normal), Err(StoreError::Busy)));
    lock.execute_batch("ROLLBACK;").unwrap();
    until(|| h.db.outstanding() == 0 && h.db.queued_bytes() == 0).await;
    assert!(gate.ready_committed());
    assert!(as_detail(invoke(&h.db, DbCommand::GetResult { session_id: "orphan".into() }, None).await.unwrap()).output.is_some());
    assert_eq!(usage(&h.db).await.count, 1);
    let failed = h.db.submit(DbCommand::GetResult { session_id: "absent".into() }, None, DbPriority::Normal).unwrap();
    assert!(matches!(wait(&failed).await, Err(StoreError::NotFound)));
    until(|| h.db.queued_bytes() == 0).await;
    drop(lock); stop(&h.db).await;
}

#[tokio::test]
async fn shutdown_closes_admission_drains_accepted_work_and_acknowledges_owner_release() {
    let h = Harness::new(options()); let gate = guard("shutdown");
    checkpoint(&h.db, "shutdown", StoragePolicy::History, &gate).await;
    let lock = write_lock(&h.file);
    let pending = h.db.submit(DbCommand::Ready { output: output("shutdown") }, Some(gate), DbPriority::Normal).unwrap();
    let queued = queue_behind_active(&h.db).await;
    let shutdown = h.db.submit(DbCommand::Shutdown, None, DbPriority::Critical).unwrap();
    assert!(matches!(h.db.submit(DbCommand::Preferences, None, DbPriority::Normal), Err(StoreError::WorkerStopped)));
    assert!(!shutdown.is_complete()); assert!(!h.db.owner_released());
    assert!(h.db.outstanding() >= 2);
    lock.execute_batch("ROLLBACK;").unwrap(); drop(lock);
    wait(&pending).await.unwrap(); wait(&queued).await.unwrap();
    let observer = shutdown.clone();
    tokio::task::spawn_blocking(move || observer.wait_blocking()).await.unwrap().unwrap();
    wait(&shutdown).await.unwrap();
    assert!(h.db.owner_released()); assert_eq!(h.db.outstanding(), 0); assert_eq!(h.db.queued_bytes(), 0);
    let reopened = Store::open(&h.file.0).unwrap();
    assert!(reopened.get_result("shutdown").unwrap().output.is_some());
    assert_eq!(reopened.usage_today(DATE).unwrap().count, 1);
    drop(reopened);
    fs::remove_file(&h.file.0).expect("owner ACK must release the actual SQLite file handle");
}

#[tokio::test]
async fn ready_sql_failure_rolls_back_output_and_usage_then_retry_commits_once() {
    let h = Harness::new(options()); let gate = guard("failed-ready");
    checkpoint(&h.db, "failed-ready", StoragePolicy::History, &gate).await;
    invoke(&h.db, DbCommand::TestExecuteBatch { sql: "CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'worker fault'); END;".into() }, None).await.unwrap();
    assert!(invoke(&h.db, DbCommand::Ready { output: output("failed-ready") }, Some(&gate)).await.is_err());
    assert!(!gate.ready_committed()); assert!(!gate.finalizing());
    let preserved = as_detail(invoke(&h.db, DbCommand::GetResult { session_id: "failed-ready".into() }, None).await.unwrap());
    assert_eq!(preserved.source, source("failed-ready")); assert!(preserved.output.is_none());
    let verify = h.file.connect(); assert_eq!(count(&verify, "outputs"), 0); assert_eq!(count(&verify, "usage_events"), 0); drop(verify);
    invoke(&h.db, DbCommand::TestExecuteBatch { sql: "DROP TRIGGER fail_usage;".into() }, None).await.unwrap();
    let mut retry = output("failed-ready"); retry.creation_reason = CreationReason::Retry;
    invoke(&h.db, DbCommand::Ready { output: retry }, Some(&gate)).await.unwrap();
    assert_eq!(usage(&h.db).await.count, 1);
    assert!(invoke(&h.db, DbCommand::PrepareDelivery { attempt: delivery("failed-ready") }, Some(&gate)).await.is_err());
    stop(&h.db).await;
}

#[tokio::test]
async fn memory_only_keeps_all_body_tables_empty_but_usage_survives_clear_and_restart() {
    let h = Harness::new(options()); let gate = guard("private");
    ready(&h.db, "private", StoragePolicy::MemoryOnly, &gate).await;
    invoke(&h.db, DbCommand::Ready { output: output("private") }, Some(&gate)).await.unwrap();
    let verify = h.file.connect();
    for table in ["sessions", "sources", "source_items", "outputs"] { assert_eq!(count(&verify, table), 0, "{table}"); }
    assert_eq!(count(&verify, "usage_events"), 1); drop(verify);
    let totals = usage(&h.db).await;
    assert_eq!(totals.count, 1); assert_eq!(totals.capture_duration_ms, 12_000);
    assert_eq!(totals.source_char_count, super::store::source_character_count(&source("private").text()));
    match invoke(&h.db, DbCommand::HistoryList { query: String::new(), cursor: 0, limit: 20 }, None).await.unwrap() {
        DbValue::HistoryPage(page) => assert_eq!(page.total, 0), other => panic!("{other:?}"),
    }
    match invoke(&h.db, DbCommand::ReadSnapshot { date: DATE.into() }, None).await.unwrap() {
        DbValue::Snapshot(snapshot) => assert_eq!(snapshot.temporary_result.unwrap().source, source("private")), other => panic!("{other:?}"),
    }
    invoke(&h.db, DbCommand::ClearTemporary, None).await.unwrap();
    assert!(matches!(invoke(&h.db, DbCommand::GetResult { session_id: "private".into() }, None).await, Err(StoreError::NotFound)));
    assert_eq!(usage(&h.db).await, totals);
    let cancelled = guard("cancelled-private"); assert_eq!(cancelled.cancel(), CancelDecision::Accepted);
    assert!(matches!(invoke(&h.db, DbCommand::Checkpoint { seed: seed("cancelled-private", StoragePolicy::MemoryOnly), source: source("cancelled-private") }, Some(&cancelled)).await, Err(StoreError::Cancelled)));
    stop(&h.db).await;
    let reopened = Store::open(&h.file.0).unwrap();
    assert!(reopened.latest_temporary_result().unwrap().is_none());
    assert_eq!(reopened.usage_today(DATE).unwrap(), totals);
}

#[tokio::test]
async fn actor_unicode_restore_cas_and_one_level_undo_preserve_source_and_other_blocks() {
    let h = Harness::new(options()); let gate = guard("unicode");
    let initial = ready(&h.db, "unicode", StoragePolicy::History, &gate).await.output.unwrap();
    let mut blocks = initial.blocks.clone(); blocks[0].text = "🙂可以订机票，Hello。".into();
    let edited = as_detail(invoke(&h.db, DbCommand::Edit { session_id: "unicode".into(), expected_current_revision_id: initial.id.clone(), blocks, created_at: "edit".into() }, Some(&gate.operation())).await.unwrap()).output.unwrap();
    let start = source("unicode").items[0].text.chars().position(|c| c == '不').unwrap();
    let patch = RestorePatch { block_id: "block-1".into(), start: 1, end: 6,
        source_selections: vec![SourceSelection { item_id: "item-unicode-1".into(), start, end: start + 5 }] };
    let restored = as_detail(invoke(&h.db, DbCommand::Restore { session_id: "unicode".into(), expected_current_revision_id: edited.id.clone(), patch: patch.clone(), created_at: "restore".into() }, Some(&gate.operation())).await.unwrap());
    let restored_output = restored.output.unwrap();
    assert_eq!(restored_output.blocks[0].text, "🙂不要订机票，Hello。");
    assert_eq!(restored_output.blocks[1], initial.blocks[1]); assert_eq!(restored.source, source("unicode"));
    assert!(matches!(invoke(&h.db, DbCommand::Restore { session_id: "unicode".into(), expected_current_revision_id: edited.id, patch: patch.clone(), created_at: "stale".into() }, Some(&gate.operation())).await, Err(StoreError::Conflict { .. })));
    let mut invalid = patch; invalid.end = 999;
    assert!(invoke(&h.db, DbCommand::Restore { session_id: "unicode".into(), expected_current_revision_id: restored_output.id.clone(), patch: invalid, created_at: "invalid".into() }, Some(&gate.operation())).await.is_err());
    let undone = as_detail(invoke(&h.db, DbCommand::Undo { session_id: "unicode".into(), expected_current_revision_id: restored_output.id, created_at: "undo".into() }, Some(&gate.operation())).await.unwrap());
    assert_eq!(undone.output.as_ref().unwrap().blocks, edited.blocks); assert!(!undone.can_undo);
    assert!(matches!(invoke(&h.db, DbCommand::Undo { session_id: "unicode".into(), expected_current_revision_id: undone.output.unwrap().id, created_at: "twice".into() }, Some(&gate.operation())).await, Err(StoreError::UndoUnavailable)));
    assert_eq!(usage(&h.db).await.count, 1); stop(&h.db).await;
}

#[tokio::test]
async fn cancelled_history_operation_does_not_poison_the_ready_record_or_a_sibling_operation() {
    let h = Harness::new(options()); let parent = guard("history-operation");
    let initial = ready(&h.db, "history-operation", StoragePolicy::History, &parent).await.output.unwrap();
    let child = parent.operation();
    assert_eq!(child.cancel(), CancelDecision::Accepted);
    let mut candidate = initial.clone(); candidate.id = "discarded-candidate".into(); candidate.base_revision_id = Some(initial.id.clone());
    assert!(matches!(invoke(&h.db, DbCommand::Adopt { session_id: "history-operation".into(), expected_current_revision_id: initial.id.clone(), candidate }, Some(&child)).await, Err(StoreError::Cancelled)));
    assert!(parent.ready_committed()); assert!(!parent.is_cancelled());
    let current = as_detail(invoke(&h.db, DbCommand::GetResult { session_id: "history-operation".into() }, None).await.unwrap());
    assert_eq!(current.output.unwrap(), initial); assert_eq!(current.source, source("history-operation"));
    let mut blocks = initial.blocks; blocks[0].text = "另一项明确的用户修改".into();
    invoke(&h.db, DbCommand::Edit { session_id: "history-operation".into(), expected_current_revision_id: initial.id, blocks, created_at: "later".into() }, Some(&parent.operation())).await.unwrap();
    assert_eq!(usage(&h.db).await.count, 1); stop(&h.db).await;
}

#[tokio::test]
async fn source_only_child_adoption_marks_parent_ready_without_losing_frozen_mode_or_once_usage() {
    let h = Harness::new(options()); let parent = guard("first-adopt");
    checkpoint(&h.db, "first-adopt", StoragePolicy::History, &parent).await;
    let mode = match invoke(&h.db, DbCommand::ModeSnapshot { id: "structure".into(), revision: Some(1) }, None).await.unwrap() {
        DbValue::ModeSnapshot(mode) => mode, other => panic!("{other:?}"),
    };
    let mut candidate = output("first-adopt"); candidate.id = "first-history-candidate".into();
    candidate.kind = RevisionKind::Adopt; candidate.creation_reason = CreationReason::History;
    candidate.mode_snapshot = mode.clone(); candidate.resolved_mode = BaseMode::Structure;
    let cancelled = parent.operation(); assert_eq!(cancelled.cancel(), CancelDecision::Accepted);
    assert!(matches!(invoke(&h.db, DbCommand::Adopt { session_id: "first-adopt".into(), expected_current_revision_id: String::new(), candidate: candidate.clone() }, Some(&cancelled)).await, Err(StoreError::Cancelled)));
    assert!(!parent.is_cancelled()); assert!(!parent.ready_committed());
    assert_eq!(usage(&h.db).await.count, 0);
    let child = parent.operation();
    let adopted = as_detail(invoke(&h.db, DbCommand::Adopt { session_id: "first-adopt".into(), expected_current_revision_id: String::new(), candidate: candidate.clone() }, Some(&child)).await.unwrap());
    assert!(child.ready_committed()); assert!(parent.ready_committed());
    assert_eq!(parent.cancel(), CancelDecision::AlreadyCompleted);
    let revision = adopted.output.unwrap();
    assert_eq!(revision.mode_snapshot, mode); assert_eq!(revision.parent_revision_id, None);
    assert_eq!(revision.base_revision_id, None); assert!(!adopted.can_undo); assert_eq!(adopted.source, source("first-adopt"));
    assert!(matches!(invoke(&h.db, DbCommand::Adopt { session_id: "first-adopt".into(), expected_current_revision_id: String::new(), candidate }, Some(&parent.operation())).await, Err(StoreError::Conflict { .. })));
    assert_eq!(usage(&h.db).await.count, 1);
    let mut attempt = delivery("first-adopt"); attempt.output_revision_id = revision.id;
    assert!(invoke(&h.db, DbCommand::PrepareDelivery { attempt }, Some(&parent)).await.is_err());
    stop(&h.db).await;
}

#[tokio::test]
async fn shared_delete_invalidates_an_inflight_child_and_persistent_tombstone_prevents_recreation() {
    let h = Harness::new(options()); let parent = guard("deleted");
    let initial = ready(&h.db, "deleted", StoragePolicy::History, &parent).await.output.unwrap();
    let child = parent.operation(); let lock = write_lock(&h.file);
    let mut blocks = initial.blocks; blocks[0].text = "must not resurrect".into();
    let pending = h.db.submit(DbCommand::Edit { session_id: "deleted".into(), expected_current_revision_id: initial.id, blocks, created_at: "late".into() }, Some(child.clone()), DbPriority::Normal).unwrap();
    let queued = queue_behind_active(&h.db).await;
    parent.delete(); assert!(child.is_cancelled());
    let deletion = h.db.submit(DbCommand::DeleteHistory { session_id: "deleted".into() }, Some(parent), DbPriority::Critical).unwrap();
    lock.execute_batch("ROLLBACK;").unwrap();
    assert!(matches!(wait(&pending).await, Err(StoreError::Cancelled)));
    wait(&deletion).await.unwrap(); wait(&queued).await.unwrap();
    assert!(matches!(invoke(&h.db, DbCommand::Checkpoint { seed: seed("deleted", StoragePolicy::History), source: source("deleted") }, Some(&guard("deleted"))).await, Err(StoreError::Cancelled)));
    assert_eq!(usage(&h.db).await.count, 1);
    let verify = h.file.connect();
    for table in ["sessions", "sources", "source_items", "outputs"] { assert_eq!(count(&verify, table), 0, "{table}"); }
    assert_eq!(count(&verify, "content_tombstones"), 1);
    drop(verify); drop(lock); stop(&h.db).await;
}

#[tokio::test]
async fn real_commit_busy_is_typed_and_rolls_back_before_retry_or_cancel_cleanup() {
    // Both variants wait on actual SQLite COMMIT, not a mocked Store delay.
    for (policy, cancel_at_commit) in [(StoragePolicy::MemoryOnly, false), (StoragePolicy::History, true)] {
        let h = Harness::new(options()); let gate = guard("commit-busy");
        checkpoint(&h.db,"commit-busy",policy,&gate).await;
        invoke(&h.db,DbCommand::TestExecuteBatch{sql:"PRAGMA busy_timeout=300;".into()},None).await.unwrap();
        let lock=read_lock(&h.file);
        let pending=h.db.submit(DbCommand::Ready{output:output("commit-busy")},Some(gate.clone()),DbPriority::Normal).unwrap();
        until(||gate.finalizing()).await;
        if cancel_at_commit {assert_eq!(cancel_promptly(gate.clone()).await,CancelDecision::Pending);}
        assert!(matches!(wait(&pending).await,Err(StoreError::Busy)),"COMMIT Busy must not be string-parsed or disguised as cancellation");
        assert!(!gate.finalizing());assert!(!gate.ready_committed());
        lock.execute_batch("ROLLBACK;").unwrap();drop(lock);
        let preserved=as_detail(invoke(&h.db,DbCommand::GetResult{session_id:"commit-busy".into()},None).await.unwrap());
        assert_eq!(preserved.source,source("commit-busy"));assert!(preserved.output.is_none());
        assert_eq!(usage(&h.db).await.count,0);assert!(h.db.snapshot().temporary_result.is_none());
        let verify=h.file.connect();assert_eq!(count(&verify,"outputs"),0);assert_eq!(count(&verify,"usage_events"),0);drop(verify);
        if cancel_at_commit {
            assert!(gate.is_cancelled());assert!(!gate.pending());
            invoke(&h.db,DbCommand::CancelUnfinished{session_id:"commit-busy".into()},Some(&gate)).await.unwrap();
            assert!(matches!(invoke(&h.db,DbCommand::GetResult{session_id:"commit-busy".into()},None).await,Err(StoreError::NotFound)));
        }else{
            let mut retry=output("commit-busy");retry.creation_reason=CreationReason::Retry;
            invoke(&h.db,DbCommand::Ready{output:retry},Some(&gate)).await.unwrap();
            assert_eq!(usage(&h.db).await.count,1);assert!(h.db.snapshot().temporary_result.is_some());
            assert!(invoke(&h.db,DbCommand::PrepareDelivery{attempt:delivery("commit-busy")},Some(&gate)).await.is_err());
        }
        assert!(h.db.cache_error().is_none());stop(&h.db).await;
    }
}

#[tokio::test]
async fn cancelled_settings_operation_cannot_commit_after_waiting_for_the_real_writer() {
    let h=Harness::new(options());let original=h.db.snapshot().preferences;
    let mut next=original.clone();next.language="zh".into();
    let operation=Arc::new(CommitGuard::new("settings-operation".into(),0));
    let lock=write_lock(&h.file);
    let ticket=h.db.submit(DbCommand::SavePreferences{next,expected_version:original.version},Some(operation.clone()),DbPriority::Normal).unwrap();
    let queued=queue_behind_active(&h.db).await;
    assert_eq!(cancel_promptly(operation).await,CancelDecision::Accepted);
    lock.execute_batch("ROLLBACK;").unwrap();drop(lock);
    assert!(matches!(wait(&ticket).await,Err(StoreError::Cancelled)));wait(&queued).await.unwrap();
    assert_eq!(h.db.snapshot().preferences,original);
    assert!(matches!(invoke(&h.db,DbCommand::Preferences,None).await.unwrap(),DbValue::Preferences(value) if value==original));
    stop(&h.db).await;
}

#[tokio::test]
async fn cancelled_dictionary_import_cannot_commit_after_waiting_for_a_real_writer() {
    let h=Harness::new(options());let original=h.db.snapshot().dictionary;
    let operation=Arc::new(CommitGuard::new("dictionary-operation".into(),0)).operation();
    let lock=write_lock(&h.file);
    let ticket=h.db.submit(DbCommand::ImportDictionary{terms:vec!["CancelledTerm".into()],expected_version:original.version},Some(operation.clone()),DbPriority::Normal).unwrap();
    let queued=queue_behind_active(&h.db).await;
    assert_eq!(cancel_promptly(operation).await,CancelDecision::Accepted);
    lock.execute_batch("ROLLBACK;").unwrap();drop(lock);
    assert!(matches!(wait(&ticket).await,Err(StoreError::Cancelled)));wait(&queued).await.unwrap();
    assert_eq!(h.db.snapshot().dictionary,original);
    assert!(matches!(invoke(&h.db,DbCommand::Dictionary,None).await.unwrap(),DbValue::Dictionary(value) if value==original));
    stop(&h.db).await;
}

#[tokio::test]
async fn dictionary_cas_rechecks_version_after_a_competing_sql_writer_wins() {
    let h=Harness::new(options());let operation=Arc::new(CommitGuard::new("dictionary-cas".into(),0)).operation();
    let lock=write_lock(&h.file);
    let ticket=h.db.submit(DbCommand::ImportDictionary{terms:vec!["StaleTerm".into()],expected_version:1},Some(operation),DbPriority::Normal).unwrap();
    let queued=queue_behind_active(&h.db).await;
    lock.execute_batch("UPDATE dictionary_meta SET version=2;INSERT INTO dictionary_entries(id,normalized_term,term,created_at,updated_at) VALUES('external-winner','winner','Winner','test','test');COMMIT;").unwrap();drop(lock);
    assert!(matches!(wait(&ticket).await,Err(StoreError::Conflict{current}) if current=="2"));wait(&queued).await.unwrap();
    let DbValue::Dictionary(saved)=invoke(&h.db,DbCommand::Dictionary,None).await.unwrap()else{panic!("wrong dictionary type")};
    assert_eq!(saved.version,2);assert_eq!(saved.entries.iter().map(|entry|entry.term.as_str()).collect::<Vec<_>>(),["Winner"]);
    stop(&h.db).await;
}

#[tokio::test]
async fn dictionary_cache_does_not_retain_deleted_words_when_an_unrelated_refresh_fails() {
    let h=Harness::new(options());let operation=Arc::new(CommitGuard::new("dictionary-add".into(),0)).operation();
    let DbValue::Dictionary(saved)=invoke(&h.db,DbCommand::ImportDictionary{terms:vec!["RemoveMe".into()],expected_version:1},Some(&operation)).await.unwrap()else{panic!("wrong dictionary type")};
    assert_eq!(operation.cancel(),CancelDecision::AlreadyCompleted);
    invoke(&h.db,DbCommand::TestExecuteBatch{sql:"ALTER TABLE preferences RENAME TO unavailable_preferences;".into()},None).await.unwrap();
    let deletion=Arc::new(CommitGuard::new("dictionary-delete".into(),0)).operation();
    let DbValue::Dictionary(deleted)=invoke(&h.db,DbCommand::DeleteDictionary{entry_id:saved.entries[0].id.clone(),expected_version:saved.version},Some(&deletion)).await.unwrap()else{panic!("wrong dictionary type")};
    assert!(deleted.entries.is_empty());assert_eq!(h.db.snapshot().dictionary,deleted);assert!(h.db.cache_error().is_some());
    stop(&h.db).await;
}

#[tokio::test]
async fn dictionary_mutations_require_guard_and_observer_drop_does_not_cancel_owned_commit() {
    let h=Harness::new(options());
    assert!(matches!(invoke(&h.db,DbCommand::ImportDictionary{terms:vec!["NoGuard".into()],expected_version:1},None).await,Err(StoreError::Invalid(_))));
    let operation=Arc::new(CommitGuard::new("dictionary-owned".into(),0)).operation();let lock=write_lock(&h.file);
    let ticket=h.db.submit(DbCommand::ImportDictionary{terms:vec!["OwnedTerm".into()],expected_version:1},Some(operation.clone()),DbPriority::Normal).unwrap();
    let observer=ticket.clone();let waiter=tokio::spawn(async move{observer.wait().await});let queued=queue_behind_active(&h.db).await;
    waiter.abort();let _=waiter.await;lock.execute_batch("ROLLBACK;").unwrap();drop(lock);
    assert!(matches!(wait(&ticket).await.unwrap(),DbValue::Dictionary(snapshot) if snapshot.version==2));wait(&queued).await.unwrap();
    assert_eq!(h.db.snapshot().dictionary.entries[0].term,"OwnedTerm");assert_eq!(operation.cancel(),CancelDecision::AlreadyCompleted);stop(&h.db).await;
}

#[tokio::test]
async fn dropping_the_last_client_drains_work_and_releases_the_real_database_owner() {
    let file=DbPath::new();let db=DbClient::open_with_options(&file.0,options()).unwrap();
    let gate=guard("abandoned-client");let lock=write_lock(&file);
    let pending=db.submit(DbCommand::Checkpoint{seed:seed("abandoned-client",StoragePolicy::History),source:source("abandoned-client")},Some(gate),DbPriority::Normal).unwrap();
    let queued=queue_behind_active(&db).await;
    drop(db); // Deliberately no request_shutdown and no Harness Drop implementation.
    lock.execute_batch("ROLLBACK;").unwrap();drop(lock);
    wait(&pending).await.unwrap();wait(&queued).await.unwrap();
    until(||fs::remove_file(&file.0).is_ok()).await;
}

#[tokio::test]
async fn temporary_body_is_redacted_when_clear_or_replacement_succeeds_but_refresh_fails() {
    let h=Harness::new(options());let gate=guard("temporary-old");
    ready(&h.db,"temporary-old",StoragePolicy::MemoryOnly,&gate).await;
    assert!(h.db.snapshot().temporary_result.is_some());
    invoke(&h.db,DbCommand::TestExecuteBatch{sql:"ALTER TABLE preferences RENAME TO saved_preferences;".into()},None).await.unwrap();
    assert!(h.db.cache_error().is_some());
    invoke(&h.db,DbCommand::ClearTemporary,None).await.unwrap();
    assert!(h.db.snapshot().temporary_result.is_none());assert!(h.db.cache_error().is_some());
    assert!(matches!(invoke(&h.db,DbCommand::GetResult{session_id:"temporary-old".into()},None).await,Err(StoreError::NotFound)));
    invoke(&h.db,DbCommand::TestExecuteBatch{sql:"ALTER TABLE saved_preferences RENAME TO preferences;".into()},None).await.unwrap();
    ready(&h.db,"temporary-new",StoragePolicy::MemoryOnly,&guard("temporary-new")).await;
    invoke(&h.db,DbCommand::TestExecuteBatch{sql:"ALTER TABLE preferences RENAME TO saved_preferences;".into()},None).await.unwrap();
    ready(&h.db,"temporary-replacement",StoragePolicy::MemoryOnly,&guard("temporary-replacement")).await;
    assert!(h.db.snapshot().temporary_result.is_none(),"cache must not retain superseded plaintext");
    assert!(as_detail(invoke(&h.db,DbCommand::GetResult{session_id:"temporary-replacement".into()},None).await.unwrap()).output.is_some());
    assert!(matches!(invoke(&h.db,DbCommand::GetResult{session_id:"temporary-new".into()},None).await,Err(StoreError::NotFound)));
    invoke(&h.db,DbCommand::TestExecuteBatch{sql:"ALTER TABLE saved_preferences RENAME TO preferences;".into()},None).await.unwrap();
    assert_eq!(h.db.snapshot().temporary_result.unwrap().session.seed.id,"temporary-replacement");
    assert_eq!(usage(&h.db).await.count,3);stop(&h.db).await;
}

#[tokio::test]
async fn sibling_operation_cancel_does_not_wait_forever_on_another_operations_commit_gate() {
    let h=Harness::new(options());let parent=guard("siblings");
    let original=ready(&h.db,"siblings",StoragePolicy::History,&parent).await;
    let first=parent.operation();let second=parent.operation();let lock=read_lock(&h.file);
    let current=original.output.unwrap();let mut blocks=current.blocks.clone();blocks[0].text="第一个编辑".into();
    let pending=h.db.submit(DbCommand::Edit{session_id:"siblings".into(),expected_current_revision_id:current.id,blocks,created_at:"now".into()},Some(first.clone()),DbPriority::Normal).unwrap();
    until(||first.finalizing()).await;
    assert_eq!(cancel_promptly(second.clone()).await,CancelDecision::Accepted);
    assert!(second.is_cancelled());assert!(!second.pending());
    lock.execute_batch("ROLLBACK;").unwrap();drop(lock);wait(&pending).await.unwrap();
    assert_eq!(first.cancel(),CancelDecision::AlreadyCompleted);assert!(!parent.is_cancelled());
    assert_eq!(usage(&h.db).await.count,1);stop(&h.db).await;
}
