use super::*;
use crate::cloud::rewrite::{ManualCandidate,ManualReviewIssue};

#[test]
fn full_pipeline_budget_has_two_nominal_windows_and_keeps_one_shot_policy(){
    for (chars,one_shot,full) in [(0,20_000,40_000),(800,20_000,40_000),(801,30_000,60_000),
        (3000,30_000,60_000),(3001,60_000,120_000)] {
        let(_,_,source)=fixture(&"字".repeat(chars));
        assert_eq!(super::super::super::initial_wait_ms(&source),one_shot);
        assert_eq!(super::super::super::full_pipeline_wait_ms(&source),full);
    }
}

#[tokio::test]
async fn admitted_draft_is_observed_before_review_finishes_and_cancellation_stops_updates(){
    let input="请检查日志。";
    let server=Server::new(vec![Reply::draft(input),Reply::new(ACCEPT).after(2000)]).await;
    let(config,mode,source)=fixture(input);let endpoint=server.endpoint.clone();
    let observed=Arc::new(Mutex::new(Vec::<ManualCandidate>::new()));let seen=observed.clone();
    let execution=RewriteExecution::for_duration(Duration::from_secs(4))
        .with_reviewable_observer(move|candidate|seen.lock().unwrap().push(candidate));
    let cancel=CancellationToken::new();let worker_cancel=cancel.clone();
    let job=tokio::spawn(async move{execute_until(&client(),&config,"synthetic-key",&mode,&source,
        &DictionarySnapshot::default(),worker_cancel,&endpoint,execution,None).await});
    server.wait_for(2).await;
    {
        let candidates=observed.lock().unwrap();assert_eq!(candidates.len(),1);
        assert_eq!(candidates[0].draft.blocks[0].text,input);
        assert_eq!(candidates[0].review_issues,vec![ManualReviewIssue::ReviewUnavailable]);
    }
    cancel.cancel();let outcome=job.await.unwrap();
    assert_eq!(outcome.result.err().unwrap().code,"cancelled");
    assert_eq!(observed.lock().unwrap().len(),1);assert_eq!(server.count(),2);
}

#[tokio::test]
async fn outer_timeout_retains_only_an_admitted_draft_and_explicit_rejection_is_annotated(){
    let input="请检查日志。";
    let server=Server::new(vec![Reply::draft(input),Reply::new(ACCEPT).after(2000)]).await;
    let(config,mode,source)=fixture(input);
    let observed=Arc::new(Mutex::new(Vec::<ManualCandidate>::new()));let seen=observed.clone();
    let execution=RewriteExecution::for_duration(Duration::from_secs(4))
        .with_reviewable_observer(move|candidate|seen.lock().unwrap().push(candidate));
    let http=client();let dictionary=DictionarySnapshot::default();
    let mut job=Box::pin(execute_until(&http,&config,"synthetic-key",&mode,&source,
        &dictionary,CancellationToken::new(),&server.endpoint,execution,None));
    tokio::select!{outcome=&mut job=>panic!("review must remain pending: {}",outcome.trace.calls),_=server.wait_for(2)=>{}}
    assert!(tokio::time::timeout(Duration::from_millis(10),job).await.is_err());
    let candidates=observed.lock().unwrap();assert_eq!(candidates.len(),1);
    assert_eq!(candidates[0].draft.blocks[0].source_refs,vec![source.items[0].id.clone()]);
    assert_eq!(candidates[0].draft.blocks[0].text,input);drop(candidates);

    let mut uncertain:Value=serde_json::from_str(ACCEPT).unwrap();
    uncertain["decision"]=json!("reject");uncertain["checks"]["coverage"]=json!("uncertain");
    let server=Server::new(vec![Reply::draft(input),Reply::new(uncertain.to_string())]).await;
    let seen=observed.clone();let execution=RewriteExecution::for_duration(Duration::from_secs(4))
        .with_reviewable_observer(move|candidate|seen.lock().unwrap().push(candidate));
    let out=execute_until(&client(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),
        CancellationToken::new(),&server.endpoint,execution,None).await;
    assert_eq!(out.result.err().unwrap().code,"rewrite_semantic_rejected");
    let candidates=observed.lock().unwrap();assert_eq!(candidates.len(),3);
    assert_eq!(candidates[2].review_issues,vec![ManualReviewIssue::ReviewRejected]);assert_eq!(server.count(),2);
}

#[tokio::test]
async fn newest_admitted_correction_survives_final_transport_failure_and_bad_corrections_are_not_published(){
    let input="请检查日志，然后阅读使用说明。";let bad="请检查日志。";
    let server=Server::new(vec![Reply::draft(bad),Reply::new(rejection("阅读使用说明",bad,"coverage")),
        Reply::draft(input),Reply::new("private failure").http_status(503)]).await;
    let(config,mode,source)=fixture(input);let observed=Arc::new(Mutex::new(Vec::<ManualCandidate>::new()));let seen=observed.clone();
    let execution=RewriteExecution::for_duration(Duration::from_secs(4))
        .with_reviewable_observer(move|candidate|seen.lock().unwrap().push(candidate));
    let out=execute_until(&client(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),
        CancellationToken::new(),&server.endpoint,execution,None).await;
    assert_eq!(out.result.err().unwrap().code,"rewrite_service_error");assert_eq!(server.count(),4);
    let candidates=observed.lock().unwrap();assert_eq!(candidates.len(),3);
    assert_eq!(candidates[1].review_issues,vec![ManualReviewIssue::ReviewRejected]);
    assert_eq!(candidates[2].draft.blocks[0].text,input);
    assert_eq!(candidates[2].review_issues,vec![ManualReviewIssue::ReviewUnavailable]);drop(candidates);

    let input="嗯，预算3500元，如果测试通过再上线。";
    let server=Server::new(vec![Reply::draft("预算3500元，直接上线。"),Reply::draft("预算5000元，如果测试通过再上线。")]).await;
    let(config,mode,source)=fixture(input);let observed=Arc::new(Mutex::new(Vec::<ManualCandidate>::new()));let seen=observed.clone();
    let execution=RewriteExecution::for_duration(Duration::from_secs(4))
        .with_reviewable_observer(move|candidate|seen.lock().unwrap().push(candidate));
    let out=execute_until(&client(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),
        CancellationToken::new(),&server.endpoint,execution,None).await;
    assert!(out.result.is_err());assert!(observed.lock().unwrap().is_empty());assert_eq!(server.count(),2);
}

#[tokio::test]
async fn correction_and_final_review_keep_transient_request_errors_but_invalid_content_keeps_original_rejection(){
    let input="嗯，预算3500元，如果测试通过再上线。";
    let bad="预算3500元，直接上线。";let corrected="预算3500元，如果测试通过再上线。";
    for final_review in [false,true] {
        for (terminal,expected) in [(Reply::disconnected(),"rewrite_network"),
            (Reply::new("private failure").http_status(503),"rewrite_service_error")] {
            let mut replies=vec![Reply::draft(bad)];if final_review{replies.push(Reply::draft(corrected));}replies.push(terminal);
            let server=Server::new(replies).await;let out=test_run(&server,input,Duration::from_secs(4)).await;
            assert_eq!(out.result.err().unwrap().code,expected,"final_review={final_review}");
            assert_eq!(server.count(),if final_review{3}else{2});assert!(out.trace.correction_attempted);
        }
    }
    let server=Server::new(vec![Reply::draft(bad),Reply::draft(corrected),Reply::new("{}")]).await;
    let out=test_run(&server,input,Duration::from_secs(4)).await;
    assert_eq!(out.result.err().unwrap().code,"invalid_output");assert_eq!(server.count(),3);
    let server=Server::new(vec![Reply::draft(bad),Reply::draft("预算5000元，如果测试通过再上线。")]).await;
    let out=test_run(&server,input,Duration::from_secs(4)).await;
    assert_eq!(out.result.err().unwrap().code,"rewrite_admission_rejected");assert_eq!(server.count(),2);
}
