//! Real command/configuration/state checks without Credential Manager or cloud.
use super::*;
use super::tests::hub;

fn mixed_config(h: &AppHub) -> ProviderConfig {
    let mut config=h.db.snapshot().provider_config;
    let expected=config.version;
    config.region=Some("beijing".into());
    config.rewrite_provider="openai".into();
    config.rewrite_model="gpt-5.4-mini".into();
    let DbValue::ProviderConfig(config)=h.db.submit(DbCommand::SaveProviderConfig { config, expected_version:expected },None,DbPriority::Normal).unwrap().wait_blocking().unwrap() else {panic!("wrong fixture receipt")};
    config
}

#[tokio::test]
async fn credential_commands_reject_cross_provider_account_before_native_key_access() {
    let h=hub();let config=mixed_config(&h);
    for (stage,provider) in [("asr","openai"),("rewrite","aliyun")] {
        let error=h.command("main",CommandRequest { name:"credential.delete".into(),payload:json!({
            "requestId":Uuid::new_v4().to_string(),"expectedProviderVersion":config.version,
            "stage":stage,"providerAccountRef":provider,
        }) }).await.unwrap_err();
        assert_eq!(error.code,"credential_account_mismatch");
        assert_eq!(h.db.snapshot().provider_config,config);
    }
}

#[tokio::test]
async fn credential_stage_and_config_version_are_checked_before_native_key_access() {
    let h=hub();let config=mixed_config(&h);
    for (version,stage,code) in [(config.version,"other","invalid_payload"),(config.version-1,"rewrite","revisionConflict")] {
        let error=h.command("main",CommandRequest { name:"credential.delete".into(),payload:json!({
            "requestId":Uuid::new_v4().to_string(),"expectedProviderVersion":version,
            "stage":stage,"providerAccountRef":"openai",
        }) }).await.unwrap_err();
        assert_eq!(error.code,code);
    }
}

#[test]
fn credential_state_requires_both_stages_and_exposes_each_status() {
    let h=hub();mixed_config(&h);
    for (asr,rewrite) in [(false,false),(true,false),(false,true),(true,true)] {
        CredentialAvailability { asr,rewrite }.apply(&mut h.inner.lock().unwrap());
        let status=h.snapshot("main").unwrap()["provider"].clone();
        assert_eq!(status["asrCredentialConfigured"],asr);
        assert_eq!(status["rewriteCredentialConfigured"],rewrite);
        assert_eq!(status["credentialConfigured"],asr&&rewrite);
        assert_eq!(status["configured"],asr&&rewrite);
    }
}

#[test]
fn recording_and_recovery_keep_provider_snapshot_when_preferences_change() {
    let h=hub();let original=mixed_config(&h);
    CredentialAvailability { asr:true,rewrite:true }.apply(&mut h.inner.lock().unwrap());
    let (id,generation,frozen,_,_,_)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    let mut next=original.clone();next.rewrite_provider="aliyun".into();next.rewrite_model="qwen3.8-flash".into();
    h.db.submit(DbCommand::SaveProviderConfig { config:next,expected_version:original.version },None,DbPriority::Normal).unwrap().wait_blocking().unwrap();
    assert_eq!(frozen,original);
    {
        let mut inner=h.inner.lock().unwrap();let session=inner.active.as_mut().unwrap();
        assert_eq!(session.id,id);assert_eq!(session.generation,generation);assert_eq!(session.config,original);
        session.phase=SessionPhase::Incomplete;session.processing_worker_released=true;
        session.cache=Some(vec![1;1600]);session.cache_complete=true;session.cache_created=Some(Instant::now());
        inner.park_retryable_failure().unwrap();
        assert_eq!(inner.recovery.as_ref().unwrap().config,original);
    }
    let (history_config,_)=h.auxiliary_rewrite_snapshot();
    assert_eq!(history_config.rewrite_provider,"aliyun");
    assert_eq!(h.db.snapshot().provider_config.rewrite_provider,"aliyun");
}

#[test]
fn omitted_stage_keeps_old_asr_credential_commands_compatible() {
    let mut config=ProviderConfig::default();config.region=Some("beijing".into());
    assert_eq!(credential_stage(&json!({"providerAccountRef":"aliyun"}),&config).unwrap(),CredentialStage::Asr);
    assert_eq!(credential_stage(&json!({"stage":"rewrite","providerAccountRef":"aliyun"}),&config).unwrap(),CredentialStage::Rewrite);
}

#[test]
fn selected_google_product_limit_is_frozen_and_uses_existing_warning_and_recovery_path() {
    let h=hub();let mut config=mixed_config(&h);let version=config.version;
    config.provider="google".into();config.region=None;config.asr_model="gemini-3.5-transcribe".into();config.adapter_version="google-transcribe-v1".into();
    h.db.submit(DbCommand::SaveProviderConfig { config,expected_version:version },None,DbPriority::Normal).unwrap().wait_blocking().unwrap();
    CredentialAvailability { asr:true,rewrite:true }.apply(&mut h.inner.lock().unwrap());
    let (id,generation,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().recording_limit_ms,420_000);
    // A later preference write cannot enlarge this recording's model limit.
    let mut next=h.db.snapshot().provider_config;let version=next.version;
    next.provider="aliyun".into();next.region=Some("beijing".into());next.asr_model="qwen3-asr-flash-realtime".into();next.adapter_version="qwen-realtime-v1".into();
    h.db.submit(DbCommand::SaveProviderConfig { config:next,expected_version:version },None,DbPriority::Normal).unwrap().wait_blocking().unwrap();
    assert!(!h.recording_progress(&id,generation,389_999));
    assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().view().recording_limit_remaining_ms,None);
    assert!(!h.recording_progress(&id,generation,390_000));
    assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().view().recording_limit_remaining_ms,Some(30_000));
    assert!(!h.recording_progress(&id,generation,419_999));
    assert!(h.recording_progress(&id,generation,420_000));
    let inner=h.inner.lock().unwrap();let view=inner.active.as_ref().unwrap().view();
    assert_eq!(view.state,SessionPhase::Incomplete);assert_eq!(view.recording_limit_remaining_ms,Some(0));
    assert_eq!(view.error_code.as_deref(),Some("recording_limit"));
}
