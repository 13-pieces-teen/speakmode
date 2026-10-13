//! Dictionary lifetime checks against the actual coordinator and SQLite cache.
//! No credentials, cloud requests, microphone capture or native delivery.
use super::*;
use super::tests::hub;

async fn import(h:&Arc<AppHub>,term:&str)->DictionarySnapshot {
    let expected=h.db.snapshot().dictionary.version;
    let result=h.command("main",CommandRequest{name:"dictionary.import".into(),payload:json!({
        "terms":[term],"expectedVersion":expected,"requestId":Uuid::new_v4().to_string()
    })}).await.unwrap();
    serde_json::from_value(result).unwrap()
}
fn configure_fixture(h:&Arc<AppHub>,region:&str)->ProviderConfig {
    let mut config=h.db.snapshot().provider_config;let expected=config.version;config.region=Some(region.into());
    h.db.submit(DbCommand::SaveProviderConfig{config,expected_version:expected},None,DbPriority::Normal).unwrap().wait_blocking().unwrap();
    // prepare_start_state checks only this cached status. The test never calls
    // start_session_at, so this does not access a real Credential Manager key.
    h.inner.lock().unwrap().key_configured=true;
    h.db.snapshot().provider_config
}

#[tokio::test]
async fn recording_rewrite_keeps_start_vocabulary_after_live_dictionary_edit() {
    let h=hub();configure_fixture(&h,"beijing");
    let before=import(&h,"SpeakMode").await;
    let (id,generation,_,_,_,asr_dictionary)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    assert_eq!(asr_dictionary,before);
    let after=import(&h,"Typeless").await;
    assert_ne!(after.version,before.version);
    // This is the exact helper used by finish_rewrite immediately before it
    // builds the cloud closure; both ASR and rewrite keep the same vocabulary.
    let frozen=h.recording_rewrite_dictionary(&id,generation).unwrap();
    assert_eq!(frozen,asr_dictionary);
    assert_eq!(frozen.entries.iter().map(|entry|entry.term.as_str()).collect::<Vec<_>>(),vec!["SpeakMode"]);
    assert_eq!(h.db.snapshot().dictionary.entries.len(),2);
}

#[tokio::test]
async fn rewrite_dictionary_cannot_be_borrowed_from_another_or_cancelled_recording() {
    let h=hub();configure_fixture(&h,"beijing");import(&h,"SpeakMode").await;
    let (id,generation,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    assert!(h.recording_rewrite_dictionary("other-session",generation).is_none());
    assert!(h.recording_rewrite_dictionary(&id,generation+1).is_none());
    assert!(h.recording_rewrite_dictionary(&id,generation).is_some());
    h.inner.lock().unwrap().active.as_ref().unwrap().cancel.cancel();
    assert!(h.recording_rewrite_dictionary(&id,generation).is_none());
}

#[tokio::test]
async fn explicit_reprocess_freezes_current_provider_and_dictionary_for_its_whole_operation() {
    let h=hub();let old_config=configure_fixture(&h,"beijing");
    let old_dictionary=import(&h,"SpeakMode").await;
    let (frozen_config,frozen_dictionary)=h.auxiliary_rewrite_snapshot();
    let current_dictionary=import(&h,"Typeless").await;
    let current_config=configure_fixture(&h,"singapore");
    assert_eq!(frozen_config,old_config);assert_eq!(frozen_dictionary,old_dictionary);
    let (next_config,next_dictionary)=h.auxiliary_rewrite_snapshot();
    assert_eq!(next_config,current_config);assert_eq!(next_dictionary,current_dictionary);
    assert_ne!(next_dictionary.version,frozen_dictionary.version);
    assert_ne!(next_config.region,frozen_config.region);
}
