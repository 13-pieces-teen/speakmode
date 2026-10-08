//! Real AppHub command/Store behavior with synthetic fixtures; no credentials,
//! microphone, provider requests, clipboard or native input are exercised.
use super::*;

#[tokio::test]
async fn incompatible_model_configuration_does_not_change_saved_settings() {
    let hub=tests::hub();let original=hub.db.snapshot().provider_config;
    let mut config=original.clone();config.region=Some("beijing".into());
    for (asr,adapter,rewrite,code) in [
        ("qwen-audio-3.0-asr-flash-streaming","qwen-realtime-v1","qwen-flash","asr_adapter_not_implemented"),
        ("qwen-audio-3.1-asr-flash-streaming","qwen-realtime-v1","qwen-flash","asr_adapter_mismatch"),
        ("qwen3-asr-flash-realtime","task-streaming","qwen-flash","asr_adapter_mismatch"),
        ("unknown-model","qwen-realtime-v1","qwen-flash","unsupported_asr_model"),
        ("qwen3-asr-flash-realtime","qwen-realtime-v1","unknown-model","unsupported_rewrite_model"),
    ] {
        config.asr_model=asr.into();config.adapter_version=adapter.into();config.rewrite_model=rewrite.into();
        let error=hub.command("main",CommandRequest{name:"provider.configure".into(),payload:json!({"requestId":Uuid::new_v4().to_string(),"config":config,"expectedVersion":original.version})}).await.unwrap_err();
        assert_eq!(error.code,code);assert_eq!(hub.db.snapshot().provider_config,original);
        assert!(hub.inner.lock().unwrap().settings_pending.is_none());
    }
}

#[test]
fn main_snapshot_exposes_compatibility_but_does_not_advertise_future_adapters_as_available() {
    let hub=tests::hub();let snapshot=hub.snapshot("main").unwrap();
    let models=snapshot["provider"]["modelCatalog"]["asr"].as_array().unwrap();
    assert!(models.iter().any(|m|m["id"]=="qwen3-asr-flash-realtime-2026-02-10"&&m["implemented"]==true));
    assert!(models.iter().any(|m|m["id"]=="qwen-audio-3.1-asr-flash-streaming"&&m["implemented"]==true&&m["adapterVersion"]=="qwen-task-streaming-v1"));
    assert!(models.iter().any(|m|m["id"]=="qwen-audio-3.0-asr-flash-streaming"&&m["implemented"]==false));
    assert!(hub.snapshot("recording").unwrap().get("provider").is_none());
}

fn failed_summary(error:&str)->CaptureSummary {
    CaptureSummary{complete:false,cancelled:false,captured_samples:48000,input_sample_rate:48000,
        emitted_frames:10,first_samples_received:true,error:Some(error.into())}
}

#[test]
fn capture_fault_has_its_own_recovery_reason_and_cancellation_is_not_overridden() {
    for (reason,code) in [
        (platform::audio::PCM_STALL_ERROR,"audio_capture_stalled"),
        ("microphone first-sample deadline exceeded","audio_first_sample_timeout"),
        ("microphone stream error/disconnected","audio_capture_device_error"),
        ("audio input queue overflow","audio_capture_queue_overflow"),
        ("malformed microphone frame","audio_capture_malformed"),
    ] {
        let mut summary=failed_summary(reason);let error=capture_failure(&summary).unwrap();
        assert_eq!(error.code,code);assert_eq!(error.stage,"audio");
        summary.cancelled=true;assert!(capture_failure(&summary).is_none());
    }
    assert!(capture_failure(&failed_summary("unclassified driver payload")).is_none());
}

#[test]
fn stalled_audio_keeps_existing_caption_without_ready_usage_or_delivery() {
    let hub=tests::hub();tests::install(&hub,"quality-audio",SessionPhase::Recording,false);
    {let mut inner=hub.inner.lock().unwrap();let session=inner.active.as_mut().unwrap();session.first_samples=true;
        session.captions=vec![CaptionItem{id:"kept".into(),text:"如果没确认，先不要发邀请。".into(),revision:1,is_final:false,capture_order:0}];}
    hub.fail("quality-audio",7,capture_failure(&failed_summary(platform::audio::PCM_STALL_ERROR)).unwrap(),true);
    let snapshot=hub.snapshot("main").unwrap();
    assert_eq!(snapshot["session"]["state"],"incomplete");
    assert_eq!(snapshot["session"]["errorCode"],"audio_capture_stalled");
    assert_eq!(snapshot["session"]["captions"][0]["text"],"如果没确认，先不要发邀请。");
    assert_eq!(snapshot["session"]["workerReleased"],true);
    assert_eq!(snapshot["session"]["canRetryAsr"],false);
    assert_eq!(snapshot["usage"]["count"],0);
    assert!(snapshot["session"]["result"].is_null());
}
