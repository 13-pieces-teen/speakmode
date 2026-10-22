//! Fixed synthetic text only. Paired reviewer/flow timing, at most 58 requests.
//! Only --execute-cloud reads the existing credential. Never reads history/audio,
//! changes preferences, records a microphone, or delivers text to another app.
use speakmode_lib::{cloud::{self,rewrite::{self,semantic_review::{self,MinimumReviewFormat}}},core::{BaseMode,ModeSnapshot,OutputBlock,ProviderConfig,SourceItem,SourceRevision}};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::{fs::{File,OpenOptions},io::Write,path::Path,time::{Duration,Instant}};

struct Case {id:&'static str,mode:&'static str,source:&'static str,draft:&'static str,accept:bool}
const CASES:&[Case]=&[
 Case{id:"chat-clean",mode:"chat",source:"嗯，小林，会议明天下午三点，不对，四点。你方便的话，把会议链接发给我。",draft:"小林，会议明天下午四点。你方便的话，把会议链接发给我。",accept:true},
 Case{id:"technical-terms",mode:"technical",source:"We use Python for AI agents and GraphRAG systems. The API limit is 20 requests per minute.",draft:"We use Python for AI agents and GraphRAG systems. The API limit is 20 requests per minute.",accept:true},
 Case{id:"document-clean",mode:"document",source:"登录页已经完成。支付页还在联调，预计周五完成。回归测试通过以后再上线。",draft:"登录页已经完成。支付页还在联调，预计周五完成。回归测试通过以后再上线。",accept:true},
 Case{id:"scope-clean",mode:"chat",source:"如果周五通过测试，就周一发布。预算仍未确定，先不要对外承诺。",draft:"如果周五通过测试，就周一发布。预算仍未确定，先不要对外承诺。",accept:true},
 Case{id:"coverage-loss",mode:"chat",source:"先检查麦克风，再调整增益，最后记录当前设置。",draft:"先调整增益，最后记录当前设置。",accept:false},
 Case{id:"fact-swap",mode:"chat",source:"场地预算两千元，设备预算三千元。",draft:"场地预算三千元，设备预算两千元。",accept:false},
 Case{id:"scope-loss",mode:"chat",source:"只有测试通过才发布。暂时不要发送邀请，时间还没有确定。",draft:"测试通过，立即发布并发送邀请，时间已经确定。",accept:false},
 Case{id:"intent-loss",mode:"chat",source:"我觉得方案还需要再看一遍。你能帮我检查一下吗？",draft:"方案已经没问题了，我会直接执行。",accept:false},
 Case{id:"cleanup-residue",mode:"chat",source:"嗯，呃，我我今天，嗯，准备准备把测试补上。",draft:"嗯，呃，我我今天，嗯，准备准备把测试补上。",accept:false},
];
const FLOWS:&[Case]=&[
 Case{id:"flow-chat",mode:"chat",source:"那个，小林，会议明天下午三点，呃，不对，四点。你方便的话，把会议链接发给我。",draft:"",accept:true},
 Case{id:"flow-document",mode:"document",source:"今天同步三件事。登录页已经完成。支付页还在联调，预计周五完成。发布前先补一轮回归测试，通过以后再上线。",draft:"",accept:true},
 Case{id:"flow-technical",mode:"technical",source:"这个接口只在测试环境打开，生产环境不要开启。重试最多三次，这不是并发数，并发保持一。",draft:"",accept:true},
 Case{id:"flow-email",mode:"email",source:"小林你好，请帮我确认明天下午四点是否方便开会。测试报告已经整理完了，附件还在检查。你确认之后，我再发会议链接。",draft:"",accept:true},
];
fn mode(case:&Case)->ModeSnapshot{ModeSnapshot{mode_id:case.mode.into(),name:case.mode.into(),base_mode:BaseMode::Auto,rules:String::new(),revision:1}}
fn source(case:&Case)->SourceRevision{SourceRevision{id:format!("source-{}",case.id),session_id:format!("synthetic-{}",case.id),asr_attempt_id:"fixed-text".into(),generation:1,provider:"synthetic".into(),model:"fixed-text".into(),adapter_version:"latency-lab-v1".into(),complete:true,created_at:"fixed".into(),items:vec![SourceItem{id:format!("item-{}",case.id),provider_item_id:"0".into(),capture_order:0,text:case.source.into(),is_final:true,start_ms:None,end_ms:None}]}}
fn draft(case:&Case,source:&SourceRevision)->rewrite::RewriteResult{rewrite::RewriteResult{resolved_mode:BaseMode::Light,blocks:vec![OutputBlock{id:format!("draft-{}",case.id),text:case.draft.into(),source_refs:source.items.iter().map(|item|item.id.clone()).collect()}]}}
fn config(path:&Path)->anyhow::Result<ProviderConfig>{
 anyhow::ensure!(path.is_absolute(),"configuration path must be absolute");
 let db=rusqlite::Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY|rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
 // This is the only query. No dictation/history/audio is read.
 let text:String=db.query_row("SELECT data FROM provider_config WHERE singleton=1",[],|r|r.get(0))?;
 anyhow::ensure!(text.len()<16*1024,"configuration exceeds capacity");
 let config:ProviderConfig=serde_json::from_str(&text)?;cloud::endpoints(&config)?;Ok(config)
}
fn hash(value:&impl serde::Serialize)->String{format!("{:x}",Sha256::digest(serde_json::to_vec(value).unwrap()))}
fn save(path:&Path,value:&Value)->anyhow::Result<()>{std::fs::write(path,serde_json::to_vec_pretty(value)?)?;Ok(())}
fn journal(file:&mut File,value:&Value)->anyhow::Result<()>{serde_json::to_writer(&mut *file,value)?;file.write_all(b"\n")?;file.flush()?;Ok(())}
fn arm(format:MinimumReviewFormat)->&'static str{match format{MinimumReviewFormat::Named=>"named",MinimumReviewFormat::Ordered=>"ordered"}}
async fn call(client:&reqwest::Client,config:&ProviderConfig,secret:&str,body:&Value)->anyhow::Result<Value>{
 let mut request=client.post(cloud::endpoints(config)?.rewrite).bearer_auth(secret).json(body).timeout(Duration::from_secs(20));
 if let Some(workspace)=config.workspace.as_deref().filter(|v|!v.is_empty()){request=request.header("X-DashScope-WorkSpace",workspace);}
 let mut response=request.send().await?;anyhow::ensure!(response.status().is_success(),"HTTP status {}",response.status().as_u16());
 let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await?{anyhow::ensure!(bytes.len()+chunk.len()<=1024*1024,"response capacity exceeded");bytes.extend_from_slice(&chunk);}
 let data:Value=serde_json::from_slice(&bytes)?;
 anyhow::ensure!(data.pointer("/choices/0/finish_reason").and_then(Value::as_str)==Some("stop"),"response not complete");
 let content=data.pointer("/choices/0/message/content").and_then(Value::as_str).ok_or_else(||anyhow::anyhow!("missing content"))?;
 Ok(json!({"content":content,"usage":data.get("usage"),"responseBytes":bytes.len()}))
}
async fn run()->anyhow::Result<()>{
 let args=std::env::args().skip(1).collect::<Vec<_>>();
 let replay=args.len()==6&&args[4]=="--replay-reviews";
 anyhow::ensure!((args.len()==5&&matches!(args[4].as_str(),"--plan-only"|"--execute-cloud")||replay)&&args[0]=="--config-db"&&args[2]=="--out","use --config-db ABS --out ABS_NEW.json --plan-only|--execute-cloud|--replay-reviews ABS_RECEIPT");
 let out=Path::new(&args[3]);anyhow::ensure!(out.is_absolute(),"output path must be absolute");
 let mut output=OpenOptions::new().create_new(true).write(true).open(out)?;
 let mut log=OpenOptions::new().create_new(true).write(true).open(out.with_extension("jsonl"))?;
 let config=config(Path::new(&args[1]))?;
 anyhow::ensure!(config.region.as_deref()==Some("beijing")&&config.rewrite_model=="qwen3.8-flash","paired scope is Beijing qwen3.8-flash only");
 let mut plan=Vec::new();
 // Nine fixed acceptance/rejection cases, followed by four repeats in reverse
 // arm order. First-request connection warming is counterbalanced, not hidden.
 let review_cases=(0..CASES.len()).chain(0..4).collect::<Vec<_>>();
 for (index,&case_index) in review_cases.iter().enumerate(){let c=&CASES[case_index];let s=source(c);let d=draft(c,&s);
  for f in if index%2==0{[MinimumReviewFormat::Named,MinimumReviewFormat::Ordered]}else{[MinimumReviewFormat::Ordered,MinimumReviewFormat::Named]}{
   let r=semantic_review::prepare_minimum_with_format(&config,&mode(c),&s,&d,f)?;
   plan.push(json!({"kind":"review","caseId":c.id,"arm":arm(f),"expectedAccept":c.accept,"sourceSha256":hash(&s),"draftSha256":hash(&d),"requestSha256":hash(&r.body)}));
  }
 }
 let mut receipt=json!({"schemaVersion":1,"tool":"speakmode-dictation-latency-lab","scope":"fixed synthetic text, paired reviewer and full rewrite pipeline; no microphone, ASR, profile writes or host delivery","automaticRetries":0,"maximumRequests":58,"config":{"region":config.region,"model":config.rewrite_model},"reviewPlan":plan,"flowPlan":FLOWS.iter().map(|c|json!({"id":c.id,"mode":c.mode,"sourceSha256":hash(&source(c))})).collect::<Vec<_>>(),"results":[],"requests":0,"status":"planned"});
 serde_json::to_writer_pretty(&mut output,&receipt)?;output.flush()?;drop(output);journal(&mut log,&json!({"kind":"plan","receipt":receipt}))?;
 if args[4]=="--plan-only"{println!("planned: 26 reviewer requests and at most 32 flow requests; no credential/network access");return Ok(());}
 if replay {
  let path=Path::new(&args[5]);anyhow::ensure!(path.is_absolute()&&std::fs::metadata(path)?.len()<=8*1024*1024,"replay requires a bounded absolute receipt");
  let bytes=std::fs::read(path)?;let original:Value=serde_json::from_slice(&bytes)?;
  anyhow::ensure!(original["tool"]=="speakmode-dictation-latency-lab"&&original["config"]==receipt["config"]&&original["reviewPlan"]==receipt["reviewPlan"],"replay plan/configuration must match exactly");
  for row in original["results"].as_array().ok_or_else(||anyhow::anyhow!("missing replay results"))?.iter().filter(|r|r["kind"]=="review") {
   let c=CASES.iter().find(|c|row["caseId"]==c.id).ok_or_else(||anyhow::anyhow!("unknown replay case"))?;
   let f=match row["arm"].as_str(){Some("named")=>MinimumReviewFormat::Named,Some("ordered")=>MinimumReviewFormat::Ordered,_=>anyhow::bail!("unknown replay arm")};
   let s=source(c);let d=draft(c,&s);let r=semantic_review::prepare_minimum_with_format(&config,&mode(c),&s,&d,f)?;
   let content=row["response"]["content"].as_str().ok_or_else(||anyhow::anyhow!("missing replay content"))?;
   let parsed=semantic_review::parse_minimum(content,&r,&mode(c),&s,&d)?;
   let replayed=json!({"caseId":c.id,"arm":arm(f),"decisionUnchanged":row["accepted"]==parsed.accepted(),"expectedAccept":c.accept,"accepted":parsed.accepted()});
   journal(&mut log,&replayed)?;receipt["results"].as_array_mut().unwrap().push(replayed);
  }
  anyhow::ensure!(receipt["results"].as_array().unwrap().len()==26,"replay must cover all original reviewer responses");
  receipt["status"]=json!("replayed_without_network");receipt["originalReceiptSha256"]=json!(format!("{:x}",Sha256::digest(bytes)));save(out,&receipt)?;
  println!("replayed 26 original responses; no credential/network access");return Ok(());
 }
 let secret=match cloud::credentials::read(&config){Ok(value)=>value,Err(error)=>{receipt["status"]=json!("halted");receipt["errorCode"]=json!(error.code);save(out,&receipt)?;return Ok(());}};
 receipt["status"]=json!("running");save(out,&receipt)?;
 let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).pool_max_idle_per_host(2).build()?;
 let mut requests=0;
 for (index,&case_index) in review_cases.iter().enumerate(){let c=&CASES[case_index];let s=source(c);let d=draft(c,&s);
  for f in if index%2==0{[MinimumReviewFormat::Named,MinimumReviewFormat::Ordered]}else{[MinimumReviewFormat::Ordered,MinimumReviewFormat::Named]}{
   let r=semantic_review::prepare_minimum_with_format(&config,&mode(c),&s,&d,f)?;let began=Instant::now();requests+=1;
   receipt["requests"]=json!(requests);save(out,&receipt)?;journal(&mut log,&json!({"kind":"requestStarted","request":requests,"caseId":c.id,"arm":arm(f)}))?;
   let response=match call(&client,&config,&secret,&r.body).await{Ok(value)=>value,Err(_)=>{receipt["status"]=json!("halted");receipt["requests"]=json!(requests);receipt["errorCode"]=json!("request_failed");save(out,&receipt)?;return Ok(());}};
   let elapsed=began.elapsed().as_secs_f64()*1000.;let content=response["content"].as_str().unwrap();let parsed=semantic_review::parse_minimum(content,&r,&mode(c),&s,&d);
   let row=json!({"kind":"review","pair":index,"caseId":c.id,"arm":arm(f),"elapsedMs":elapsed,"expectedAccept":c.accept,"parsed":parsed.is_ok(),"accepted":parsed.as_ref().is_ok_and(|v|v.accepted()),"expectationMet":parsed.as_ref().is_ok_and(|v|v.accepted()==c.accept),"errorCode":parsed.as_ref().err().map(|e|&e.code),"response":response});
   println!("review {} {}: {:.0}ms expected={}",c.id,arm(f),elapsed,row["expectationMet"]);journal(&mut log,&row)?;receipt["results"].as_array_mut().unwrap().push(row);receipt["requests"]=json!(requests);save(out,&receipt)?;
  }
 }
 for (index,c) in FLOWS.iter().enumerate(){let s=source(c);
  for f in if index%2==0{[MinimumReviewFormat::Named,MinimumReviewFormat::Ordered]}else{[MinimumReviewFormat::Ordered,MinimumReviewFormat::Named]}{
   anyhow::ensure!(requests+4<=58,"request budget exhausted");let began=Instant::now();
   let result=rewrite::minimum_pipeline::experiment_with_review_format(&config,&secret,&mode(c),&s,f,Duration::from_secs(20)).await?;
   requests+=result.trace.calls;
   let row=json!({"kind":"flow","pair":index,"caseId":c.id,"arm":arm(f),"elapsedMs":began.elapsed().as_secs_f64()*1000.,"validated":result.result.is_ok(),"errorCode":result.result.as_ref().err().map(|e|&e.code),"output":result.result.as_ref().ok(),"trace":result.trace,"sourceSha256":hash(&s),"sourceUnchanged":s==source(c)});
   println!("flow {} {}: {}ms validated={}",c.id,arm(f),row["elapsedMs"],row["validated"]);journal(&mut log,&row)?;receipt["results"].as_array_mut().unwrap().push(row);receipt["requests"]=json!(requests);save(out,&receipt)?;
  }
 }
 receipt["status"]=json!("completed");save(out,&receipt)?;Ok(())
}
#[tokio::main]async fn main(){if let Err(_)=run().await{eprintln!("Latency lab failed; see bounded local receipt. No request is automatically retried.");std::process::exit(1);}}
