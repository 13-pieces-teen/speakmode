//! Synthetic Typeless words through the real AppHub/SQLite dictionary path.
//! Readers are injected: no APPDATA, Typeless files, credentials or user data.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn imported(h: &AppHub, expected: u64, terms: &[&str]) -> Result<Value> {
    let terms = terms.iter().map(|term| term.to_string()).collect();
    h.import_typeless_dictionary_with(expected, move || Ok(terms)).await
}

async fn stored(h: &AppHub) -> DictionarySnapshot {
    match h.db.submit(DbCommand::Dictionary, None, DbPriority::Normal).unwrap().wait().await.unwrap() {
        DbValue::Dictionary(value) => value,
        _ => panic!("unexpected dictionary result"),
    }
}

#[tokio::test]
async fn typeless_append_uses_store_dedup_and_preserves_existing_spelling_and_identity() {
    let h = hub();
    let before = imported(&h, 1, &["SpeakMode"]).await.unwrap();
    let after = imported(&h, 2, &["ＳＰＥＡＫＭＯＤＥ", " PolyU ", "polyu"]).await.unwrap();
    assert_eq!(after["version"], 3);
    assert_eq!(after["entries"].as_array().unwrap().len(), 2);
    let existing = after["entries"].as_array().unwrap().iter().find(|entry| entry["term"] == "SpeakMode").unwrap();
    assert_eq!(existing, &before["entries"][0]);
    assert!(after["entries"].as_array().unwrap().iter().any(|entry| entry["term"] == "PolyU"));
    assert_eq!(imported(&h, 3, &["speakmode", "ＰＯＬＹＵ"]).await.unwrap(), after);
    assert_eq!(wire(stored(&h).await).unwrap(), after);
}

#[tokio::test]
async fn typeless_source_failure_does_not_write_and_preserves_stable_error() {
    let h = hub();
    imported(&h, 1, &["KeepMe"]).await.unwrap();
    let before = stored(&h).await;
    let error = h.import_typeless_dictionary_with(2, || {
        Err(ApiError::new("typeless_not_found", "dictionary", "未找到 Typeless 本地词典。"))
    }).await.unwrap_err();
    assert_eq!(error.code, "typeless_not_found");
    assert_eq!(error.stage, "dictionary");
    assert_eq!(error.message, "未找到 Typeless 本地词典。");
    assert_eq!(stored(&h).await, before);
}

#[tokio::test]
async fn typeless_stale_version_is_rejected_before_source_is_read() {
    let h = hub();
    imported(&h, 1, &["Current"]).await.unwrap();
    let before = stored(&h).await;
    let reads = Arc::new(AtomicUsize::new(0));
    let reader_reads = reads.clone();
    let error = h.import_typeless_dictionary_with(1, move || {
        reader_reads.fetch_add(1, Ordering::SeqCst);
        Ok(vec!["MustNotRead".into()])
    }).await.unwrap_err();
    assert_eq!(error.code, "revisionConflict");
    assert_eq!(error.stage, "dictionary");
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(stored(&h).await, before);
}

#[tokio::test]
async fn typeless_ipc_scope_and_missing_or_stale_version_fail_before_source_access() {
    let h = hub();
    for window in ["recording", "unknown"] {
        let error = h.command(window, CommandRequest {
            name: "dictionary.importTypeless".into(),
            payload: json!({"expectedVersion":1,"requestId":"typeless-forbidden"}),
        }).await.unwrap_err();
        assert_eq!(error.code, "forbidden");
    }
    let missing = command(&h, "dictionary.importTypeless", json!({"requestId":"typeless-no-version"})).await.unwrap_err();
    assert_eq!(missing.code, "invalid_payload");
    let stale = command(&h, "dictionary.importTypeless", json!({"expectedVersion":0,"requestId":"typeless-stale-route"})).await.unwrap_err();
    assert_eq!(stale.code, "revisionConflict");
    assert_eq!(stored(&h).await.version, 1);
    assert!(stored(&h).await.entries.is_empty());
}

#[tokio::test]
async fn typeless_empty_invalid_and_oversized_imports_are_atomic() {
    let h = hub();
    imported(&h, 1, &["KeepMe"]).await.unwrap();
    let before = stored(&h).await;
    let cases = vec![
        (vec![], "dictionary_import_limit"),
        (vec!["ValidFirst".into(), "".into()], "dictionary_empty"),
        (vec!["ValidFirst".into(), "bad\nword".into()], "dictionary_control"),
        (vec!["ValidFirst".into(), "x".repeat(81)], "dictionary_term_limit"),
        ((0..201).map(|i| format!("Term{i}")).collect(), "dictionary_import_limit"),
        ((0..100).map(|i| format!("{i:03}{}", "x".repeat(70))).collect(), "dictionary_context_limit"),
    ];
    for (terms, code) in cases {
        let error = h.import_typeless_dictionary_with(2, move || Ok(terms)).await.unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(stored(&h).await, before, "{code} must not append even the valid prefix");
    }
}

#[tokio::test]
async fn typeless_append_respects_existing_dictionary_capacity() {
    let h = hub();
    let terms = (0..DICTIONARY_MAX_ENTRIES).map(|i| format!("t{i:03}")).collect();
    h.import_typeless_dictionary_with(1, move || Ok(terms)).await.unwrap();
    let before = stored(&h).await;
    let error = imported(&h, 2, &["NewWord"]).await.unwrap_err();
    assert_eq!(error.code, "dictionary_entry_limit");
    assert_eq!(stored(&h).await, before);
}

#[tokio::test]
async fn typeless_concurrent_dictionary_change_during_read_is_not_overwritten() {
    let h = hub();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker_hub = h.clone();
    let worker = tokio::spawn(async move {
        worker_hub.import_typeless_dictionary_with(1, move || {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            Ok(vec!["StaleImport".into()])
        }).await
    });
    entered_rx.await.unwrap();
    let current = imported(&h, 1, &["ConcurrentEdit"]).await.unwrap();
    release_tx.send(()).unwrap();
    let error = worker.await.unwrap().unwrap_err();
    assert_eq!(error.code, "revisionConflict");
    assert_eq!(wire(stored(&h).await).unwrap(), current);
}

#[tokio::test]
async fn typeless_database_failure_rolls_back_terms_and_version() {
    let h = hub();
    imported(&h, 1, &["KeepMe"]).await.unwrap();
    let before = stored(&h).await;
    h.db.submit(DbCommand::TestExecuteBatch {
        sql: "CREATE TRIGGER reject_typeless_import BEFORE INSERT ON dictionary_entries BEGIN SELECT RAISE(ABORT,'synthetic import failure'); END;".into(),
    }, None, DbPriority::Normal).unwrap().wait().await.unwrap();
    let error = imported(&h, 2, &["NewFirst", "NewSecond"]).await.unwrap_err();
    assert_eq!(error.code, "storageFailed");
    assert_eq!(error.stage, "dictionary");
    assert_eq!(stored(&h).await, before);
}

#[tokio::test]
async fn typeless_reader_panic_is_a_read_failure_without_dictionary_mutation() {
    let h = hub();
    let before = stored(&h).await;
    let error = h.import_typeless_dictionary_with(1, || panic!("synthetic reader failure")).await.unwrap_err();
    assert_eq!(error.code, "typeless_import_unavailable");
    assert_eq!(error.stage, "dictionary");
    assert_eq!(stored(&h).await, before);
}

#[tokio::test]
async fn typeless_replay_uses_dictionary_version_without_retaining_or_rereading_source() {
    let h = hub();
    let reads = Arc::new(AtomicUsize::new(0));
    let reader_reads = reads.clone();
    let saved = h.import_typeless_dictionary_with(1, move || {
        reader_reads.fetch_add(1, Ordering::SeqCst);
        Ok(vec!["ImportedOnce".into()])
    }).await.unwrap();
    let replay = AppHub::replay_reference("dictionary.importTypeless", &saved);
    assert!(matches!(&replay, ReplayValue::Dictionary(2)));
    assert_eq!(h.replay(replay.clone()).await.unwrap(), saved);
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    command(&h, "dictionary.delete", json!({"entryId":saved["entries"][0]["id"],"expectedVersion":2,"requestId":"delete-typeless-import"})).await.unwrap();
    assert_eq!(h.replay(replay).await.unwrap_err().code, "request_already_processed");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert!(stored(&h).await.entries.is_empty());
}
