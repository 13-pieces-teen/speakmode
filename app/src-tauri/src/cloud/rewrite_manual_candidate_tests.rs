use super::*;
use crate::core::SourceItem;
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::TcpListener};

fn fixture(text:&str)->(ProviderConfig,ModeSnapshot,SourceRevision) {
    let mut config=ProviderConfig::default();config.region=Some("beijing".into());
    (config,ModeSnapshot{mode_id:"light".into(),name:"轻整理".into(),base_mode:BaseMode::Light,rules:String::new(),revision:1},
     SourceRevision{id:"source".into(),session_id:"session".into(),asr_attempt_id:"attempt".into(),generation:1,
      provider:"synthetic".into(),model:"fixed".into(),adapter_version:"test".into(),complete:true,created_at:"now".into(),
      items:vec![SourceItem{id:"item-0".into(),provider_item_id:"0".into(),capture_order:0,text:text.into(),is_final:true,start_ms:None,end_ms:None}]})
}
fn content(text:&str)->String {json!({"resolvedMode":"light","blocks":[{"text":text,"sourceRange":[0,0]}]}).to_string()}

#[test]
fn numeric_rejection_is_reviewable_without_relaxing_automatic_release() {
    let (config,mode,source)=fixture("设备预算是三千五百元。");
    let draft=content("设备预算是五千元。");
    assert!(parse_model_output(&draft,&mode,&source).is_err());
    let candidate=decode(&config,&mode,&source,&DictionarySnapshot::default(),&draft).unwrap();
    assert_eq!(candidate.draft.blocks[0].text,"设备预算是五千元。");
    assert_eq!(candidate.draft.blocks[0].source_refs,vec!["item-0"]);
    assert!(candidate.review_issues.contains(&ManualReviewIssue::ValuesChanged));
    assert_eq!(source.text(),"设备预算是三千五百元。");
}

#[test]
fn removed_negation_and_literal_have_review_findings_not_fabricated_fidelity() {
    for (before,after,expected) in [
        ("如果会议没确认，先不要发邀请。","如果会议没确认，先发邀请。",ManualReviewIssue::MeaningChanged),
        ("请逐字保留‘嗯，这个，等我想一下’。","请逐字保留‘等我想一下’。",ManualReviewIssue::LiteralsChanged),
    ] {
        let (config,mode,source)=fixture(before);let draft=content(after);
        assert!(parse_model_output(&draft,&mode,&source).is_err());
        let result=decode(&config,&mode,&source,&DictionarySnapshot::default(),&draft).unwrap();
        assert!(result.review_issues.contains(&expected),"{:?}",result.review_issues);
        assert_eq!(result.draft.blocks[0].text,after);
    }
}

#[test]
fn malformed_foreign_incomplete_and_wrong_mode_drafts_are_still_rejected() {
    let (config,mode,source)=fixture("会议时间待确认。");
    for bad in [
        "not-json",
        r#"{"resolvedMode":"light","blocks":[{"text":"会议时间待确认。","sourceRefs":["foreign"]}]}"#,
        r#"{"resolvedMode":"light","blocks":[{"text":"会议时间待确认。","sourceRange":[1,1]}]}"#,
        r#"{"resolvedMode":"structure","blocks":[{"text":"会议时间待确认。","sourceRange":[0,0]}]}"#,
        r#"{"resolvedMode":"light","blocks":[]}"#,
    ] { assert!(decode(&config,&mode,&source,&DictionarySnapshot::default(),bad).is_err()); }
    let mut incomplete=source.clone();incomplete.complete=false;
    assert!(decode(&config,&mode,&incomplete,&DictionarySnapshot::default(),&content("会议时间待确认。")).is_err());
}

#[test]
fn clean_candidate_has_no_claim_of_semantic_proof_and_keeps_complete_mapping() {
    let (config,mode,source)=fixture("如果会议没确认，先不要发邀请。");
    let result=decode(&config,&mode,&source,&DictionarySnapshot::default(),&content(&source.text())).unwrap();
    assert!(result.review_issues.is_empty());assert_eq!(result.draft.blocks[0].source_refs,vec!["item-0"]);
}

async fn server(content:String,finish:&str,delay:Duration)->(String,tokio::task::JoinHandle<()>) {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint=format!("http://{}/test",listener.local_addr().unwrap());
    let response=json!({"choices":[{"finish_reason":finish,"message":{"content":content}}]}).to_string();
    let task=tokio::spawn(async move{
        let (mut socket,_)=listener.accept().await.unwrap();let mut bytes=Vec::new();let mut chunk=[0;4096];
        loop {
            let size=socket.read(&mut chunk).await.unwrap();if size==0{return;}bytes.extend_from_slice(&chunk[..size]);
            assert!(bytes.len()<2*1024*1024);
            if let Some(index)=bytes.windows(4).position(|w|w==b"\r\n\r\n") {
                let header=String::from_utf8_lossy(&bytes[..index]);
                let length=header.lines().find_map(|line|line.split_once(':').filter(|(key,_)|key.eq_ignore_ascii_case("content-length")).map(|(_,value)|value.trim().parse::<usize>().unwrap())).unwrap();
                if bytes.len()>=index+4+length {break;}
            }
        }
        tokio::time::sleep(delay).await;
        let header=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len());
        if socket.write_all(header.as_bytes()).await.is_ok(){let _=socket.write_all(response.as_bytes()).await;}
    });
    (endpoint,task)
}

#[tokio::test]
async fn auto_history_returns_one_reviewable_draft_without_requesting_an_automatic_verdict() {
    let (config,mut mode,source)=fixture("设备预算是三千五百元。");mode.base_mode=BaseMode::Auto;mode.mode_id="auto".into();
    let (endpoint,task)=server(json!({"resolvedMode":"light","text":"设备预算是五千元。"}).to_string(),"stop",Duration::ZERO).await;
    let result=run(shared_client().unwrap(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),CancellationToken::new(),&endpoint,Duration::from_secs(2)).await.unwrap();
    task.await.unwrap();assert!(result.review_issues.contains(&ManualReviewIssue::ValuesChanged));
}

#[tokio::test]
async fn explicit_light_history_uses_shared_cleanup_but_risky_draft_remains_editable() {
    let (config,mode,source)=fixture("设备预算是三千五百元。");
    assert_eq!(production_protocol_version(&config,&mode),MINIMUM_PIPELINE_VERSION);
    let (endpoint,task)=server(json!({"resolvedMode":"light","text":"设备预算是五千元。"}).to_string(),"stop",Duration::ZERO).await;
    let result=run(shared_client().unwrap(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),CancellationToken::new(),&endpoint,Duration::from_secs(2)).await.unwrap();
    task.await.unwrap();assert_eq!(result.draft.blocks[0].text,"设备预算是五千元。");
    assert!(result.review_issues.contains(&ManualReviewIssue::ValuesChanged));
    assert_eq!(source.text(),"设备预算是三千五百元。");
}

#[tokio::test]
async fn transport_does_not_expose_truncated_or_oversized_drafts() {
    let (config,mode,source)=fixture("会议时间待确认。");
    for (body,finish,expected) in [(content(&source.text()),"length","truncated_output"),("大".repeat(400_000),"stop","response_too_large")] {
        let (endpoint,task)=server(body,finish,Duration::ZERO).await;
        let error=run(shared_client().unwrap(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),CancellationToken::new(),&endpoint,Duration::from_secs(2)).await.err().unwrap();
        assert_eq!(error.code,expected);task.await.unwrap();
    }
}

#[tokio::test]
async fn cancellation_wins_and_absolute_deadline_cannot_return_a_late_candidate() {
    let (config,mode,source)=fixture("会议时间待确认。");
    let token=CancellationToken::new();token.cancel();
    let error=run(shared_client().unwrap(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),token,"http://127.0.0.1:1",Duration::from_secs(1)).await.err().unwrap();
    assert_eq!(error.code,"cancelled");
    let (endpoint,task)=server(content(&source.text()),"stop",Duration::from_millis(150)).await;
    let error=run(shared_client().unwrap(),&config,"synthetic-key",&mode,&source,&DictionarySnapshot::default(),CancellationToken::new(),&endpoint,Duration::from_millis(30)).await.err().unwrap();
    assert_eq!(error.code,"rewrite_timeout");task.abort();
}
