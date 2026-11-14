// Actual no-host context selection and scripted loopback transport. These are
// route/cost regressions, not model quality or real recording measurements.
#[tokio::test]
async fn comparison_context_matches_normal_profile_off_route_for_every_no_host_setting() {
    let h=hub();let mut count=0;
    let modes=[("auto",BaseMode::Auto,""),("light",BaseMode::Light,""),("structure",BaseMode::Structure,""),
        ("personal-custom",BaseMode::Light,"保持原顺序"),("auto",BaseMode::Auto,"使用自定义排版"),
        ("chat",BaseMode::Auto,""),("document",BaseMode::Auto,""),("email",BaseMode::Auto,""),("technical",BaseMode::Auto,"")];
    for (mode_id,base_mode,rules) in modes {
        for language in [DraftLanguage::AsSpoken,DraftLanguage::English] {
            for input_context_enabled in [false,true] {
                for app_writing_enabled in [false,true] {
                    for profile_state in 0..3 {
                        let mut personal=config();
                        if profile_state==0 {personal.enabled=false;}
                        if profile_state==1 {personal=PersonalizationConfig{enabled:true,..Default::default()};}
                        let preferences=AppWritingPreferences{enabled:app_writing_enabled,input_context_enabled,output_language:language,..Default::default()};
                        let mode=ModeSnapshot{mode_id:mode_id.into(),name:"fixture".into(),base_mode,rules:rules.into(),revision:1};
                        install(&h,"routing-matrix",SessionPhase::FinalizingAsr,true);
                        let normal={let mut inner=h.inner.lock().unwrap();let session=inner.active.as_mut().unwrap();
                            session.mode=mode.clone();session.prefs.app_writing=preferences.clone();session.personalization=personal.clone();
                            // No target or host text exists in this comparison.
                            app_writing::freeze(session)};
                        let compared=personalization::context_for_preferences(&mode,&preferences,&personal);
                        assert_eq!(compared,normal,"{mode_id} {rules:?} {language:?} input={input_context_enabled} app={app_writing_enabled} profile={profile_state}");
                        assert!(compared.as_ref().is_none_or(|context|context.input.is_none()));
                        let mut profile_off=personal.clone();profile_off.enabled=false;
                        let baseline=personalization::context_for_preferences(&mode,&preferences,&profile_off);
                        assert!(baseline.as_ref().is_none_or(|context|context.personalization.is_none()));
                        let explicit=["chat","document","email","technical"].contains(&mode_id);
                        let automatic=mode_id=="auto"&&rules.is_empty();
                        assert_eq!(baseline.is_some(),explicit||automatic||input_context_enabled||language==DraftLanguage::English);
                        count+=1;
                    }
                }
            }
        }
    }
    assert_eq!(count,216);h.db.shutdown().await.unwrap();
}

#[tokio::test]
async fn custom_comparison_and_normal_dictation_share_full_review_with_optional_context() {
    use crate::cloud::rewrite::minimum_pipeline::tests::{Server,Reply};
    const ROUTING_ACCEPT:&str=r#"{"decision":"accept","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"pass"},"issues":[]}"#;
    let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let config=ProviderConfig{rewrite_provider:"custom".into(),rewrite_model:"scripted-local-model".into(),..Default::default()};
    let input=source("routing-local");let original=input.clone();let dictionary=DictionarySnapshot::default();
    let personal=PersonalizationConfig{enabled:true,preferences:ExpressionStyle{paragraphs:true,..Default::default()},..Default::default()};
    let mut off=personal.clone();off.enabled=false;
    for mode_id in ["light","personal-custom"] {
        let mode=ModeSnapshot{mode_id:mode_id.into(),name:"fixture".into(),base_mode:BaseMode::Light,rules:if mode_id=="light"{""}else{"保持原顺序"}.into(),revision:1};
        for input_context_enabled in [false,true] {
            let prefs=AppWritingPreferences{input_context_enabled,..Default::default()};
            let baseline=personalization::context_for_preferences(&mode,&prefs,&off);
            let personalized=personalization::context_for_preferences(&mode,&prefs,&personal).unwrap();
            assert_eq!(baseline.is_some(),input_context_enabled);
            for context in [baseline,Some(personalized)] {
                let personalized=context.as_ref().is_some_and(|context|context.personalization.is_some());
                let response=json!({"resolvedMode":"light","text":input.text()});
                let replies=vec![Reply::new(response.to_string()),Reply::new(ROUTING_ACCEPT)];
                let server=Server::new(replies).await;let dispatch=personalization::DispatchTracker::default();
                let execution=dispatch.observe(rewrite::execution::RewriteExecution::for_duration(std::time::Duration::from_secs(3)),context.as_ref());
                let result=rewrite::rewrite_with_client_optional_context(&client,&config,"synthetic",&mode,&input,&dictionary,
                    CancellationToken::new(),&server.endpoint,execution,context.as_ref()).await.unwrap();
                assert_eq!(server.count(),2,"{mode_id} input={input_context_enabled} profile={personalized}");
                assert_eq!(result.blocks.iter().map(|block|block.text.as_str()).collect::<Vec<_>>().join("\n"),input.text());
                assert_eq!(result.blocks.iter().flat_map(|block|block.source_refs.clone()).collect::<Vec<_>>(),input.items.iter().map(|item|item.id.clone()).collect::<Vec<_>>());
                assert_eq!(input,original);
                let event=personalization::task_event("routing-profile-check",PersonalizationEventKind::Comparison,context.as_ref(),PersonalizationEventStatus::Started,0);
                let complete=dispatch.finished(&event,PersonalizationEventStatus::Success,1);
                assert_eq!(complete.reference_dispatch,Some(if personalized{ReferenceDispatch::Started}else{ReferenceDispatch::NotStarted}));
            }
        }
    }
}
