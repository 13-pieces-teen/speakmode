use super::*;
use crate::cloud::rewrite::minimum_pipeline::tests::{Server, Reply, rejection};

#[tokio::test]
async fn polishing_failure_or_timeout_keeps_the_draft_that_passed_content_review() {
    let original = "嗯，方案目前还没有太能做到。预算的话是3500元，先不要发送。";
    let draft = "方案目前还没有太能做到。预算3500元，先不要发送。";
    for failure in ["invalidRepair", "timeout", "invalidFinalReview", "changedMeaning"] {
        let h = hub();
        install(&h, "s", SessionPhase::FinalizingAsr, true);
        h.inner.lock().unwrap().active.as_mut().unwrap().config.region = Some("beijing".into());
        let mut frozen = source("s");
        frozen.items.truncate(1);
        frozen.items[0].text = original.into();
        h.checkpoint("s", 7, frozen.clone()).unwrap();
        let review = rejection("方案目前还没有太能做到。", "方案目前还没有太能做到。", "cleanup");
        let repair = match failure {
            "timeout" => Reply::new("{}").after(4000),
            "invalidFinalReview" => Reply::new(serde_json::json!({"replacements":[{"id":"r0","text":"方案目前还没有很好地做到。"}]}).to_string()),
            "changedMeaning" => Reply::new(serde_json::json!({"replacements":[{"id":"r0","text":"方案目前已经做好了。"}]}).to_string()),
            _ => Reply::new("invalid json"),
        };
        let server = Server::new(vec![Reply::draft(draft), Reply::new(review), repair, Reply::new("invalid final review")]).await;
        let endpoint = server.endpoint.clone();
        h.finish_rewrite_with_policy("s".into(), 7, move |config, mode, source, cancel, execution| async move {
            rewrite::rewrite_with_client_optional_context(&reqwest::Client::new(), &config, "synthetic-secret",
                &mode, &source, &DictionarySnapshot::default(), cancel, &endpoint, execution, None).await
        }, RewriteWaitPolicy { deadline: Duration::from_millis(1800) }, |_,_,_| {}).await;

        let detail = h.db.get_result("s").unwrap();
        let output = detail.output.unwrap();
        assert_eq!(detail.source, frozen);
        assert_eq!(output.text(), draft, "{failure}");
        assert_eq!(output.rewrite_recovery.unwrap().kind, RewriteRecoveryKind::ReviewedDraft);
        assert!(output.rewrite_fallback.is_none());
        assert_eq!(server.count(), if failure == "invalidFinalReview" {4} else {3});
        assert_eq!(view(&h).state, SessionPhase::Ready);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(), 1);
    }
}

#[tokio::test]
async fn uncertain_content_uses_basic_cleanup_and_cancelled_polishing_cannot_commit() {
    let original = "嗯，方案目前还没有太能做到。预算的话是3500元，先不要发送。";
    let draft = "方案目前还没有太能做到。预算3500元，先不要发送。";
    for cancel_after_review in [false, true] {
        let h = hub();
        install(&h, "s", SessionPhase::FinalizingAsr, true);
        h.inner.lock().unwrap().active.as_mut().unwrap().config.region = Some("beijing".into());
        let mut frozen = source("s");
        frozen.items.truncate(1);
        frozen.items[0].text = original.into();
        h.checkpoint("s", 7, frozen.clone()).unwrap();
        let mut review: serde_json::Value = serde_json::from_str(&rejection(
            "方案目前还没有太能做到。", "方案目前还没有太能做到。", "cleanup")).unwrap();
        if !cancel_after_review {
            review["checks"]["facts"] = serde_json::json!("uncertain");
            review["issues"] = serde_json::json!([]);
        }
        let server = Server::new(vec![Reply::draft(draft), Reply::new(review.to_string()), Reply::new("{}").after(4000)]).await;
        let endpoint = server.endpoint.clone();
        let worker = h.clone();
        let task = tokio::spawn(async move {
            worker.finish_rewrite_with_policy("s".into(), 7, move |config, mode, source, cancel, execution| async move {
                rewrite::rewrite_with_client_optional_context(&reqwest::Client::new(), &config, "synthetic-secret",
                    &mode, &source, &DictionarySnapshot::default(), cancel, &endpoint, execution, None).await
            }, RewriteWaitPolicy { deadline: Duration::from_secs(3) }, |_,_,_| {}).await;
        });
        if cancel_after_review {
            wait_until(||server.count() == 3).await;
            h.begin_cancel(&identity("s")).unwrap();
        }
        task.await.unwrap();
        if cancel_after_review {
            assert_eq!(h.db.query_count("SELECT COUNT(*) FROM outputs").unwrap(), 0);
        } else {
            let output = h.db.get_result("s").unwrap().output.unwrap();
            assert_eq!(output.text(), "方案目前还没有太能做到。预算的话是3500元，先不要发送。");
            assert_eq!(output.rewrite_recovery.unwrap().kind, RewriteRecoveryKind::BasicCleanup);
            assert_eq!(server.count(), 2);
        }
    }
}
