use super::*;

#[tokio::test]
async fn long_dictation_can_correct_missing_or_rewritten_literal_markers_before_full_review() {
    let input=format!("请保留 `userID` 和 SVID。{}","描述当前流程。".repeat(100));
    for bad in ["请保留接口。","请保留 userId 和 SVID。"] {
        let server=Server::new(vec![Reply::draft(bad),Reply::new("__ECHO_GUARDED_SOURCE__"),Reply::new(ACCEPT)]).await;
        let out=test_run(&server,&input,Duration::from_secs(4)).await;
        assert_eq!(out.result.unwrap().blocks[0].text,input);
        assert!(out.trace.draft.is_none());assert!(out.trace.correction_attempted);
        assert_eq!(server.count(),3);assert!(out.trace.final_review.unwrap().accepted());
        let requests=server.requests.lock().unwrap();
        let original:Value=serde_json::from_str(requests[0]["messages"][1]["content"].as_str().unwrap()).unwrap();
        let correction:Value=serde_json::from_str(requests[1]["messages"][1]["content"].as_str().unwrap()).unwrap();
        let review:Value=serde_json::from_str(requests[2]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(original["sourceText"],correction["sourceText"]);
        assert_eq!(review["draftText"],input);assert_eq!(review["sourceText"],input);
    }
    let server=Server::new(vec![Reply::draft("请保留接口。"),Reply::draft("请保留未知接口。"),Reply::new(ACCEPT)]).await;
    let out=test_run(&server,&input,Duration::from_secs(4)).await;
    assert_eq!(out.result.err().unwrap().code,"protected_terms_missing");assert_eq!(server.count(),2);
    assert!(out.trace.final_review.is_none());
}

#[tokio::test]
async fn correction_restores_a_dropped_condition_then_requires_new_full_review() {
    let input="嗯，预算3500元，如果测试通过再上线。";
    let bad="预算3500元，直接上线。";
    let corrected="预算3500元，如果测试通过再上线。";
    let server=Server::new(vec![Reply::draft(bad),Reply::draft(corrected),Reply::new(ACCEPT)]).await;
    let out=test_run(&server,input,Duration::from_secs(4)).await;
    assert_eq!(out.result.unwrap().blocks[0].text,corrected);
    assert!(out.trace.correction_attempted);assert_eq!(out.trace.calls,3);assert_eq!(server.count(),3);
    assert!(out.trace.review.is_none());assert!(out.trace.final_review.unwrap().accepted());
    let requests=server.requests.lock().unwrap();
    let before:Value=serde_json::from_str(requests[0]["messages"][1]["content"].as_str().unwrap()).unwrap();
    let repair:Value=serde_json::from_str(requests[1]["messages"][1]["content"].as_str().unwrap()).unwrap();
    let final_review:Value=serde_json::from_str(requests[2]["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(before["sourceText"],repair["sourceText"]);assert_eq!(repair["rejectedDraft"]["text"],bad);
    assert_eq!(final_review["sourceText"],input);assert_eq!(final_review["draftText"],corrected);
}

#[tokio::test]
async fn coverage_failure_can_restore_an_omitted_action_within_four_calls() {
    let input="请检查日志，然后阅读使用说明。";
    let bad="请检查日志。";
    let corrected="请检查日志，然后阅读使用说明。";
    let server=Server::new(vec![Reply::draft(bad),Reply::new(rejection("阅读使用说明",bad,"coverage")),
        Reply::draft(corrected),Reply::new(ACCEPT)]).await;
    let out=test_run(&server,input,Duration::from_secs(4)).await;
    assert_eq!(out.result.unwrap().blocks[0].text,corrected);
    assert_eq!(out.trace.calls,4);assert_eq!(server.count(),4);assert!(out.trace.correction_attempted);
    assert!(!out.trace.review.unwrap().accepted());assert!(out.trace.final_review.unwrap().accepted());
}

#[tokio::test]
async fn invalid_correction_or_final_verdict_never_becomes_an_output_or_another_retry() {
    let input="嗯，预算3500元，如果测试通过再上线。";
    let bad="预算3500元，直接上线。";
    for (correction,verdict,count) in [
        ("预算5000元，如果测试通过再上线。",ACCEPT.to_owned(),2),
        (bad,ACCEPT.to_owned(),2),
        ("预算3500元，如果测试通过再上线。",rejection(input,"预算3500元，如果测试通过再上线。","coverage"),3),
        ("预算3500元，如果测试通过再上线。","{}".into(),3),
    ] {
        let server=Server::new(vec![Reply::draft(bad),Reply::draft(correction),Reply::new(verdict),Reply::draft(input)]).await;
        let out=test_run(&server,input,Duration::from_secs(4)).await;
        assert!(out.result.is_err());assert_eq!(server.count(),count);assert!(out.trace.correction_attempted);
    }
}

#[tokio::test]
async fn uncertain_review_and_small_remaining_budget_do_not_speculatively_regenerate() {
    let input="请检查日志。";
    let mut uncertain:Value=serde_json::from_str(ACCEPT).unwrap();
    uncertain["decision"]=json!("reject");uncertain["checks"]["coverage"]=json!("uncertain");
    for (verdict,wait) in [(uncertain.to_string(),Duration::from_secs(4)),(rejection(input,input,"coverage"),Duration::from_secs(1))] {
        let server=Server::new(vec![Reply::draft(input),Reply::new(verdict)]).await;
        let out=test_run(&server,input,wait).await;
        assert!(out.result.is_err());assert_eq!(server.count(),2);assert!(!out.trace.correction_attempted);
    }
}

#[tokio::test]
async fn cancellation_and_the_original_deadline_cover_corrective_generation_and_final_review() {
    let input="嗯，预算3500元，如果测试通过再上线。";
    let bad="预算3500元，直接上线。";
    let corrected="预算3500元，如果测试通过再上线。";
    for stop_after in [2,3] {
        let server=Server::new(vec![Reply::draft(bad),Reply::draft(corrected).after(if stop_after==2{2500}else{0}),
            Reply::new(ACCEPT).after(2500)]).await;
        let(config,mode,source)=fixture(input);let endpoint=server.endpoint.clone();
        let cancel=CancellationToken::new();let worker_cancel=cancel.clone();
        let job=tokio::spawn(async move{execute(&client(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),worker_cancel,&endpoint,Duration::from_secs(4)).await});
        server.wait_for(stop_after).await;cancel.cancel();
        let out=job.await.unwrap();assert_eq!(out.result.err().unwrap().code,"cancelled");assert_eq!(server.count(),stop_after);
    }
    let server=Server::new(vec![Reply::draft(bad),Reply::draft(corrected).after(1900),Reply::new(ACCEPT).after(400)]).await;
    let started=Instant::now();let out=test_run(&server,input,Duration::from_millis(2100)).await;
    assert_eq!(out.result.err().unwrap().code,"rewrite_timeout");assert!(started.elapsed()<Duration::from_millis(2800));
    assert!(out.trace.correction_attempted);assert!(server.count()<=3);
}

#[test]
fn correction_keeps_masks_source_mode_and_context_bound_and_cannot_be_repeated() {
    let input=format!("请保留 `userID` 和 SVID。{}","检查流程。".repeat(100));
    let(config,mut mode,source)=fixture(&input);mode.mode_id="document".into();
    let context=super::super::super::writing_context::for_mode(&mode).unwrap();
    let mut request=minimum::prepare_with_writing_context(&config,&mode,&source,&DictionarySnapshot::default(),true,Some(&context)).unwrap();
    let before:Value=serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap()).unwrap();
    let wire=json!({"resolvedMode":"light","text":before["sourceText"]}).to_string();
    let feedback=json!({"stage":"review","checks":{"coverage":"fail"}});
    let mut changed=context.clone();changed.language=crate::core::DraftLanguage::English;
    assert_eq!(request.prepare_correction(&wire,feedback.clone(),Some(&changed)).err().unwrap().code,"stale_rewrite");
    request.prepare_correction(&wire,feedback.clone(),Some(&context)).unwrap();
    let after:Value=serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap()).unwrap();
    for (key,value) in before.as_object().unwrap(){assert_eq!(after[key],*value,"{key}");}
    assert_eq!(minimum::parse(&wire,&mode,&source,&request).unwrap().blocks[0].text,input);
    let mut changed_source=source.clone();changed_source.generation+=1;
    assert!(minimum::parse(&wire,&mode,&changed_source,&request).is_err());
    let mut changed_mode=mode.clone();changed_mode.revision+=1;
    assert!(minimum::parse(&wire,&changed_mode,&source,&request).is_err());
    assert!(request.prepare_correction(&wire,feedback,Some(&context)).is_err());
}
