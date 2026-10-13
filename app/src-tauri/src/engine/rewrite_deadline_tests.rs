//! Coordinator -> production dispatcher -> real loopback HTTP -> SQLite.
//! The baseline arm injects the former cloud-only 20s clock, changing nothing else.
use super::*;
use crate::cloud::rewrite::minimum_pipeline::tests::{Server,Reply,rejection};
use crate::cloud::rewrite::execution::RequestOutcome;
use serde_json::json;
use sha2::{Digest,Sha256};

async fn paired_deadline_regression(chars:usize,budget_secs:u64) {
    let lead="我感觉这个方案还没有太能做到。预算3500元，先不要发送。";
    let text=format!("{lead}{}", "描述当前流程。".repeat(chars/7));
    let text:String=text.chars().take(chars).collect();
    for legacy_clock in [true,false] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);
        h.inner.lock().unwrap().active.as_mut().unwrap().config.region=Some("beijing".into());
        let mut frozen=source("s");frozen.items.truncate(1);frozen.items[0].text=text.clone();
        h.checkpoint("s",7,frozen.clone()).unwrap();install_trace(&h);
        let server=Server::new(vec![Reply::new("__ECHO_GUARDED_SOURCE__").after(100),
            Reply::new(rejection("我感觉这个方案还没有太能做到。","我感觉这个方案还没有太能做到。","cleanup")).after(budget_secs*1000-1200),
            Reply::new("{}").after(5000)]).await;
        let endpoint=server.endpoint.clone();
        let started=std::time::Instant::now();
        h.finish_rewrite_with_policy("s".into(),7,move|config,mode,source,cancel,mut execution|async move {
            // This is the former bug, scoped only to the paired regression arm.
            if legacy_clock {execution.deadline=tokio::time::Instant::now()+Duration::from_secs(20);}
            rewrite::rewrite_with_client_execution(&reqwest::Client::builder().no_proxy().build().unwrap(),
                &config,"synthetic-test-key",&mode,&source,&DictionarySnapshot::default(),cancel,&endpoint,execution).await
        },None::<RewriteWaitPolicy>,|hub,_,_|{
            hub.inner.lock().unwrap().active.as_mut().unwrap().delivery_worker_released=true;
        }).await;
        let elapsed_ms=started.elapsed().as_secs_f64()*1000.0;
        let saved=h.db.get_result("s").unwrap().output.unwrap();
        let snapshot=h.diagnostics.snapshot().unwrap();let trace=&snapshot.traces[0];let rewrite=trace.rewrite.as_ref().unwrap();
        assert_eq!(rewrite.configured_budget_ms,budget_secs*1000);
        assert_eq!(saved.text(),text);assert!(saved.rewrite_fallback.is_some());
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),1);
        assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled());
        let expected=if legacy_clock && budget_secs<20 {3}else{2};
        assert_eq!(server.count(),expected,"chars={chars}, legacy={legacy_clock}");
        assert_eq!(rewrite.requests.len(),expected);
        assert!(rewrite.requests.iter().all(|r|r.end_ms.is_some()&&r.duration_ms.is_some()&&r.outcome.is_some()));
        if legacy_clock && budget_secs<20 {
            assert_eq!(saved.rewrite_fallback,Some(RewriteFallbackReason::Timeout));
            // The mismatched legacy inner clock cannot see the outer cutoff:
            // its dropped in-flight request is truthfully "interrupted".
            assert_eq!(rewrite.requests.last().unwrap().outcome,Some(RequestOutcome::Interrupted));
        }
        let receipt=json!({"scope":"coordinator_loopback_http_sqlite","legacyClock":legacy_clock,
            "chars":chars,"configuredBudgetMs":budget_secs*1000,"elapsedMs":elapsed_ms,
            "receivedRequests":server.count(),"inputSha256":format!("{:x}",Sha256::digest(text.as_bytes())),
            "sourceUnchanged":saved.text()==text,"outputs":1,"rewrite":rewrite});
        std::fs::write(h.data_dir.join("deadline-regression.json"),serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn coordinator_http_deadline_short_5s(){paired_deadline_regression(80,5).await;}
#[tokio::test]
async fn coordinator_http_deadline_medium_8s(){paired_deadline_regression(401,8).await;}
#[tokio::test]
async fn coordinator_http_deadline_long_20s(){paired_deadline_regression(801,20).await;}

#[tokio::test]
async fn expired_shared_http_request_closes_timing_and_commits_only_source(){
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
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),source("s").text());
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(),1);
}
