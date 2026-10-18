use super::*;

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
