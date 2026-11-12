//! Coordinator -> production dispatcher -> real loopback HTTP -> SQLite.
//! The baseline arm injects the former cloud-only 20s clock, changing nothing else.
use super::*;
use crate::cloud::rewrite::minimum_pipeline::tests::{Server,Reply};
use crate::cloud::rewrite::execution::RequestOutcome;

// Exercise an actual slow HTTP response under the coordinator's production
// budget. Budget boundaries for all lengths are checked separately.
#[tokio::test]
async fn coordinator_allows_final_review_beyond_former_five_second_cutoff() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);install_trace(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().config.region=Some("beijing".into());
    let mut frozen=source("s");frozen.items.truncate(1);frozen.items[0].text="嗯，预算3500元，先不要发送。".into();h.checkpoint("s",7,frozen).unwrap();
    let accept=r#"{"decision":"accept","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"pass"},"issues":[]}"#;
    let server=Server::new(vec![Reply::draft("预算3500元，先不要发送。"),Reply::new(accept).after(5200)]).await;
    let endpoint=server.endpoint.clone();
    h.finish_rewrite_with_policy("s".into(),7,move|config,mode,source,cancel,execution|async move {
        rewrite::rewrite_with_client_execution(&reqwest::Client::builder().no_proxy().build().unwrap(),
            &config,"synthetic",&mode,&source,&DictionarySnapshot::default(),cancel,&endpoint,execution).await
    },None::<RewriteWaitPolicy>,|_,_,_|{}).await;
    let output=h.db.get_result("s").unwrap().output.unwrap();
    assert_eq!(output.text(),"预算3500元，先不要发送。");assert!(output.rewrite_recovery.is_none());
    assert_eq!(server.count(),2);assert_eq!(usage_count(&h),1);
    let snapshot=h.diagnostics.snapshot().unwrap();let rewrite=snapshot.traces[0].rewrite.as_ref().unwrap();
    assert_eq!(rewrite.configured_budget_ms,20000);assert!(rewrite.automatic_recovery.is_none());
}

#[tokio::test]
async fn expired_shared_http_request_closes_timing_without_committing_intermediate(){
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");install_trace(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().config.region=Some("beijing".into());
    let server=Server::new(vec![Reply::new("{}").after(3000)]).await;let endpoint=server.endpoint.clone();
    h.finish_rewrite_with_policy("s".into(),7,move|config,mode,source,cancel,execution|async move{
        rewrite::rewrite_with_client_execution(&reqwest::Client::builder().no_proxy().build().unwrap(),
            &config,"synthetic",&mode,&source,&DictionarySnapshot::default(),cancel,&endpoint,execution).await
    },RewriteWaitPolicy{deadline:Duration::from_millis(300)},|h,_,_|h.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true).await;
    assert_eq!(server.count(),1);let snapshot=h.diagnostics.snapshot().unwrap();
    let requests=&snapshot.traces[0].rewrite.as_ref().unwrap().requests;
    assert_eq!(requests.len(),1);assert_eq!(requests[0].outcome,Some(RequestOutcome::TimedOut));assert!(requests[0].duration_ms.is_some());
    super::rewrite_fallback_tests::assert_unfinished(&h);
}
