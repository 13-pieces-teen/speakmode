//! User-assisted read-only validation of production's opaque-window adapter.
//! No microphone, database, credentials, text read, clipboard write or input.
#[cfg(windows)]
fn main()->anyhow::Result<()> {
    use anyhow::{ensure,Context};
    use serde_json::json;
    use sha2::{Digest,Sha256};
    use speakmode_lib::platform::{PlatformError,delivery::{capture_compatibility_activity,capture_foreground_anchor,inspect_wechat_window}};
    use std::{fs::{self,OpenOptions},io::Write,path::Path,time::{Duration,Instant}};
    use windows::Win32::{Foundation::HWND,UI::WindowsAndMessaging::{GetForegroundWindow,GetWindowThreadProcessId}};
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    if args==["--help"] {println!("ABSOLUTE_NEW_OUTPUT_JSON EXPECTED_HWND_DECIMAL EXPECTED_PID EXPECTED_PROCESS_CREATION_FILETIME WAIT_SECONDS(1..300)\nRead-only production adapter inspection. Leave the intended WeChat input focused; no clipboard/text/input side effects.");return Ok(());}
    ensure!(args.len()==5,"use --help");
    let out=Path::new(&args[0]);ensure!(out.is_absolute()&&!out.exists(),"fresh absolute output required");
    ensure!(out.parent().is_some_and(Path::is_dir),"output parent must exist");
    let hwnd=args[1].parse::<usize>()?;let pid=args[2].parse::<u32>()?;let created=args[3].parse::<u64>()?;
    let wait=args[4].parse::<u64>()?;ensure!(hwnd!=0&&pid!=0&&created!=0&&(1..=300).contains(&wait),"invalid target identity or wait");
    let mut file=OpenOptions::new().write(true).create_new(true).open(out)?;
    // The production desktop already owns this observer through HotkeyService.
    // This read-only runner installs a nonblocking listener with no tap policy.
    let _listener=handy_keys::KeyboardListener::new()?;
    let hash=|bytes:&[u8]|format!("{:x}",Sha256::digest(bytes));
    let mut result=json!({"tool":"wechat-window-inspection-v1","status":"WAITING","expectedHwnd":hwnd,"expectedPid":pid,
        "expectedProcessCreationFileTime":created.to_string(),"startedAt":chrono::Utc::now().to_rfc3339(),
        "targetText":"NOT_READ","clipboard":"NOT_READ_OR_WRITTEN","inputEvents":0,"focus":"NOT_CHANGED",
        "audio":"NOT_STARTED","userDatabase":"NOT_OPENED","credentials":"NOT_READ","networkRequests":0,
        "productionDeliverySourceSha256":hash(include_bytes!("../src/platform/delivery.rs")),
        "productionAdapterSourceSha256":hash(include_bytes!("../src/platform/delivery_wechat_native.rs")),
        "activitySourceSha256":hash(include_bytes!("../src/platform/vendor/handy-keys/src/platform/windows/activity.rs")),
        "policySourceSha256":hash(include_bytes!("../src/platform/delivery_wechat.rs")),
        "runnerSha256":hash(include_bytes!("wechat_window_inspection.rs")),"executableSha256":hash(&fs::read(std::env::current_exe()?)?),
        "deliveryAuthorized":false,"actualInsertion":"NOT_RUN"});
    let until=Instant::now()+Duration::from_secs(wait);
    let ready=loop {
        let mut owner=0;
        let current=unsafe{GetForegroundWindow()};
        if current==HWND(hwnd as _) {unsafe{GetWindowThreadProcessId(current,Some(&mut owner));}}
        if current==HWND(hwnd as _)&&owner==pid {break true;}
        if Instant::now()>=until {break false;}
        std::thread::sleep(Duration::from_millis(100));
    };
    let code=|error:&PlatformError|match error {
        PlatformError::TargetCapture(reason)=>reason.delivery_reason(),
        PlatformError::TargetHigherIntegrity=>"targetHigherIntegrity",
        _=>"nativeInspectionUnavailable",
    };
    if ready {
        // Let the user's click/key release finish before emulating the stop
        // boundary. Any later activity invalidates this exact original token.
        std::thread::sleep(Duration::from_millis(500));
        let attempt=(||->Result<_,PlatformError>{
            let activity=capture_compatibility_activity().ok_or(PlatformError::TargetCapture(speakmode_lib::platform::TargetCaptureFailure::InputActivityUnavailable))?;
            let anchor=capture_foreground_anchor()?;
            if anchor.hwnd!=hwnd||anchor.process_id!=pid||anchor.process_created_at!=created {
                return Err(PlatformError::TargetCapture(speakmode_lib::platform::TargetCaptureFailure::StopFocusChanged));
            }
            let target=inspect_wechat_window(&anchor,activity)?;
            let again=inspect_wechat_window(&anchor,activity)?;
            Ok((target,again))
        })();
        match attempt {
            Ok((target,again))=>{
                ensure!(target==again,"read-only recapture changed identity");
                ensure!(target.compatibility.is_some()&&target.selection.is_none()&&target.runtime_id.is_empty(),"wrong adapter route");
                result["status"]=json!("CAPTURED_METADATA_ONLY");result["stableRecapture"]=json!(true);
                result["caretVerified"]=json!(false);result["focusedHwnd"]=json!(target.focused_hwnd);
            }
            Err(error)=>{result["status"]=json!("CAPTURE_REJECTED");result["errorCode"]=json!(code(&error));}
        }
    }else{result["status"]=json!("NOT_RUN_WRONG_FOREGROUND");}
    result["finishedAt"]=json!(chrono::Utc::now().to_rfc3339());
    serde_json::to_writer_pretty(&mut file,&result).context("write metadata receipt")?;file.write_all(b"\n")?;file.sync_all()?;
    println!("{}; no input or clipboard/text access",result["status"].as_str().unwrap());Ok(())
}
#[cfg(not(windows))]
fn main(){eprintln!("Windows metadata-only inspection required");std::process::exit(1);}
