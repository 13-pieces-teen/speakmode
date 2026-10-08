//! Real localhost TCP/WebSocket traffic, scripted provider events. Not real cloud acceptance.
use super::*;
use std::{future::Future,sync::{Mutex,atomic::{AtomicUsize,Ordering}}};
use tokio::{net::{TcpListener,TcpStream},task::JoinHandle};
use tokio_tungstenite::{WebSocketStream,accept_async};

async fn server<F,Fut>(script:F)->(String,JoinHandle<()>)
where F:FnOnce(WebSocketStream<TcpStream>)->Fut+Send+'static,Fut:Future<Output=()>+Send+'static {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let task=tokio::spawn(async move{let (socket,_)=listener.accept().await.unwrap();let socket=accept_async(socket).await.unwrap();script(socket).await;});
    (format!("ws://{address}/test-only"),task)
}
async fn receive(socket:&mut WebSocketStream<TcpStream>)->Value {
    loop {let message=tokio::time::timeout(Duration::from_secs(3),socket.next()).await.unwrap().unwrap().unwrap();if let Message::Text(text)=message{return serde_json::from_str(&text).unwrap();}}
}
async fn emit(socket:&mut WebSocketStream<TcpStream>,value:Value){socket.send(Message::Text(value.to_string())).await.unwrap();}
fn short_timeouts()->Timeouts {Timeouts{establishment:Duration::from_secs(2),send:Duration::from_secs(1),finish:Duration::from_secs(1)}}
async fn run_local(url:String,audio:mpsc::Receiver<AsrInput>,cancel:CancellationToken,callback:Arc<dyn Fn(AsrProgress)+Send+Sync>,timeouts:Timeouts)->Result<Vec<SourceItem>,CloudError> {
    run_at(ProviderConfig::default(),"test-only-not-a-real-key".into(),"auto".into(),"test-attempt".into(),audio,cancel,callback,url,timeouts).await
}
fn ignored()->Arc<dyn Fn(AsrProgress)+Send+Sync>{Arc::new(|_|{})}

#[tokio::test]
async fn frames_wait_for_config_ack_then_finish_follows_every_audio_byte() {
    let (url,server)=server(|mut socket|async move{
        let update=receive(&mut socket).await;assert_eq!(update["type"],"session.update");assert_eq!(update["session"]["sample_rate"],16000);assert!(update["session"]["input_audio_transcription"].get("language").is_none());
        assert!(tokio::time::timeout(Duration::from_millis(30),socket.next()).await.is_err(),"audio sent before provider accepted configuration");
        emit(&mut socket,json!({"type":"session.updated"})).await;
        let first=receive(&mut socket).await;let second=receive(&mut socket).await;let finish=receive(&mut socket).await;
        assert_eq!(first["type"],"input_audio_buffer.append");assert_eq!(STANDARD.decode(first["audio"].as_str().unwrap()).unwrap(),vec![1,0,2,0,3,0]);assert_eq!(STANDARD.decode(second["audio"].as_str().unwrap()).unwrap(),vec![255,127,0,128]);assert_eq!(finish["type"],"session.finish");
        emit(&mut socket,json!({"type":"input_audio_buffer.committed","item_id":"a","previous_item_id":null})).await;
        emit(&mut socket,json!({"type":"conversation.item.input_audio_transcription.text","item_id":"a","text":"如果","stash":"通过"})).await;
        emit(&mut socket,json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"如果通过再发布。"})).await;
        emit(&mut socket,json!({"type":"session.finished"})).await;
    }).await;
    let (tx,rx)=mpsc::channel(3);tx.send(AsrInput::Frame{sequence:0,pcm16:vec![1,2,3],padding_samples:0}).await.unwrap();tx.send(AsrInput::Frame{sequence:1,pcm16:vec![i16::MAX,i16::MIN],padding_samples:0}).await.unwrap();tx.send(AsrInput::Finish{next_sequence:2}).await.unwrap();
    let result=run_local(url,rx,CancellationToken::new(),ignored(),short_timeouts()).await.unwrap();assert_eq!(result[0].text,"如果通过再发布。");server.await.unwrap();
}
#[tokio::test]
async fn config_ack_timeout_is_not_finish_timeout_and_does_not_send_audio() {
    let (url,server)=server(|mut socket|async move{assert_eq!(receive(&mut socket).await["type"],"session.update");assert!(tokio::time::timeout(Duration::from_millis(150),socket.next()).await.is_ok());}).await;
    let (tx,rx)=mpsc::channel(1);tx.send(AsrInput::Frame{sequence:0,pcm16:vec![1],padding_samples:0}).await.unwrap();let mut budgets=short_timeouts();budgets.establishment=Duration::from_millis(80);
    let error=run_local(url,rx,CancellationToken::new(),ignored(),budgets).await.unwrap_err();assert_eq!(error.code,"asr_config_timeout");server.await.unwrap();
}
#[tokio::test]
async fn duplicate_config_ack_only_notifies_once_and_final_ack_needs_finish() {
    let configured=Arc::new(AtomicUsize::new(0));let count=configured.clone();
    let (url,server)=server(|mut socket|async move{receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;emit(&mut socket,json!({"type":"session.updated"})).await;assert_eq!(receive(&mut socket).await["type"],"session.finish");emit(&mut socket,json!({"type":"session.finished"})).await;}).await;
    let (tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();assert!(run_local(url,rx,CancellationToken::new(),Arc::new(move|progress|{if matches!(progress,AsrProgress::Configured){count.fetch_add(1,Ordering::SeqCst);}}),short_timeouts()).await.unwrap().is_empty());assert_eq!(configured.load(Ordering::SeqCst),1);server.await.unwrap();
}
#[tokio::test]
async fn premature_finished_is_rejected_before_local_finish() {
    let (url,server)=server(|mut socket|async move{receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;emit(&mut socket,json!({"type":"session.finished"})).await;}).await;
    let (_tx,rx)=mpsc::channel(1);let error=run_local(url,rx,CancellationToken::new(),ignored(),short_timeouts()).await.unwrap_err();assert_eq!(error.code,"unexpected_finish");server.await.unwrap();
}
#[tokio::test]
async fn finish_ack_deadline_preserves_incomplete_status() {
    let (url,server)=server(|mut socket|async move{receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;assert_eq!(receive(&mut socket).await["type"],"session.finish");std::future::pending::<()>().await;}).await;
    let (tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();let mut budgets=short_timeouts();budgets.finish=Duration::from_millis(80);
    let error=run_local(url,rx,CancellationToken::new(),ignored(),budgets).await.unwrap_err();assert_eq!(error.code,"asr_finish_timeout");server.abort();
}
#[tokio::test]
async fn reordered_partial_replacement_and_duplicate_final_use_one_ordered_source() {
    let captions=Arc::new(Mutex::new(Vec::<Vec<CaptionItem>>::new()));let observed=captions.clone();
    let (url,server)=server(|mut socket|async move{receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;receive(&mut socket).await;
        for event in [json!({"type":"input_audio_buffer.committed","item_id":"b","previous_item_id":"a"}),json!({"type":"input_audio_buffer.committed","item_id":"a","previous_item_id":null}),json!({"type":"conversation.item.input_audio_transcription.text","item_id":"a","text":"今天","stash":"可以"}),json!({"type":"conversation.item.input_audio_transcription.text","item_id":"a","text":"今天","stash":"不可以"}),json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"b","transcript":"明天再看。"}),json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"b","transcript":"明天再看。"}),json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"今天不可以。"}),json!({"type":"session.finished"})] {emit(&mut socket,event).await;}
    }).await;
    let (tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();let rows=run_local(url,rx,CancellationToken::new(),Arc::new(move|progress|{if let AsrProgress::Caption(items)=progress{observed.lock().unwrap().push(items);}}),short_timeouts()).await.unwrap();assert_eq!(rows.iter().map(|row|row.text.as_str()).collect::<Vec<_>>(),["今天不可以。","明天再看。"]);let observations=captions.lock().unwrap();assert_eq!(observations.len(),4);assert_eq!(observations[1][0].text,"今天不可以");assert_eq!(observations.last().unwrap()[0].id,"test-attempt:a");server.await.unwrap();
}
#[tokio::test]
async fn missing_predecessor_cannot_be_hidden_by_complete_timestamps() {
    let (url,server)=server(|mut socket|async move{receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;receive(&mut socket).await;for event in [json!({"type":"input_audio_buffer.speech_started","item_id":"b","audio_start_ms":1000}),json!({"type":"input_audio_buffer.committed","item_id":"b","previous_item_id":"missing"}),json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"b","transcript":"第二句"}),json!({"type":"session.finished"})]{emit(&mut socket,event).await;}}).await;
    let (tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),short_timeouts()).await.unwrap_err().code,"missing_predecessor");server.await.unwrap();
}
#[tokio::test]
async fn cancellation_from_caption_callback_cannot_yield_a_complete_source() {
    let cancel=CancellationToken::new();let callback_cancel=cancel.clone();
    let (url,server)=server(|mut socket|async move{receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;receive(&mut socket).await;emit(&mut socket,json!({"type":"conversation.item.input_audio_transcription.text","item_id":"a","text":"尚未","stash":"完成"})).await;std::future::pending::<()>().await;}).await;
    let (tx,rx)=mpsc::channel(1);tx.send(AsrInput::Finish{next_sequence:0}).await.unwrap();let error=run_local(url,rx,cancel,Arc::new(move|progress|{if matches!(progress,AsrProgress::Caption(_)){callback_cancel.cancel();}}),short_timeouts()).await.unwrap_err();assert_eq!(error.code,"cancelled");server.abort();
}
#[tokio::test]
async fn cancelled_connection_does_not_wait_for_peer_close_or_blocked_send() {
    let (url,server)=server(|mut socket|async move{receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;std::future::pending::<()>().await;}).await;
    let cancel=CancellationToken::new();let later=cancel.clone();let (tx,rx)=mpsc::channel(32);let producer=tokio::spawn(async move{for sequence in 0..4000{if tx.send(AsrInput::Frame{sequence,pcm16:vec![1;1600],padding_samples:0}).await.is_err(){break;}}});
    tokio::spawn(async move{tokio::time::sleep(Duration::from_millis(80)).await;later.cancel();});let mut budgets=short_timeouts();budgets.send=Duration::from_secs(5);
    let error=tokio::time::timeout(Duration::from_secs(1),run_local(url,rx,cancel,ignored(),budgets)).await.unwrap().unwrap_err();assert_eq!(error.code,"cancelled");producer.await.unwrap();server.abort();
}
#[tokio::test]
async fn oversized_frame_never_becomes_a_final_source() {
    let (url,server)=server(|mut socket|async move{receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;std::future::pending::<()>().await;}).await;
    let (tx,rx)=mpsc::channel(1);tx.send(AsrInput::Frame{sequence:0,pcm16:vec![0;1601],padding_samples:0}).await.unwrap();assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),short_timeouts()).await.unwrap_err().code,"audio_frame_limit");server.abort();
}
#[test]
fn terminal_late_events_and_conflicting_timestamp_or_invalid_predecessor_are_rejected() {
    let mut a=Assembler::default();a.ingest(&json!({"type":"session.finished"})).unwrap();assert!(a.ingest(&json!({"type":"conversation.item.input_audio_transcription.text","item_id":"a","text":"late"})).is_err());
    let mut a=Assembler::default();a.ingest(&json!({"type":"input_audio_buffer.speech_started","item_id":"a","audio_start_ms":0})).unwrap();assert!(a.ingest(&json!({"type":"input_audio_buffer.speech_started","item_id":"a","audio_start_ms":1})).is_err());assert!(a.ingest(&json!({"type":"input_audio_buffer.committed","item_id":"b","previous_item_id":42})).is_err());
}

#[tokio::test]
async fn sequence_gap_is_rejected_before_missing_frame_is_sent() {
    let (url,server)=server(|mut socket|async move{
        receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;
        // Dropping the rejected client connection is bounded and never sends append/finish.
        if let Some(Ok(Message::Text(text)))=socket.next().await {panic!("unexpected protocol data: {text}");}
    }).await;
    let (tx,rx)=mpsc::channel(1);tx.send(AsrInput::Frame{sequence:1,pcm16:vec![1;1600],padding_samples:0}).await.unwrap();
    assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),short_timeouts()).await.unwrap_err().code,"audio_incomplete");server.await.unwrap();
}

#[tokio::test]
async fn duplicate_audio_sequence_does_not_append_twice_or_finish() {
    let (url,server)=server(|mut socket|async move{
        receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;
        assert_eq!(receive(&mut socket).await["type"],"input_audio_buffer.append");
        if let Some(Ok(Message::Text(text)))=socket.next().await {panic!("unexpected protocol data: {text}");}
    }).await;
    let (tx,rx)=mpsc::channel(2);
    for _ in 0..2 {tx.send(AsrInput::Frame{sequence:0,pcm16:vec![1;1600],padding_samples:0}).await.unwrap();}
    assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),short_timeouts()).await.unwrap_err().code,"audio_incomplete");server.await.unwrap();
}

#[tokio::test]
async fn padded_tail_keeps_every_pcm_byte_before_finish_receipt() {
    let (url,server)=server(|mut socket|async move{
        receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;
        let frame=receive(&mut socket).await;let bytes=STANDARD.decode(frame["audio"].as_str().unwrap()).unwrap();
        assert_eq!(bytes.len(),3200);assert_eq!(&bytes[..6],&[1,0,2,0,3,0]);assert!(bytes[6..].iter().all(|b|*b==0));
        assert_eq!(receive(&mut socket).await["type"],"session.finish");
        emit(&mut socket,json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"tail","transcript":"保留最后一个词。"})).await;
        emit(&mut socket,json!({"type":"session.finished"})).await;
    }).await;
    let mut pcm=vec![0;1600];pcm[..3].copy_from_slice(&[1,2,3]);let (tx,rx)=mpsc::channel(2);
    tx.send(AsrInput::Frame{sequence:0,pcm16:pcm,padding_samples:1597}).await.unwrap();
    tx.send(AsrInput::Finish{next_sequence:1}).await.unwrap();
    assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),short_timeouts()).await.unwrap()[0].text,"保留最后一个词。");server.await.unwrap();
}

#[tokio::test]
async fn audio_after_padded_tail_cannot_become_complete_source() {
    let (url,server)=server(|mut socket|async move{
        receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;
        assert_eq!(receive(&mut socket).await["type"],"input_audio_buffer.append");
        if let Some(Ok(Message::Text(text)))=socket.next().await {panic!("unexpected protocol data: {text}");}
    }).await;
    let (tx,rx)=mpsc::channel(2);tx.send(AsrInput::Frame{sequence:0,pcm16:vec![0;1600],padding_samples:1}).await.unwrap();
    tx.send(AsrInput::Frame{sequence:1,pcm16:vec![1;1600],padding_samples:0}).await.unwrap();
    assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),short_timeouts()).await.unwrap_err().code,"audio_incomplete");server.await.unwrap();
}

#[tokio::test]
async fn finish_declaring_a_missing_tail_never_sends_session_finish() {
    let (url,server)=server(|mut socket|async move{
        receive(&mut socket).await;emit(&mut socket,json!({"type":"session.updated"})).await;
        assert_eq!(receive(&mut socket).await["type"],"input_audio_buffer.append");
        if let Some(Ok(Message::Text(text)))=socket.next().await {panic!("unexpected protocol data: {text}");}
    }).await;
    let (tx,rx)=mpsc::channel(2);tx.send(AsrInput::Frame{sequence:0,pcm16:vec![1;1600],padding_samples:0}).await.unwrap();
    tx.send(AsrInput::Finish{next_sequence:2}).await.unwrap();
    assert_eq!(run_local(url,rx,CancellationToken::new(),ignored(),short_timeouts()).await.unwrap_err().code,"audio_incomplete");server.await.unwrap();
}

#[test]
fn padding_metadata_and_ten_minute_limit_are_validated_without_transmission() {
    for (pcm,padding) in [(vec![0;1600],1600),(vec![0;3],1),(vec![1;1600],1)] {
        assert_eq!(AudioSequence::default().frame(0,&pcm,padding).unwrap_err().code,"audio_incomplete");
    }
    let mut audio=AudioSequence::default();audio.real_samples=16_000*600;
    assert_eq!(audio.frame(0,&[1],0).unwrap_err().code,"audio_frame_limit");assert_eq!(audio.next,0);
}
