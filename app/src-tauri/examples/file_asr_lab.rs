//! Opt-in file recovery experiment. No UI, database writes, auto retry or delivery.
//! file_asr_lab --wav ABS --config-db ABS --out ABS [--language auto|zh|en]
//! The receipt includes test transcription and stays in the explicit local output.
use base64::{Engine as _,engine::general_purpose::STANDARD};
use chrono::Utc;
use rusqlite::{Connection,OpenFlags};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use speakmode_lib::{cloud::{self,CloudError},core::{ProviderConfig,SourceItem,SourceRevision}};
use std::{fs::{self,OpenOptions},io::{Cursor,Write},path::PathBuf,time::Duration};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MODEL:&str="qwen3-asr-flash";
const ADAPTER:&str="qwen-file-lab-v1";
const MAX_ENCODED_BYTES:usize=10_000_000;
fn error(code:&str)->CloudError{CloudError::new(code,"file_asr_experiment","文件识别实验未完成；详见固定结果码")}

struct Audio {body:Value,sha256:String,samples:u64}
fn prepare(bytes:&[u8],language:&str)->Result<Audio,CloudError>{
    if !["auto","zh","en"].contains(&language){return Err(error("unsupported_language"));}
    // Bound the encoded input too: the provider's documented limit includes base64 expansion.
    if bytes.len().div_ceil(3)*4+"data:audio/wav;base64,".len()>MAX_ENCODED_BYTES{return Err(error("audio_size_limit"));}
    let mut reader=hound::WavReader::new(Cursor::new(bytes)).map_err(|_|error("invalid_audio"))?;
    let spec=reader.spec();
    if spec.channels!=1||spec.sample_rate!=16000||spec.bits_per_sample!=16||spec.sample_format!=hound::SampleFormat::Int {
        return Err(error("audio_format"));
    }
    let samples=u64::from(reader.duration());
    if samples==0||samples>16000*300{return Err(error("audio_duration_limit"));}
    let actual=reader.samples::<i16>().try_fold(0u64,|n,sample|sample.map(|_|n+1)).map_err(|_|error("invalid_audio"))?;
    if actual!=samples{return Err(error("incomplete_audio"));}
    let mut options=json!({"enable_itn":false});
    if language!="auto"{options["language"]=json!(language);}
    Ok(Audio{body:json!({"model":MODEL,"stream":false,"messages":[{"role":"user","content":[
        {"type":"input_audio","input_audio":{"data":format!("data:audio/wav;base64,{}",STANDARD.encode(bytes))}}
    ]}],"asr_options":options}),sha256:format!("{:x}",Sha256::digest(bytes)),samples})
}

fn parse(data:&Value,attempt:&str)->Result<SourceRevision,CloudError>{
    let model=data["model"].as_str().ok_or_else(||error("invalid_response"))?;
    if model!=MODEL{return Err(error("unexpected_model"));}
    if data.pointer("/choices/0/finish_reason").and_then(Value::as_str)!=Some("stop") {return Err(error("incomplete_response"));}
    let text=data.pointer("/choices/0/message/content").and_then(Value::as_str).ok_or_else(||error("invalid_response"))?;
    if text.trim().is_empty()||text.contains('\0')||text.chars().count()>80000{return Err(error("invalid_transcript"));}
    Ok(SourceRevision{id:Uuid::new_v4().to_string(),session_id:format!("file-asr-lab-{attempt}"),
        asr_attempt_id:attempt.into(),generation:1,provider:"aliyun".into(),model:MODEL.into(),adapter_version:ADAPTER.into(),
        complete:true,created_at:Utc::now().to_rfc3339(),items:vec![SourceItem{id:format!("{attempt}:0"),
            provider_item_id:"0".into(),capture_order:0,text:text.into(),is_final:true,start_ms:None,end_ms:None}]})
}

async fn request(client:&reqwest::Client,endpoint:&str,workspace:Option<&str>,secret:&str,body:&Value,
    attempt:&str,cancel:&CancellationToken,deadline:Instant)->Result<SourceRevision,CloudError>{
    let check=||if cancel.is_cancelled(){Err(error("cancelled"))}else if Instant::now()>=deadline{Err(error("file_asr_timeout"))}else{Ok(())};
    check()?;
    let job=async{
        let mut request=client.post(endpoint).bearer_auth(secret).json(body).timeout(deadline.saturating_duration_since(Instant::now()));
        if let Some(workspace)=workspace.filter(|s|!s.is_empty()){request=request.header("X-DashScope-WorkSpace",workspace);}
        let transport=|e:reqwest::Error|error(if e.is_timeout(){"file_asr_timeout"}else{"file_asr_network"});
        let mut response=request.send().await.map_err(transport)?;check()?;
        if !response.status().is_success(){return Err(error(match response.status().as_u16(){401|403=>"authentication_failed",429=>"rate_limited",_=>"file_asr_service_error"}));}
        let mut bytes=Vec::new();
        while let Some(chunk)=response.chunk().await.map_err(transport)? {
            check()?;if bytes.len()+chunk.len()>1024*1024{return Err(error("response_too_large"));}bytes.extend_from_slice(&chunk);
        }
        let data:Value=serde_json::from_slice(&bytes).map_err(|_|error("invalid_response"))?;
        let source=parse(&data,attempt)?;check()?;Ok(source)
    };
    let result=tokio::select!{biased;_=cancel.cancelled()=>Err(error("cancelled")),
        result=tokio::time::timeout_at(deadline,job)=>result.unwrap_or_else(|_|Err(error("file_asr_timeout")))};
    check()?;result
}

async fn run()->Result<(),CloudError>{
    let args:Vec<String>=std::env::args().skip(1).collect();
    if ![6,8].contains(&args.len()){return Err(error("usage"));}
    let mut pairs=std::collections::HashMap::new();
    for pair in args.chunks_exact(2){
        if !["--wav","--config-db","--out","--language"].contains(&pair[0].as_str())||pairs.insert(pair[0].as_str(),pair[1].as_str()).is_some(){return Err(error("usage"));}
    }
    let path=|key|->Result<PathBuf,CloudError>{let path=PathBuf::from(pairs.get(key).ok_or_else(||error("usage"))?);
        if !path.is_absolute(){return Err(error("absolute_path_required"));}Ok(path)};
    let wav=path("--wav")?;let db=path("--config-db")?;let out=path("--out")?;let language=*pairs.get("--language").unwrap_or(&"auto");
    if fs::metadata(&wav).map_err(|_|error("audio_unavailable"))?.len()>MAX_ENCODED_BYTES as u64{return Err(error("audio_size_limit"));}
    let audio=prepare(&fs::read(&wav).map_err(|_|error("audio_unavailable"))?,language)?;
    let connection=Connection::open_with_flags(db,OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|_|error("config_unavailable"))?;
    let raw:String=connection.query_row("SELECT data FROM provider_config WHERE singleton=1",[],|row|row.get(0)).map_err(|_|error("config_unavailable"))?;
    let config:ProviderConfig=serde_json::from_str(&raw).map_err(|_|error("invalid_config"))?;drop(connection);
    let endpoint=cloud::endpoints(&config)?.rewrite;
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|error("network_setup"))?;
    // Claim an immutable receipt before credential access or any network request.
    let mut output=OpenOptions::new().write(true).create_new(true).open(&out).map_err(|_|error("output_unavailable"))?;
    let attempt=Uuid::new_v4().to_string();let start=std::time::Instant::now();
    let mut row=json!({"scope":"FILE_RECOVERY_SYNTHETIC_PILOT","startedAt":Utc::now().to_rfc3339(),
        "appVersion":env!("CARGO_PKG_VERSION"),"model":MODEL,"adapterVersion":ADAPTER,"region":config.region,
        "audioSha256":audio.sha256,"audioSamples":audio.samples,"language":language,"dictionaryTerms":[],
        "automaticRetries":0,"automaticInputs":0,"attemptId":attempt,"status":"started"});
    writeln!(output,"{}",row).map_err(|_|error("output_failed"))?;output.flush().map_err(|_|error("output_failed"))?;
    let result=match cloud::credentials::read(&config){Ok(secret)=>request(&client,&endpoint,config.workspace.as_deref(),&secret,&audio.body,&attempt,&CancellationToken::new(),Instant::now()+Duration::from_secs(60)).await,Err(e)=>Err(e)};
    row["elapsedMs"]=json!(start.elapsed().as_secs_f64()*1000.0);row["completedAt"]=json!(Utc::now().to_rfc3339());
    match &result{Ok(source)=>{row["status"]=json!("passed");row["source"]=serde_json::to_value(source).unwrap();},Err(e)=>{row["status"]=json!("failed");row["errorCode"]=json!(e.code);}}
    writeln!(output,"{}",row).map_err(|_|error("output_failed"))?;output.flush().map_err(|_|error("output_failed"))?;
    println!("{}",json!({"receipt":out,"status":row["status"],"elapsedMs":row["elapsedMs"],"errorCode":row["errorCode"]}));
    result.map(|_|())
}
#[tokio::main]
async fn main(){if let Err(e)=run().await{eprintln!("{}",json!({"errorCode":e.code}));std::process::exit(1);}}

#[cfg(test)]
mod tests{
    use super::*;
    fn wav(samples:u32,rate:u32)->Vec<u8>{let mut bytes=Cursor::new(Vec::new());{
        let mut w=hound::WavWriter::new(&mut bytes,hound::WavSpec{channels:1,sample_rate:rate,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();
        for _ in 0..samples{w.write_sample(1i16).unwrap();}w.finalize().unwrap();}bytes.into_inner()}
    #[test]fn audio_limits_are_checked_before_encoding_or_network(){
        assert!(prepare(&wav(1600,16000),"auto").is_ok());assert!(prepare(&wav(0,16000),"auto").is_err());
        assert!(prepare(&wav(1600,48000),"auto").is_err());assert!(prepare(&vec![0;7_500_000],"auto").is_err());
        let mut truncated=wav(1600,16000);truncated.truncate(truncated.len()-2);assert!(prepare(&truncated,"auto").is_err());
    }
    #[test]fn only_complete_matching_model_results_become_fresh_source(){
        let good=json!({"model":MODEL,"choices":[{"finish_reason":"stop","message":{"content":"预算3500元，先不要发送。"}}]});
        let source=parse(&good,"new-attempt").unwrap();assert!(source.complete);assert_eq!(source.model,MODEL);
        assert_eq!(source.adapter_version,ADAPTER);assert_eq!(source.items[0].id,"new-attempt:0");assert!(source.items[0].end_ms.is_none());
        for bad in [json!({}),json!({"model":"qwen-plus","choices":good["choices"]}),
            json!({"model":MODEL,"choices":[{"finish_reason":"length","message":{"content":"不完整"}}]}),
            json!({"model":MODEL,"choices":[{"finish_reason":"stop","message":{"content":" "}}]})]{assert!(parse(&bad,"attempt").is_err());}
    }
    #[tokio::test]async fn cancelled_or_expired_call_never_opens_a_socket(){
        let client=reqwest::Client::new();let cancel=CancellationToken::new();cancel.cancel();
        let result=request(&client,"http://127.0.0.1:1",None,"synthetic",&json!({}),"a",&cancel,Instant::now()+Duration::from_secs(1)).await;
        assert_eq!(result.unwrap_err().code,"cancelled");
        let result=request(&client,"http://127.0.0.1:1",None,"synthetic",&json!({}),"a",&CancellationToken::new(),Instant::now()).await;
        assert_eq!(result.unwrap_err().code,"file_asr_timeout");
    }
    #[tokio::test]async fn invalid_http_responses_are_bounded_and_never_complete(){
        use tokio::{net::TcpListener,io::{AsyncReadExt,AsyncWriteExt}};
        for (status,body,expected) in [(401,"private-provider-body".to_owned(),"authentication_failed"),
            (200,"not json".into(),"invalid_response"),(200,"x".repeat(1024*1024+1),"response_too_large")] {
            let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let endpoint=format!("http://{}",listener.local_addr().unwrap());
            let server=tokio::spawn(async move{let(mut socket,_)=listener.accept().await.unwrap();let mut bytes=[0;4096];let _=socket.read(&mut bytes).await;
                let _=socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await;});
            let result=request(&reqwest::Client::builder().no_proxy().build().unwrap(),&endpoint,None,"synthetic",&json!({}),"a",&CancellationToken::new(),Instant::now()+Duration::from_secs(3)).await;
            assert_eq!(result.unwrap_err().code,expected);server.await.unwrap();
        }
    }
    #[tokio::test]async fn cancellation_and_timeout_interrupt_inflight_http(){
        use tokio::{net::TcpListener,io::AsyncReadExt,sync::oneshot};
        for cancel_early in [true,false]{
            let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let endpoint=format!("http://{}",listener.local_addr().unwrap());
            let(tx,rx)=oneshot::channel();let server=tokio::spawn(async move{let(mut socket,_)=listener.accept().await.unwrap();let mut b=[0;4096];let _=socket.read(&mut b).await;let _=tx.send(());std::future::pending::<()>().await;});
            let cancel=CancellationToken::new();let trigger=cancel.clone();let stopper=tokio::spawn(async move{rx.await.unwrap();if cancel_early{trigger.cancel();}});
            let result=request(&reqwest::Client::builder().no_proxy().build().unwrap(),&endpoint,None,"synthetic",&json!({}),"a",&cancel,Instant::now()+Duration::from_millis(250)).await;
            assert_eq!(result.unwrap_err().code,if cancel_early{"cancelled"}else{"file_asr_timeout"});stopper.await.unwrap();server.abort();
        }
    }
}
