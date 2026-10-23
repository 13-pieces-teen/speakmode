use super::*;

/// Explicit opt-in, one synthetic document. Reads only provider_config; the
/// credential stays inside the normal native credential/rewrite implementation.
#[tokio::test]
#[ignore = "requires explicit cloud-probe configuration and one billable synthetic rewrite"]
async fn onboarding_live_document_probe() {
    let config_path=std::env::var("SPEAKMODE_PROBE_CONFIG_DB").expect("explicit config database required");
    let output_path=std::env::var("SPEAKMODE_PROBE_OUTPUT").expect("new probe receipt required");
    let file=std::fs::OpenOptions::new().write(true).create_new(true).open(output_path).unwrap();
    let connection=rusqlite::Connection::open_with_flags(config_path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let config_json:String=connection.query_row("SELECT data FROM provider_config WHERE singleton=1",[],|r|r.get(0)).unwrap();
    let config:ProviderConfig=serde_json::from_str(&config_json).unwrap();
    let secret=credentials::read_for(&config,CredentialStage::Rewrite).expect("configured native rewrite credential required");
    let h=hub();install(&h,"practice-probe",SessionPhase::FinalizingAsr,true);
    let mode=h.db.mode_snapshot("document",None).unwrap();
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.onboarding_lesson=Some(OnboardingLesson::Document);s.prefs.delivery_mode=DeliveryMode::Preview;s.mode=mode;s.config=config.clone();}
    let mut frozen=source("practice-probe");frozen.items.truncate(1);
    frozen.items[0].text="今天同步三件事。登录页已经完成。支付页还在联调，预计周五完成。发布前先补一轮回归测试，通过以后再上线。".into();
    h.checkpoint("practice-probe",7,frozen.clone()).unwrap();
    let began=Instant::now();h.finish_rewrite("practice-probe".into(),7,secret).await;
    let detail=h.db.get_result("practice-probe").unwrap();
    let receipt=json!({"scope":"one fixed onboarding text; live configured rewrite and review; no audio, clipboard or host input","provider":config.rewrite_provider,"model":config.rewrite_model,"elapsedMs":began.elapsed().as_millis(),"sourceUnchanged":detail.source==frozen,"output":detail.output});
    serde_json::to_writer_pretty(file,&receipt).unwrap();
    assert!(detail.output.as_ref().is_some_and(|o|o.rewrite_fallback.is_none()),"live rewrite did not produce a reviewed output; see receipt");
    assert_eq!(detail.source,frozen);assert_eq!(usage_count(&h),1);
}

#[tokio::test(start_paused=true)]
async fn onboarding_document_waits_for_validated_output_past_the_daily_short_budget() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.onboarding_lesson=Some(OnboardingLesson::Document);s.prefs.delivery_mode=DeliveryMode::Preview;}
    h.finish_rewrite_with_policy("s".into(),7,|_,_,frozen,_,_|async move {
        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok(rewrite_fallback::source_fallback(&frozen))
    },None::<RewriteWaitPolicy>,|hub,_,_|hub.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true).await;
    let saved=h.db.get_result("s").unwrap();
    assert!(saved.output.unwrap().rewrite_fallback.is_none(),"practice must allow the complete 20s pipeline");
    assert_eq!(saved.source,source("s"));assert_eq!(usage_count(&h),1);
}

#[tokio::test]
async fn onboarding_source_fallback_requires_acknowledging_the_exact_output() {
    let h=hub();command(&h,"onboarding.open",json!({"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    step(&h,"document").await;
    install(&h,"s",SessionPhase::Rewriting,true);
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.onboarding_lesson=Some(OnboardingLesson::Document);s.prefs.delivery_mode=DeliveryMode::Preview;}
    checkpoint(&h,"s");let mut result=output(&h,"s");result.rewrite_fallback=Some(RewriteFallbackReason::Timeout);
    result.blocks=rewrite_fallback::source_fallback(&source("s")).blocks;result.resolved_mode=BaseMode::Light;
    h.commit_output("s",7,result.clone()).unwrap();
    let accept=|revision:Option<&str>|json!({"sessionId":"s","expectedGeneration":7,"expectedVersion":h.db.snapshot().onboarding.version,"acceptedFallbackRevisionId":revision,"requestId":Uuid::new_v4().to_string()});
    assert_eq!(command(&h,"onboarding.acceptLesson",accept(None)).await.unwrap_err().code,"source_confirmation_required");
    assert_eq!(command(&h,"onboarding.acceptLesson",accept(Some("old-output"))).await.unwrap_err().code,"source_confirmation_required");
    assert!(h.db.snapshot().onboarding.completed_lessons.is_empty());
    command(&h,"onboarding.acceptLesson",accept(Some(&result.id))).await.unwrap();
    assert_eq!(h.db.snapshot().onboarding.completed_lessons,vec![OnboardingLesson::Document]);
}

#[tokio::test(flavor="current_thread")]
async fn recorder_resize_does_not_block_async_mode_confirmation() {
    let h=hub();install(&h,"s",SessionPhase::Recording,false);
    let entered=Arc::new(AtomicBool::new(false));let observed=entered.clone();
    h.set_recorder_control(Arc::new(move|width,height,menu|{
        assert_eq!((width,height,menu),(240.0,64.0,false));
        observed.store(true,Ordering::SeqCst);std::thread::sleep(Duration::from_millis(160));Ok(())
    }));
    let worker=h.clone();let began=Instant::now();
    let resize=tokio::spawn(async move{worker.command("recording",CommandRequest{name:"window.resizeRecorder".into(),payload:json!({"requestId":Uuid::new_v4().to_string(),"width":240,"height":64,"menuOpen":false})}).await});
    tokio::time::timeout(Duration::from_secs(2),async{while !entered.load(Ordering::SeqCst)&&!resize.is_finished(){tokio::task::yield_now().await;}}).await.unwrap();
    assert!(entered.load(Ordering::SeqCst),"resize request rejected before reaching the native control");
    h.select_session_mode_async(&json!({"sessionId":"s","expectedGeneration":7,"modeId":"document","revision":1})).await.unwrap();
    let elapsed=began.elapsed();resize.await.unwrap().unwrap();
    assert!(elapsed<Duration::from_millis(100),"native geometry blocked mode confirmation: {elapsed:?}");
    assert_eq!(view(&h).mode_snapshot.mode_id,"document");
}

async fn step(h: &Arc<AppHub>, value: &str) {
    let version = h.snapshot("main").unwrap()["onboarding"]["progress"]["version"].as_u64().unwrap();
    command(h, "onboarding.step", json!({"step":value,"expectedVersion":version,"requestId":Uuid::new_v4().to_string()})).await.unwrap();
}

#[tokio::test]
async fn onboarding_shortcut_check_uses_the_native_hook_without_starting_capture() {
    let h = hub();
    command(&h, "onboarding.open", json!({"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    step(&h, "shortcut").await;
    h.toggle_from_hotkey().await.unwrap();
    assert!(h.snapshot("main").unwrap()["session"].is_null());
    assert_eq!(h.snapshot("main").unwrap()["onboarding"]["shortcutPresses"], 1);
}

#[tokio::test]
async fn onboarding_practice_freezes_preview_scene_without_changing_user_preferences() {
    let h = hub();
    let mut config = h.db.provider_config().unwrap();
    config.region = Some("beijing".into());
    h.db.run(DbCommand::SaveProviderConfig { config, expected_version:1 }).unwrap();
    h.inner.lock().unwrap().key_configured = true;
    let mut preferences=h.db.preferences().unwrap();preferences.app_writing.input_context_enabled=true;
    h.db.save_preferences(preferences,1).unwrap();
    let before = h.db.preferences().unwrap();
    command(&h, "onboarding.open", json!({"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    step(&h, "chat").await;
    // Stops before any credentials, audio device, cloud or native-input operation.
    let (id, generation, _, frozen, mode, _) = h.prepare_start_state(Instant::now(), Origin::UiCommand).unwrap();
    assert_eq!(frozen.delivery_mode, DeliveryMode::Preview);
    assert!(!frozen.app_writing.input_context_enabled);
    assert_eq!(mode.mode_id, "chat");
    assert_eq!(h.db.preferences().unwrap(), before);
    assert_eq!(view(&h).onboarding_lesson, Some(OnboardingLesson::Chat));
    assert!(h.begin_stop_target(&json!({"sessionId":id,"expectedGeneration":generation}),Instant::now()).unwrap().is_none());
    assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().target_capture_pending);
}

#[tokio::test]
async fn onboarding_first_run_can_pause_and_resume_without_claiming_completion() {
    let h = hub();
    assert_eq!(h.snapshot("main").unwrap()["onboarding"]["progress"]["autoShow"], true);
    let opened = command(&h, "onboarding.open", json!({"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    assert_eq!(opened["active"], true);
    let version = opened["progress"]["version"].as_u64().unwrap();
    command(&h, "onboarding.step", json!({"step":"setup","expectedVersion":version,"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    let version = h.snapshot("main").unwrap()["onboarding"]["progress"]["version"].as_u64().unwrap();
    command(&h, "onboarding.dismiss", json!({"expectedVersion":version,"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    let state = h.snapshot("main").unwrap();
    assert_eq!(state["onboarding"]["active"], false);
    assert_eq!(state["onboarding"]["progress"]["autoShow"], false);
    assert_eq!(state["onboarding"]["progress"]["step"], "setup");
    assert!(state["onboarding"]["progress"]["completedAt"].is_null());
    let resumed = command(&h, "onboarding.open", json!({"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    assert_eq!(resumed["progress"]["step"], "setup");
}

#[tokio::test]
async fn onboarding_completion_requires_the_current_owned_finished_practice() {
    let h = hub();
    command(&h, "onboarding.open", json!({"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    step(&h, "chat").await;
    let version = || h.db.snapshot().onboarding.version;
    let error = command(&h, "onboarding.finish", json!({"expectedVersion":version(),"requestId":Uuid::new_v4().to_string()})).await.unwrap_err();
    assert_eq!(error.code, "practice_required");
    install(&h,"unrelated",SessionPhase::Rewriting,true);
    ready(&h,"unrelated");
    let accept = |id:&str,generation| json!({"sessionId":id,"expectedGeneration":generation,"expectedVersion":version(),"requestId":Uuid::new_v4().to_string()});
    assert_eq!(command(&h,"onboarding.acceptLesson",accept("unrelated",7)).await.unwrap_err().code,"practice_required");
    {
        let mut i=h.inner.lock().unwrap();
        let s=i.active.as_mut().unwrap();s.onboarding_lesson=Some(OnboardingLesson::Chat);s.prefs.delivery_mode=DeliveryMode::Preview;
    }
    assert_eq!(command(&h,"onboarding.acceptLesson",accept("unrelated",6)).await.unwrap_err().code,"stale_session");
    command(&h,"onboarding.acceptLesson",accept("unrelated",7)).await.unwrap();
    assert_eq!(h.db.snapshot().onboarding.completed_lessons,vec![OnboardingLesson::Chat]);
    step(&h,"document").await;
    assert_eq!(command(&h,"onboarding.acceptLesson",accept("unrelated",7)).await.unwrap_err().code,"practice_required");
    install(&h,"document-practice",SessionPhase::Rewriting,true);
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.onboarding_lesson=Some(OnboardingLesson::Document);s.prefs.delivery_mode=DeliveryMode::Preview;}
    ready(&h,"document-practice");
    command(&h,"onboarding.acceptLesson",accept("document-practice",7)).await.unwrap();
    command(&h,"onboarding.finish",json!({"expectedVersion":version(),"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    assert!(h.db.snapshot().onboarding.completed_at.is_some());
    assert!(!h.inner.lock().unwrap().onboarding.active);
    let reopened=command(&h,"onboarding.open",json!({"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    assert_eq!(reopened["progress"]["step"],"welcome");
}

#[tokio::test]
async fn onboarding_hidden_guide_cannot_start_a_normal_insertion_and_busy_practice_cannot_be_dismissed() {
    let h=hub();
    command(&h,"onboarding.open",json!({"requestId":Uuid::new_v4().to_string()})).await.unwrap();
    step(&h,"chat").await;
    h.onboarding_focus_changed(false);
    assert_eq!(h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap_err().code,"onboarding_focus_required");
    assert!(h.snapshot("main").unwrap()["session"].is_null());
    h.onboarding_focus_changed(true);
    install(&h,"active-practice",SessionPhase::Recording,false);
    let recorder_calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));let calls=recorder_calls.clone();
    h.set_native_actions(Arc::new(move|name,enabled|{assert_eq!(name,"recording");assert!(enabled);calls.fetch_add(1,Ordering::SeqCst);Ok(())}));
    h.onboarding_main_hidden();assert_eq!(recorder_calls.load(Ordering::SeqCst),0);
    h.inner.lock().unwrap().active.as_mut().unwrap().onboarding_lesson=Some(OnboardingLesson::Chat);
    h.onboarding_main_hidden();assert_eq!(recorder_calls.load(Ordering::SeqCst),1);
    let error=h.select_session_mode_async(&json!({"sessionId":"active-practice","expectedGeneration":7,"modeId":"email"})).await.unwrap_err();
    assert_eq!(error.code,"practice_scene_frozen");
    assert_eq!(command(&h,"onboarding.dismiss",json!({"expectedVersion":h.db.snapshot().onboarding.version,"requestId":Uuid::new_v4().to_string()})).await.unwrap_err().code,"session_busy");
    assert!(h.inner.lock().unwrap().onboarding.active);
    let forbidden=h.command("recording",CommandRequest{name:"onboarding.finish".into(),payload:json!({"requestId":Uuid::new_v4().to_string()})}).await.unwrap_err();
    assert_eq!(forbidden.code,"forbidden");
}
