use super::*;
use tokio::{io::{AsyncReadExt,AsyncWriteExt},sync::oneshot};

fn config()->ProviderConfig {ProviderConfig{provider:"google".into(),asr_model:"gemini-3.5-transcribe".into(),adapter_version:"google-transcribe-v1".into(),rewrite_provider:"deepseek".into(),rewrite_model:"deepseek-flash".into(),..ProviderConfig::default()}}
fn valid()->Value {json!({"status":"completed","steps":[{"type":"model_output","content":[{"type":"text","text":"Hello，明天再试。"}]}]})}
async fn server(status:&str,value:Value,delay:Duration)->(String,oneshot::Receiver<(String,Value)>,tokio::task::JoinHandle<()>) {
    server_with(status,value,delay,"",false).await
}
async fn server_with(status:&str,value:Value,delay:Duration,extra:&str,chunked:bool)->(String,oneshot::Receiver<(String,Value)>,tokio::task::JoinHandle<()>) {
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url=format!("http://{}/v1beta/interactions",listener.local_addr().unwrap());
    let status=status.to_owned();let extra=extra.to_owned();let (tx,rx)=oneshot::channel();
    let task=tokio::spawn(async move {
        let (mut socket,_)=listener.accept().await.unwrap();let mut bytes=Vec::new();let mut buf=[0u8;4096];
        let header_end=loop {let n=socket.read(&mut buf).await.unwrap();assert!(n>0);bytes.extend_from_slice(&buf[..n]);
            if let Some(pos)=bytes.windows(4).position(|w|w==b"\r\n\r\n") {break pos+4;}};
        let headers=String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let len=headers.lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v|v.trim().parse::<usize>().ok())).unwrap();
        while bytes.len()<header_end+len {let n=socket.read(&mut buf).await.unwrap();assert!(n>0);bytes.extend_from_slice(&buf[..n]);}
        let _=tx.send((headers,serde_json::from_slice(&bytes[header_end..header_end+len]).unwrap()));
        tokio::time::sleep(delay).await;let body=value.to_string();
        let response=if chunked {format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n{extra}Connection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",body.len())}
            else {format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",body.len())};
        let _=socket.write_all(response.as_bytes()).await;
    });(url,rx,task)
}
async fn inputs(finish:bool)->mpsc::Receiver<AsrInput> {
    let (tx,rx)=mpsc::channel(4);let mut pcm16=vec![0;1600];pcm16[0]=1;pcm16[1]=-2;
    tx.send(AsrInput::Frame{sequence:0,pcm16,padding_samples:1598}).await.unwrap();
    if finish {tx.send(AsrInput::Finish{next_sequence:1}).await.unwrap();}rx
}
#[tokio::test]
async fn complete_wav_and_verbatim_request_use_google_key_without_aliyun_headers() {
    let (url,request,task)=server("200 OK",valid(),Duration::ZERO).await;
    let telemetry=Arc::new(std::sync::Mutex::new(None));let observed=telemetry.clone();
    let items=run_at(config(),"google-test-key".into(),"en".into(),DictionarySnapshot::default(),"attempt-g".into(),inputs(true).await,CancellationToken::new(),Arc::new(move|p|if let AsrProgress::Diagnostics(value)=p {*observed.lock().unwrap()=Some(value);}),url,Duration::from_secs(2)).await.unwrap();
    assert_eq!(items[0].id,"attempt-g:google-final");assert_eq!(items[0].text,"Hello，明天再试。");
    let (headers,body)=request.await.unwrap();let headers=headers.to_ascii_lowercase();
    assert!(headers.contains("x-goog-api-key: google-test-key"));assert!(!headers.contains("authorization:"));assert!(!headers.contains("dashscope"));
    assert_eq!(body["store"],false);assert_eq!(body["generation_config"]["transcription_config"]["mode"]["type"],"verbatim");
    assert_eq!(body["generation_config"]["transcription_config"]["language_codes"],json!(["en-US"]));
    let wav=STANDARD.decode(body["input"][0]["data"].as_str().unwrap()).unwrap();assert_eq!(&wav[..4],b"RIFF");assert_eq!(&wav[44..],&[1,0,254,255]);task.await.unwrap();
    let telemetry=telemetry.lock().unwrap();let metadata=telemetry.as_ref().unwrap();
    assert_eq!(metadata.dequeued.padding_samples,1598);assert_eq!(metadata.socket_written.padding_samples,0);
    assert!(metadata.finish_written_at.is_none(),"batch HTTP cannot observe WebSocket write boundaries");
}
#[tokio::test]
async fn in_flight_cancellation_discards_the_late_completed_transcript() {
    let (url,request,server)=server("200 OK",valid(),Duration::from_millis(80)).await;
    let token=CancellationToken::new();let cancel=token.clone();let audio=inputs(true).await;
    let job=tokio::spawn(run_at(config(),"key".into(),"auto".into(),DictionarySnapshot::default(),"a".into(),audio,cancel,Arc::new(|_|{}),url,Duration::from_secs(2)));
    request.await.unwrap();token.cancel();assert_eq!(job.await.unwrap().unwrap_err().code,"cancelled");server.await.unwrap();
}
#[tokio::test]
async fn declared_and_chunked_oversized_responses_are_rejected() {
    for chunked in [false,true] {
        let (url,_,server)=server_with("200 OK",json!({"private":"x".repeat(MAX_RESPONSE+1)}),Duration::ZERO,"",chunked).await;
        let result=run_at(config(),"key".into(),"auto".into(),DictionarySnapshot::default(),"a".into(),inputs(true).await,CancellationToken::new(),Arc::new(|_|{}),url,Duration::from_secs(2)).await;
        assert_eq!(result.unwrap_err().code,"invalid_asr_event");server.await.unwrap();
    }
}
#[tokio::test]
async fn redirect_does_not_send_google_key_to_another_origin() {
    let target=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let location=format!("Location: http://{}/stolen\r\n",target.local_addr().unwrap());
    let (url,_,server)=server_with("307 Temporary Redirect",valid(),Duration::ZERO,&location,false).await;
    let result=run_at(config(),"key".into(),"auto".into(),DictionarySnapshot::default(),"a".into(),inputs(true).await,CancellationToken::new(),Arc::new(|_|{}),url,Duration::from_secs(2)).await;
    assert_eq!(result.unwrap_err().code,"asr_service_unavailable");
    assert!(tokio::time::timeout(Duration::from_millis(40),target.accept()).await.is_err());server.await.unwrap();
}
#[tokio::test]
async fn missing_frame_or_wrong_finish_count_fails_before_http() {
    for (sequence,next_sequence) in [(1,2),(0,2)] {
        let (tx,rx)=mpsc::channel(4);tx.send(AsrInput::Frame{sequence,pcm16:vec![1;16],padding_samples:0}).await.unwrap();
        tx.send(AsrInput::Finish{next_sequence}).await.unwrap();drop(tx);
        let result=run_at(config(),"never-sent".into(),"auto".into(),DictionarySnapshot::default(),"a".into(),rx,CancellationToken::new(),Arc::new(|_|{}),"http://127.0.0.1:1".into(),Duration::from_secs(1)).await;
        assert_eq!(result.unwrap_err().code,"audio_incomplete");
    }
}
#[tokio::test]
async fn incomplete_capture_and_cancel_do_not_send_audio() {
    let error=run_at(config(),"never-sent".into(),"auto".into(),DictionarySnapshot::default(),"a".into(),inputs(false).await,CancellationToken::new(),Arc::new(|_|{}),"http://127.0.0.1:1".into(),Duration::from_secs(1)).await.unwrap_err();
    assert_eq!(error.code,"audio_disconnected");
    let token=CancellationToken::new();token.cancel();
    assert_eq!(run_at(config(),"never-sent".into(),"auto".into(),DictionarySnapshot::default(),"a".into(),inputs(true).await,token,Arc::new(|_|{}),"http://127.0.0.1:1".into(),Duration::from_secs(1)).await.unwrap_err().code,"cancelled");
}
#[tokio::test]
async fn timeout_and_late_response_cannot_return_a_source() {
    // Exercise an in-flight late response, not first-use TLS/client setup on a
    // loaded test host. The production timeout is unchanged.
    client().unwrap();
    let (url,request,task)=server("200 OK",valid(),Duration::from_millis(500)).await;
    let error=run_at(config(),"key".into(),"auto".into(),DictionarySnapshot::default(),"a".into(),inputs(true).await,CancellationToken::new(),Arc::new(|_|{}),url,Duration::from_millis(200)).await.unwrap_err();
    assert_eq!(error.code,"asr_finish_timeout");assert!(request.await.is_ok());task.await.unwrap();
}
#[tokio::test]
async fn authentication_rate_limits_and_redirects_do_not_expose_provider_error_body() {
    for (status,code) in [("403 Forbidden","asr_auth_failed"),("429 Too Many Requests","asr_rate_limited"),("302 Found","asr_service_unavailable")] {
        let (url,_,task)=server(status,json!({"secret":"provider-private-body"}),Duration::ZERO).await;
        let e=run_at(config(),"key".into(),"auto".into(),DictionarySnapshot::default(),"a".into(),inputs(true).await,CancellationToken::new(),Arc::new(|_|{}),url,Duration::from_secs(2)).await.unwrap_err();
        assert_eq!(e.code,code);assert!(!e.message.contains("provider-private"));task.await.unwrap();
    }
}
#[test]
fn incomplete_refused_empty_or_nontext_responses_are_not_final_sources() {
    for value in [json!({"status":"incomplete","steps":[]}),json!({"status":"completed","steps":[{"type":"model_output","content":[{"type":"text","text":""}]}]}),json!({"status":"completed","steps":[{"type":"model_output","content":[{"type":"refusal","text":"denied"}]}]}),json!({"status":"completed","steps":[{"type":"function_call"}]}),json!({"status":"completed","steps":[{"type":"model_output","content":[{"type":"text","text":"bad\u{0000}text"}]}]})] {assert!(parse(&value,"attempt").is_err());}
}
#[test]
fn google_product_limit_leaves_room_for_inline_encoding_and_request_overhead() {
    let body=request_body(&config(),"auto",&DictionarySnapshot::default(),&vec![0;420*16000]).unwrap();
    assert!(serde_json::to_vec(&body).unwrap().len()<MAX_REQUEST);
    assert_eq!(request_body(&config(),"auto",&DictionarySnapshot::default(),&vec![0;420*16000+1]).unwrap_err().code,"audio_frame_limit");
}

#[tokio::test]
async fn exact_product_cutoff_finishes_a_complete_batch_request_and_final_transcript(){
    // Seven minutes of synthetic samples, delivered without real-time waiting.
    // This is the real batch adapter against loopback HTTP, not Google service.
    let (url,request,server)=server("200 OK",valid(),Duration::ZERO).await;
    let (tx,rx)=mpsc::channel(4);
    let producer=tokio::spawn(async move{
        for sequence in 0..4200{tx.send(AsrInput::Frame{sequence,pcm16:vec![7;1600],padding_samples:0}).await.unwrap();}
        tx.send(AsrInput::Finish{next_sequence:4200}).await.unwrap();
    });
    let captions=Arc::new(std::sync::atomic::AtomicUsize::new(0));let observed=captions.clone();
    let items=run_at(config(),"synthetic-key".into(),"auto".into(),DictionarySnapshot::default(),"cutoff".into(),rx,CancellationToken::new(),Arc::new(move|progress|if matches!(progress,AsrProgress::Caption(_)){observed.fetch_add(1,std::sync::atomic::Ordering::SeqCst);}),url,Duration::from_secs(8)).await.unwrap();
    producer.await.unwrap();assert_eq!(items[0].text,"Hello，明天再试。");assert_eq!(captions.load(std::sync::atomic::Ordering::SeqCst),1);
    let (_,body)=request.await.unwrap();let wav=STANDARD.decode(body["input"][0]["data"].as_str().unwrap()).unwrap();
    assert_eq!(wav.len(),44+420*16000*2);assert_eq!(&wav[44..46],&7_i16.to_le_bytes());assert_eq!(&wav[wav.len()-2..],&7_i16.to_le_bytes());server.await.unwrap();
}
