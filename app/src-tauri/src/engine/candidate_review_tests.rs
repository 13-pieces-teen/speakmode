use super::*;

#[tokio::test]
async fn reviewed_text_is_saved_exactly_with_frozen_source_and_can_be_undone_without_delivery() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");candidate(&h,"s","reviewed");
    let before=h.db.get_result("s").unwrap();let edited="🙂设备预算：三千五百元。\n\n若会议没确认，先不要订票。";
    let refs=before.source.items.iter().map(|i|i.id.clone()).collect::<Vec<_>>();
    command(&h,"revision.adopt",json!({"sessionId":"s","expectedCurrentRevisionId":"output-s","candidateId":"reviewed","reviewedText":edited,"blocks":[{"id":"forged","text":"ignored","sourceRefs":["foreign"]}],"requestId":"reviewed-adopt"})).await.unwrap();
    let after=h.db.get_result("s").unwrap();assert_eq!(after.source,before.source);
    let output=after.output.unwrap();assert_eq!(output.blocks[0].text,edited);assert_eq!(output.blocks[0].source_refs,refs);
    assert_eq!(usage_count(&h),1);assert!(after.can_undo);
    command(&h,"revision.undo",json!({"sessionId":"s","expectedCurrentRevisionId":output.id,"requestId":"reviewed-undo"})).await.unwrap();
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),before.output.unwrap().text());
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}

#[tokio::test]
async fn invalid_edits_leave_original_and_candidate_available_for_correction() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");candidate(&h,"s","reviewed");
    for (i,bad) in [json!(null),json!(123),json!("  "),json!("\u{0}"),json!("大".repeat(80_001))].into_iter().enumerate() {
        assert!(command(&h,"revision.adopt",json!({"sessionId":"s","expectedCurrentRevisionId":"output-s","candidateId":"reviewed","reviewedText":bad,"requestId":format!("invalid-edit-{i}")})).await.is_err());
        assert_eq!(h.db.get_result("s").unwrap().output.unwrap().id,"output-s");
        assert!(h.inner.lock().unwrap().candidate.as_ref().unwrap().adopting.is_none());
    }
}

#[tokio::test]
async fn stale_or_cancelled_candidate_cannot_commit_reviewed_text() {
    for cancelled in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");let token=candidate(&h,"s","reviewed");
        if cancelled {token.cancel();} else {
            command(&h,"revision.edit",json!({"sessionId":"s","expectedCurrentRevisionId":"output-s","blocks":[{"id":"new","text":"后来的手动版本。","sourceRefs":[]}],"requestId":"newer-edit"})).await.unwrap();
        }
        let before=h.db.get_result("s").unwrap();
        assert!(command(&h,"revision.adopt",json!({"sessionId":"s","expectedCurrentRevisionId":"output-s","candidateId":"reviewed","reviewedText":"旧候选修改。","requestId":"late-reviewed-adopt"})).await.is_err());
        assert_eq!(h.db.get_result("s").unwrap().output,before.output);assert_eq!(usage_count(&h),1);
    }
}

#[tokio::test]
async fn flagged_candidate_replay_retains_findings_and_explicit_review_is_required() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");candidate(&h,"s","flagged");
    h.inner.lock().unwrap().candidate.as_mut().unwrap().result.as_mut().unwrap().review_issues=vec![rewrite::ManualReviewIssue::ValuesChanged];
    let replay=h.replay(ReplayValue::Candidate("s".into(),"flagged".into())).await.unwrap();
    assert_eq!(replay["id"],"flagged");assert_eq!(replay["reviewIssues"],json!(["valuesChanged"]));assert!(replay["blocks"].is_array());
    let error=command(&h,"revision.adopt",json!({"sessionId":"s","expectedCurrentRevisionId":"output-s","candidateId":"flagged","requestId":"unreviewed"})).await.unwrap_err();
    assert_eq!(error.code,"candidate_review_required");
    command(&h,"revision.adopt",json!({"sessionId":"s","expectedCurrentRevisionId":"output-s","candidateId":"flagged","reviewedText":"核对后的数字与正文。","requestId":"explicit-review"})).await.unwrap();
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),"核对后的数字与正文。");
}

#[tokio::test]
async fn first_source_only_record_can_adopt_human_edit_once_without_automatic_input() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");source_only_candidate(&h,"s","first");
    command(&h,"revision.adopt",json!({"sessionId":"s","expectedCurrentRevisionId":"","candidateId":"first","reviewedText":"首次核对与修改。","requestId":"first-review"})).await.unwrap();
    assert_eq!(h.db.get_result("s").unwrap().output.unwrap().text(),"首次核对与修改。");assert_eq!(usage_count(&h),1);
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}
