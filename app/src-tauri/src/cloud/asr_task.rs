//! Qwen Audio 3.1 task-streaming transport; capture's public contract stays in asr.
//! Protocol references, verified 2026-10-08:
//! https://help.aliyun.com/zh/model-studio/qwen-audio-asr-streaming-websocket-api
//! https://help.aliyun.com/zh/model-studio/qwen-audio-asr-streaming-client-events
//! https://help.aliyun.com/zh/model-studio/qwen-audio-asr-streaming-server-events
//! https://help.aliyun.com/zh/model-studio/improve-asr-accuracy
use super::{CloudError,asr::{AsrInput,AsrProgress,CaptionItem,AudioSequence,Timeouts,WireDiagnostics,AsrEventKind,authorized_request,send_message,cancelled}};
use crate::core::{DictionarySnapshot,ProviderConfig,SourceItem};
use futures_util::StreamExt;
use serde_json::{Value,json};
use std::{collections::BTreeMap,sync::Arc,time::Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async_with_config,tungstenite::{Message,protocol::WebSocketConfig}};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn invalid_event()->CloudError {CloudError::new("invalid_asr_event","asr","语音识别事件格式无效，已保留已有字幕")}

fn run_task(config:&ProviderConfig,language:&str,dictionary:&DictionarySnapshot,task_id:&str)->Result<Value,CloudError> {
    crate::core::store::validate_dictionary(dictionary).map_err(|_|CloudError::new("dictionary_invalid","configuration","个人词典无效或超过容量，请在词典页检查"))?;
    let mut parameters=json!({"format":"pcm","sample_rate":16000,"heartbeat":true,"vad_model":"near_meeting_16k"});
    if language!="auto" {parameters["language_hints"]=json!([language]);}
    let mut vocabulary=serde_json::Map::new();let mut context_chunks=Vec::<String>::new();let mut chunk=String::new();
    for entry in &dictionary.entries {
        // Provider hotwords: non-ASCII <=15 chars; ASCII <=7 space-separated words.
        // Longer dictionary entries remain domain context, never silently omitted.
        let fits=if entry.term.is_ascii(){entry.term.split_whitespace().count()<=7}else{entry.term.chars().count()<=15};
        if fits {vocabulary.insert(entry.term.clone(),json!(4));continue;}
        let separator=usize::from(!chunk.is_empty());
        if chunk.chars().count()+separator+entry.term.chars().count()>400 {context_chunks.push(std::mem::take(&mut chunk));}
        if !chunk.is_empty(){chunk.push('\n');}chunk.push_str(&entry.term);
    }
    if !chunk.is_empty(){context_chunks.push(chunk);}
    if context_chunks.len()>5 {
        return Err(CloudError::new("dictionary_invalid","configuration","词典中的长词条超过此识别接口的上下文容量；请精简长句，中文热词建议不超过15字，英文词组不超过7词"));
    }
    if !vocabulary.is_empty(){parameters["vocabulary"]=Value::Object(vocabulary);}
    let mut input=json!({});
    if !context_chunks.is_empty(){input["context"]=json!(context_chunks.into_iter().map(|text|json!({"role":"user","content":[{"type":"input_text","text":text}]})).collect::<Vec<_>>());}
    Ok(json!({"header":{"action":"run-task","task_id":task_id,"streaming":"duplex"},
        "payload":{"task_group":"audio","task":"asr","function":"recognition","model":config.asr_model,"parameters":parameters,"input":input}}))
}

#[derive(Default)]
struct TaskSentence {text:String,revision:u64,finalized:bool,start:u64,end:Option<u64>}
#[derive(Default)]
struct TaskAssembler {sentences:BTreeMap<u64,TaskSentence>}
impl TaskAssembler {
    fn ingest(&mut self,value:&Value)->Result<(bool,bool,u64),CloudError> {
        let id=value["sentence_id"].as_u64().filter(|id|(1..=500).contains(id)).ok_or_else(invalid_event)?;
        let text=value["text"].as_str().ok_or_else(invalid_event)?;
        let finalized=value["sentence_end"].as_bool().ok_or_else(invalid_event)?;
        let start=value["begin_time"].as_u64().filter(|time|*time<=600_100).ok_or_else(invalid_event)?;
        let end=match value.get("end_time") {None|Some(Value::Null) if !finalized=>None,Some(value)=>Some(value.as_u64().filter(|time|*time>=start&&*time<=600_100).ok_or_else(invalid_event)?),_=>return Err(invalid_event())};
        if text.len()>240_000 {return Err(CloudError::new("asr_limit","asr","识别段落超过容量限制"));}
        let first=!self.sentences.contains_key(&id);
        let sentence=self.sentences.entry(id).or_default();
        if sentence.finalized {
            if finalized&&(sentence.text!=text||sentence.start!=start||sentence.end!=end){return Err(CloudError::new("conflicting_final","asr","重复最终识别结果不一致，不能自动交付"));}
            return Ok((false,false,id));
        }
        let changed=sentence.text!=text||finalized;
        if changed {sentence.revision+=1;}
        // Interim timestamps are provisional. Only sentence_end freezes them.
        sentence.text=text.to_owned();sentence.finalized=finalized;sentence.start=start;sentence.end=end;
        if self.sentences.values().map(|row|row.text.chars().count()).sum::<usize>()>80_000 {return Err(CloudError::new("asr_limit","asr","识别字幕超过容量限制"));}
        Ok((changed,first,id))
    }
    fn captions(&self,attempt:&str)->Vec<CaptionItem> {
        self.sentences.iter().map(|(id,row)|CaptionItem{id:format!("{attempt}:sentence-{id}"),text:row.text.clone(),revision:row.revision,is_final:row.finalized,capture_order:id-1}).collect()
    }
    fn finish(&self,attempt:&str)->Result<Vec<SourceItem>,CloudError> {
        if self.sentences.values().any(|row|!row.finalized) {return Err(CloudError::new("asr_incomplete","asr","尚未收齐所有最终识别段落，可复制已有字幕"));}
        let mut previous_start=None;
        for (index,(id,row)) in self.sentences.iter().enumerate() {
            if *id!=index as u64+1 {return Err(CloudError::new("missing_predecessor","asr","识别段落缺失，已保留已有字幕"));}
            if previous_start.is_some_and(|time|row.start<time) {return Err(CloudError::new("conflicting_order","asr","识别段落顺序与时间不一致"));}
            previous_start=Some(row.start);
        }
        Ok(self.sentences.iter().filter(|(_,row)|!row.text.trim().is_empty()).map(|(id,row)|SourceItem {
            id:format!("{attempt}:sentence-{id}"),provider_item_id:format!("sentence-{id}"),capture_order:id-1,
            text:row.text.clone(),is_final:true,start_ms:Some(row.start),end_ms:row.end}).collect())
    }
    fn diagnostic(&self,diagnostics:&mut WireDiagnostics,kind:AsrEventKind,id:Option<u64>) {
        let finals=self.sentences.values().filter(|row|row.finalized).count();
        diagnostics.event_summary(kind,self.sentences.len()-finals,finals,id.map(|n|n-1),id.and_then(|n|n.checked_sub(2)));
    }
}

// Only asr::run selects production URLs. This private seam permits loopback wire tests.
pub(super) async fn run_at(config:ProviderConfig,secret:String,language:String,dictionary:DictionarySnapshot,
    attempt:String,mut audio:mpsc::Receiver<AsrInput>,cancel:CancellationToken,callback:Arc<dyn Fn(AsrProgress)+Send+Sync>,
    url:String,timeouts:Timeouts)->Result<Vec<SourceItem>,CloudError> {
    let model=super::models::validate_recognition(&config,&language)?;
    if model.id!="qwen-audio-3.1-asr-flash-streaming" {return Err(CloudError::new("asr_adapter_mismatch","configuration","语音模型与接口版本不匹配"));}
    let task_id=Uuid::new_v4().to_string();let start=run_task(&config,&language,&dictionary,&task_id)?;
    let mut diagnostics=WireDiagnostics::new(Arc::clone(&callback));
    diagnostics.dictionary(&config, &dictionary,
        start.pointer("/payload/parameters/vocabulary").and_then(Value::as_object).map_or(0, |v| v.len()),
        start.pointer("/payload/input/context").and_then(Value::as_array).map_or(0, |v| v.len()))?;
    let connect_deadline=tokio::time::Instant::now()+timeouts.establishment;
    let request=authorized_request(&url,&secret,config.workspace.as_deref())?;
    let mut ws_config=WebSocketConfig::default();ws_config.max_message_size=Some(1024*1024);ws_config.max_frame_size=Some(1024*1024);
    let connect=connect_async_with_config(request,Some(ws_config),super::asr::DISABLE_NAGLE);
    let(mut socket,_)=tokio::select!{biased;
        _=cancel.cancelled()=>return Err(cancelled()),
        result=tokio::time::timeout_at(connect_deadline,connect)=>result.map_err(|_|CloudError::new("asr_connect_timeout","asr","语音服务连接超时"))?.map_err(|_|CloudError::new("asr_connect_failed","asr","语音服务连接失败，请检查地域与 Key"))?,
    };
    send_message(&mut socket,Message::Text(start.to_string()),&cancel,connect_deadline.min(tokio::time::Instant::now()+timeouts.send)).await?;
    diagnostics.dictionary_written();
    let mut assembler=TaskAssembler::default();let mut audio_sequence=AudioSequence::default();
    let mut configured=false;let mut finish_deadline=None;
    loop {
        let deadline=finish_deadline.unwrap_or(connect_deadline);let finishing=finish_deadline.is_some();
        if cancel.is_cancelled(){return Err(cancelled());}
        let timed_out=||if finishing {CloudError::new("asr_finish_timeout","asr","语音服务未完成最终确认，保留已有字幕")}else{CloudError::new("asr_config_timeout","asr","语音服务未确认配置，已停止录音")};
        if (!configured||finishing)&&tokio::time::Instant::now()>=deadline{return Err(timed_out());}
        tokio::select!{biased;
            _=cancel.cancelled()=>return Err(cancelled()),
            _=tokio::time::sleep_until(deadline),if !configured||finishing=>return Err(timed_out()),
            incoming=socket.next()=>{
                let message=incoming.ok_or_else(||CloudError::new("asr_disconnected","asr","语音服务提前断开，结果不完整"))?.map_err(|_|CloudError::new("asr_disconnected","asr","语音连接中断"))?;
                match message {
                    Message::Text(text)=>{
                        let value:Value=match serde_json::from_str(&text) {Ok(value)=>value,Err(_)=>{assembler.diagnostic(&mut diagnostics,AsrEventKind::InvalidJson,None);return Err(invalid_event());}};
                        if value["header"]["task_id"].as_str()!=Some(task_id.as_str()) {assembler.diagnostic(&mut diagnostics,AsrEventKind::Unknown,None);return Err(invalid_event());}
                        match value["header"]["event"].as_str() {
                            Some("task-started")=>{if !configured {configured=true;callback(AsrProgress::Configured);}assembler.diagnostic(&mut diagnostics,AsrEventKind::SessionUpdated,None);},
                            Some("task-failed")=>{assembler.diagnostic(&mut diagnostics,AsrEventKind::ServiceError,None);return Err(CloudError::new("asr_service_error","asr","语音服务返回错误，请检查 Key、地域、模型权限或额度"));},
                            Some("result-generated")=>{
                                if !configured {assembler.diagnostic(&mut diagnostics,AsrEventKind::Unknown,None);return Err(invalid_event());}
                                let sentence=&value["payload"]["output"]["sentence"];
                                if sentence["heartbeat"].as_bool()==Some(true){assembler.diagnostic(&mut diagnostics,AsrEventKind::Unknown,None);continue;}
                                let (mut changed,first,id)=assembler.ingest(sentence)?;
                                if first {callback(AsrProgress::SpeechStart);}
                                let kind=if sentence["sentence_end"]==true {AsrEventKind::Final}else{AsrEventKind::Partial};
                                assembler.diagnostic(&mut diagnostics,kind,Some(id));
                                // Audio 3.1 can include a nonempty next-sentence stash. It is only
                                // a partial, and cannot become source without its own final event.
                                if let Some(stash)=sentence.get("stash").filter(|v|!v.is_null()) {
                                    if !stash.is_object(){return Err(invalid_event());}
                                    let stash_text=stash["text"].as_str().ok_or_else(invalid_event)?;
                                    if !stash_text.is_empty(){
                                        let stash_id=stash["sentence_id"].as_u64().filter(|next|*next==id+1).ok_or_else(invalid_event)?;
                                        let (stash_changed,stash_first,_)=assembler.ingest(&json!({"sentence_id":stash_id,"text":stash_text,"begin_time":stash["begin_time"],"end_time":null,"sentence_end":false}))?;
                                        changed|=stash_changed;if stash_first{callback(AsrProgress::SpeechStart);}
                                        assembler.diagnostic(&mut diagnostics,AsrEventKind::Partial,Some(stash_id));
                                    }
                                }
                                if changed {callback(AsrProgress::Caption(assembler.captions(&attempt)));}
                            },
                            Some("task-finished")=>{
                                assembler.diagnostic(&mut diagnostics,AsrEventKind::SessionFinished,None);
                                if !finishing {return Err(CloudError::new("unexpected_finish","asr","语音服务提前结束，请重新口述"));}
                                if cancel.is_cancelled(){return Err(cancelled());}return assembler.finish(&attempt);
                            },
                            Some(_)=>assembler.diagnostic(&mut diagnostics,AsrEventKind::Unknown,None),
                            None=>{assembler.diagnostic(&mut diagnostics,AsrEventKind::MissingType,None);return Err(invalid_event());}
                        }
                    },
                    Message::Ping(data)=>{let limit=tokio::time::Instant::now()+timeouts.send;send_message(&mut socket,Message::Pong(data),&cancel,if !configured||finishing{deadline.min(limit)}else{limit}).await?;},
                    Message::Close(_)=>return Err(CloudError::new("asr_disconnected","asr","语音服务未完成最终回执")),
                    Message::Binary(_)=>return Err(invalid_event()),
                    _=>{},
                }
            },
            frame=audio.recv(),if configured&&!finishing=>{
                let mut counts=None;let mut finish=false;
                let message=match frame {
                    Some(AsrInput::Frame{sequence,pcm16,padding_samples})=>{
                        let padding=padding_samples.min(pcm16.len());counts=Some((pcm16.len()-padding,padding));
                        diagnostics.metadata.dequeued.add_frame(pcm16.len()-padding,padding);diagnostics.publish();
                        audio_sequence.frame(sequence,&pcm16,padding_samples)?;
                        Message::Binary(pcm16.into_iter().flat_map(i16::to_le_bytes).collect())
                    },
                    Some(AsrInput::Finish{next_sequence})=>{
                        diagnostics.metadata.finish_dequeued_at=Some(Instant::now());diagnostics.metadata.finish_expected_frames=Some(next_sequence);diagnostics.publish();
                        audio_sequence.finish(next_sequence)?;finish_deadline=Some(tokio::time::Instant::now()+timeouts.finish);finish=true;
                        Message::Text(json!({"header":{"action":"finish-task","task_id":task_id,"streaming":"duplex"},"payload":{"input":{}}}).to_string())
                    },
                    None=>return Err(CloudError::new("audio_disconnected","asr","采音队列未正常收尾")),
                };
                send_message(&mut socket,message,&cancel,finish_deadline.unwrap_or(tokio::time::Instant::now()+timeouts.send).min(tokio::time::Instant::now()+timeouts.send)).await?;
                if let Some((real,padding))=counts {diagnostics.metadata.socket_written.add_frame(real,padding);diagnostics.metadata.last_frame_written_at=Some(Instant::now());}
                if finish {diagnostics.metadata.finish_written_at=Some(Instant::now());}diagnostics.publish();
            }
        }
    }
}

#[cfg(test)]
#[path="asr_task_tests.rs"]
mod tests;
