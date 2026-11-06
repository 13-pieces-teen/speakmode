//! Backend for the opt-in owned ValuePattern-only test window.
//! The fixture UI starts this process; desktop actions are performed by the
//! tester. This process never changes focus, chooses another app, or retries V.
#[cfg(windows)]
fn main()->anyhow::Result<()> {
    use anyhow::ensure;
    use serde_json::json;
    use speakmode_lib::platform::{delivery,PlatformError};
    use std::{fs::OpenOptions,path::Path,time::{Duration,Instant}};
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(args.len()==5,"expected owned root HWND, field HWND, PID, case, fresh absolute output");
    let root=args[0].parse::<usize>()?;let field=args[1].parse::<usize>()?;let pid=args[2].parse::<u32>()?;
    ensure!(root!=0&&field!=0&&pid!=0&&pid!=std::process::id(),"external owned fixture required");
    ensure!(["writable","custom","scan-code","readonly","password","button"].contains(&args[3].as_str()),"invalid fixture case");
    let writable=["writable","custom","scan-code"].contains(&args[3].as_str());
    let out=Path::new(&args[4]);ensure!(out.is_absolute()&&!out.exists(),"fresh absolute output required");
    let file=OpenOptions::new().write(true).create_new(true).open(out)?;
    let _listener=handy_keys::KeyboardListener::new()?;
    let until=Instant::now()+Duration::from_secs(3);
    let anchor=loop {
        if let Ok(anchor)=delivery::capture_foreground_anchor() {
            if anchor.hwnd==root && anchor.focused_hwnd==field && anchor.process_id==pid {break anchor;}
        }
        ensure!(Instant::now()<until,"owned field did not become focused; no clipboard/input");
        std::thread::sleep(Duration::from_millis(20));
    };
    std::thread::sleep(Duration::from_millis(200));
    let activity=delivery::capture_compatibility_activity().ok_or_else(||anyhow::anyhow!("input monitor unavailable"))?;
    let strict=delivery::capture_target_for_anchor(&anchor);
    let strict_code=strict.as_ref().err().map(|error|match error {
        PlatformError::TargetCapture(reason)=>reason.delivery_reason(), _=>"other",
    });
    let target=delivery::capture_target_for_anchor_with_activity(&anchor,Some(activity));
    let mut report=json!({"schema":"owned-keyboard-delivery-trial-v1","case":args[3],"strictCapture":strict_code,
        "microphone":"NOT_STARTED","networkRequests":0,"userDatabase":"NOT_OPENED","userApps":"NOT_TOUCHED",
        "inputEvents":0,"clipboardChanged":false,"actualInsertion":"UNCONFIRMED"});
    match target {
        Ok(target)=>{
            ensure!(writable,"readonly/password/button target was admitted");
            ensure!(target.compatibility.is_some()&&!target.runtime_id.is_empty()&&target.selection.is_none(),"must exercise the ValuePattern-only route");
            let permit=delivery::DeliveryPermit::from_committed_attempt("owned-synthetic-fixture".into(),1);
            let result=delivery::dispatch_once(&permit,&target,"SpeakMode 中文 English ADHD，2026年10月25日。",||true);
            let repeated=delivery::dispatch_once(&permit,&target,"must never be inserted twice",||true);
            report["receipt"]=json!(result);report["inputEvents"]=json!(result.input_events);
            report["clipboardChanged"]=json!(result.clipboard_changed);report["repeatInputEvents"]=json!(repeated.input_events);
            report["nativeDispatchPassed"]=json!(result.input_events==4 && repeated.input_events==0);
        }
        Err(error)=>{
            let code=match error {PlatformError::TargetCapture(reason)=>reason.delivery_reason(),_=>"other"};
            report["captureError"]=json!(code);
            report["nativeDispatchPassed"]=json!(!writable && code=="targetNotEditable");
            report["actualInsertion"]=json!("NOT_RUN");
        }
    }
    serde_json::to_writer_pretty(file,&report)?;
    ensure!(report["nativeDispatchPassed"]==true,"native fixture check failed; receipt preserved");
    Ok(())
}
#[cfg(not(windows))]
fn main(){eprintln!("Windows owned fixture required");std::process::exit(1);}
