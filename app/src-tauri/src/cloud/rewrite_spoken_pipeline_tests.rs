//! Production transport seams; all responses are deterministic local fixtures.
use super::*;
use crate::core::{AppWritingScene, DraftLanguage, SourceItem};
use crate::core::writing_context::WritingContext;
use std::sync::{Arc,Mutex};
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::TcpListener,task::JoinHandle};

const ACCEPT:&str=r#"{"decision":"accept","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"pass"},"issues":[]}"#;
enum Reply { Echo, Fixed(String) }
fn draft(text:&str)->Reply {Reply::Fixed(json!({"resolvedMode":"light","text":text}).to_string())}
struct Server {endpoint:String,requests:Arc<Mutex<Vec<Value>>>,task:JoinHandle<()>}
impl Drop for Server {fn drop(&mut self){self.task.abort();}}
impl Server {
    async fn new(replies:Vec<Reply>)->Self {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint=format!("http://{}/test",listener.local_addr().unwrap());
        let requests=Arc::new(Mutex::new(Vec::new()));let seen=requests.clone();
        let task=tokio::spawn(async move {
            for reply in replies {
                let (mut socket,_)=listener.accept().await.unwrap();let mut bytes=Vec::new();let mut chunk=[0;4096];
                let (header,length)=loop {
                    let n=socket.read(&mut chunk).await.unwrap();if n==0{return;}bytes.extend_from_slice(&chunk[..n]);
                    assert!(bytes.len()<2*1024*1024);
                    if let Some(at)=bytes.windows(4).position(|w|w==b"\r\n\r\n") {
                        let head=String::from_utf8_lossy(&bytes[..at]);
                        let length=head.lines().find_map(|line|line.split_once(':').filter(|(name,_)|name.eq_ignore_ascii_case("content-length"))
                            .map(|(_,value)|value.trim().parse::<usize>().unwrap())).unwrap();
                        break (at+4,length);
                    }
                };
                while bytes.len()<header+length {let n=socket.read(&mut chunk).await.unwrap();if n==0{return;}bytes.extend_from_slice(&chunk[..n]);}
                let body:Value=serde_json::from_slice(&bytes[header..header+length]).unwrap();
                let response=match reply {Reply::Fixed(text)=>text,Reply::Echo=>{
                    let data=data(&body);json!({"resolvedMode":"light","text":data["sourceText"]}).to_string()
                }};
                seen.lock().unwrap().push(body);
                let body=json!({"choices":[{"finish_reason":"stop","message":{"content":response}}]}).to_string();
                let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                if socket.write_all(response.as_bytes()).await.is_err(){return;}
            }
        });Self{endpoint,requests,task}
    }
    fn count(&self)->usize{self.requests.lock().unwrap().len()}
}
fn data(body:&Value)->Value{serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap()}
fn client()->reqwest::Client{reqwest::Client::builder().no_proxy().build().unwrap()}
fn fixture(text:&str)->(ProviderConfig,ModeSnapshot,SourceRevision){
    let mut config=ProviderConfig::default();config.region=Some("beijing".into());
    let mode=ModeSnapshot{mode_id:"document".into(),name:"文档".into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1};
    let source=SourceRevision{id:"source".into(),session_id:"session".into(),asr_attempt_id:"asr".into(),generation:9,
        provider:"synthetic".into(),model:"synthetic".into(),adapter_version:"test".into(),complete:true,created_at:"now".into(),
        items:vec![SourceItem{id:"item-0".into(),provider_item_id:"0".into(),capture_order:0,text:text.into(),is_final:true,start_ms:None,end_ms:None}]};
    (config,mode,source)
}
#[test]
fn short_english_calendar_prose_uses_english_dates_without_rerouting_language(){
    for text in ["Meet on June third, twenty twenty seven.","May 5, 2027.","The date is 二零二六年十月二十三日."] {
        let(_,_,source)=fixture(text);assert!(spoken_english(&source,None),"{text}; dates={:?}",dates::mentions(text));
    }
    for text in ["我们讨论 A D H D，日期是二零二六年十月二十三日。","使用 Google Calendar，日期先保留。","原话‘Meet on June third, twenty twenty seven.’，明天再定。","A D H D"] {
        let(_,_,source)=fixture(text);assert!(!spoken_english(&source,None),"{text}");
    }
}
async fn run(server:&Server,config:&ProviderConfig,mode:&ModeSnapshot,source:&SourceRevision)->Result<RewriteResult,CloudError>{
    rewrite_with_client_execution(&client(),config,"synthetic-secret",mode,source,&DictionarySnapshot::default(),CancellationToken::new(),
        &server.endpoint,RewriteExecution::for_duration(Duration::from_secs(4))).await
}

#[tokio::test]
async fn protected_long_chinese_restores_normalized_acronym_and_date_before_frozen_review(){
    let input=format!("我们讨论 A D H D 的研究，报告日期是二零二六年十月二十三日。{}","接着记录当前实验的流程和观察结果。".repeat(30));
    let(config,mode,source)=fixture(&input);let before=source.clone();assert!(uses_guarded_long(&source));
    let server=Server::new(vec![Reply::Echo,Reply::Fixed(ACCEPT.into())]).await;
    let result=run(&server,&config,&mode,&source).await.unwrap();
    assert!(result.blocks[0].text.contains("ADHD"));assert!(result.blocks[0].text.contains("2026年10月23日"));
    assert!(!result.blocks[0].text.contains("A D H D"));assert!(!result.blocks[0].text.contains("⟪SM"));
    assert_eq!(source,before);assert_eq!(result.blocks[0].source_refs,["item-0"]);assert_eq!(server.count(),2);
    let requests=server.requests.lock().unwrap();assert!(data(&requests[0])["sourceText"].as_str().unwrap().contains("⟪SM"));
    let review=data(&requests[1]);assert_eq!(review["sourceText"],input);assert_eq!(review["draftText"],result.blocks[0].text);
    for request in requests.iter(){assert!(request["messages"][0]["content"].as_str().unwrap().contains(spoken_format::VERSION));}
}

#[tokio::test]
async fn sentence_repair_keeps_shared_notation_and_requires_a_fresh_bound_review(){
    let bad="我感觉这个方案还没有太能做到。";let fixed="我感觉这个方案还没有很好地做到。";
    let input=format!("{bad}报告日期是二零二六年十月二十三日，我们研究 A D H D。");
    let(config,mode,source)=fixture(&input);let before=source.clone();
    let rejection=json!({"decision":"reject","checks":{"coverage":"pass","facts":"pass","scope":"pass","intent":"pass","cleanup":"fail"},
        "issues":[{"dimension":"cleanup","sourceQuote":bad,"draftQuote":bad,"reason":"仅修复本句的卡顿搭配"}]}).to_string();
    let repair=json!({"replacements":[{"id":"r0","text":fixed}]}).to_string();
    let server=Server::new(vec![draft(&input),Reply::Fixed(rejection),Reply::Fixed(repair),Reply::Fixed(ACCEPT.into())]).await;
    let result=run(&server,&config,&mode,&source).await.unwrap();
    assert_eq!(result.blocks[0].text,format!("{fixed}报告日期是2026年10月23日，我们研究 ADHD。"));
    assert_eq!(source,before);assert_eq!(server.count(),4);
    let requests=server.requests.lock().unwrap();
    for request in requests.iter(){assert!(request["messages"][0]["content"].as_str().unwrap().contains(spoken_format::VERSION));}
    assert_eq!(data(&requests[3])["sourceText"],input);assert_eq!(data(&requests[3])["draftText"],result.blocks[0].text);
    assert_ne!(data(&requests[1])["draftText"],data(&requests[3])["draftText"]);
}

#[tokio::test]
async fn merging_separate_letters_or_changing_recorded_date_cannot_reach_all_pass_review(){
    for (input,output) in [
        ("四个独立字母是 A D H D，请保持分开。","四个独立字母是 ADHD，请保持分开。"),
        ("四个独立字母是 A D H D，请保持分开。","四个独立字母是 A D X D，请保持分开。"),
        ("我们讨论 A D H D 的研究。","我们讨论 ADHHD 的研究。"),
        ("我们讨论 A D H D 的研究。","我们讨论 A D X D 的研究。"),
        ("报告日期是二零二六年十月二十三日。","报告日期是2026年10月24日。"),
    ] {
        let(config,mode,source)=fixture(input);let before=source.clone();
        let server=Server::new(vec![draft(output),Reply::Fixed(ACCEPT.into())]).await;
        assert!(run(&server,&config,&mode,&source).await.is_err());
        assert_eq!(server.count(),1,"native admission must reject before a reviewer can waive the change");assert_eq!(source,before);
    }
}

#[test]
fn even_an_explicit_five_pass_verdict_cannot_override_letter_list_guard(){
    let(config,mode,source)=fixture("四个独立字母是 A D H D，请保持分开。");
    for text in ["四个独立字母是 ADHD，请保持分开。","四个独立字母是 A D X D，请保持分开。"] {
        let draft=RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![OutputBlock{id:"draft".into(),text:text.into(),source_refs:vec!["item-0".into()]}]};
        let request=semantic_review::prepare_minimum(&config,&mode,&source,&draft).unwrap();
        assert!(semantic_review::parse_minimum(ACCEPT,&request,&mode,&source,&draft).is_err());
    }
}

#[tokio::test]
async fn explicit_english_scene_formats_chinese_date_without_touching_source(){
    let input="The report date is 二零二六年十月二十三日. We are discussing A D H D research.";
    let(config,mode,source)=fixture(input);let before=source.clone();
    let context=WritingContext{scene:AppWritingScene::Document,language:DraftLanguage::English,input:None};
    let server=Server::new(vec![draft(input),Reply::Fixed(ACCEPT.into())]).await;
    let result=minimum_pipeline::run_until_with_writing_context(&client(),&config,"synthetic-secret",&mode,&source,&DictionarySnapshot::default(),
        CancellationToken::new(),&server.endpoint,RewriteExecution::for_duration(Duration::from_secs(4)),&context).await.unwrap();
    assert!(result.blocks[0].text.contains("2026-10-23"));assert!(result.blocks[0].text.contains("ADHD"));assert_eq!(source,before);assert_eq!(server.count(),2);
    for request in server.requests.lock().unwrap().iter(){
        assert!(request["messages"][0]["content"].as_str().unwrap().contains(spoken_format::VERSION));
        assert!(data(request)["notationHints"]["dateStyle"].as_str().unwrap().contains("ISO"));
    }
}

#[tokio::test]
async fn history_keeps_human_review_candidate_when_letter_list_is_corrupted(){
    let input="四个独立字母是 A D H D，请保持分开。";
    let(config,mode,source)=fixture(input);let before=source.clone();
    let server=Server::new(vec![draft("四个独立字母是 ADHD，请保持分开。")]).await;
    let candidate=manual_candidate::run(&client(),&config,"synthetic-secret",&mode,&source,&DictionarySnapshot::default(),CancellationToken::new(),
        &server.endpoint,Duration::from_secs(4)).await.unwrap();
    assert!(candidate.review_issues.contains(&ManualReviewIssue::MeaningChanged));assert_eq!(server.count(),1);assert_eq!(source,before);
}

#[tokio::test]
async fn confirmed_history_english_dates_share_format_and_real_source_ids(){
    let input="The report date is 二零二六年十月二十三日. We are discussing A D H D research.";
    let(config,mode,source)=fixture(input);let before=source.clone();
    let context=crate::core::ConfirmedContext{source_revision_id:source.id.clone(),language:DraftLanguage::English,corrections:Vec::new()};
    let server=Server::new(vec![draft(input),Reply::Fixed(ACCEPT.into())]).await;
    let out=confirmed_candidate::run(&client(),&config,"synthetic-secret",&mode,&source,&DictionarySnapshot::default(),&context,CancellationToken::new(),
        &server.endpoint,Duration::from_secs(4)).await;
    let candidate=out.result.unwrap();assert!(candidate.review_issues.is_empty(),"{:?}",candidate.review_issues);
    assert!(candidate.draft.blocks[0].text.contains("2026-10-23"));assert!(candidate.draft.blocks[0].text.contains("ADHD"));
    assert_eq!(candidate.draft.blocks[0].source_refs,["item-0"]);assert_eq!(source,before);assert_eq!(server.count(),2);
}

#[tokio::test]
async fn compatible_provider_legacy_mode_uses_same_date_and_acronym_presentation(){
    let input="今天研究 A D H D，报告日期是二零二六年十月二十三日。";
    let(mut config,mut mode,source)=fixture(input);config.rewrite_provider="deepseek".into();config.rewrite_model="deepseek-flash".into();
    mode.mode_id="light".into();mode.base_mode=BaseMode::Light;let before=source.clone();
    let content=json!({"resolvedMode":"light","blocks":[{"text":input,"sourceRange":[0,0]}]}).to_string();
    let server=Server::new(vec![Reply::Fixed(content)]).await;
    let result=run(&server,&config,&mode,&source).await.unwrap();
    assert!(result.blocks[0].text.contains("ADHD"));assert!(result.blocks[0].text.contains("2026年10月23日"));assert_eq!(source,before);
    assert_eq!(server.count(),1);assert!(server.requests.lock().unwrap()[0]["messages"][0]["content"].as_str().unwrap().contains(spoken_format::VERSION));
}
