//! Real production input operations, confined to a UUID-scoped local browser fixture.
//! The companion driver owns the isolated browser profile and verifies its local
//! URL. This tool independently verifies directory, PID/lifetime, HWND and title.
//! It never launches a browser, records audio, calls a provider or reads passwords.

#[cfg(windows)]
#[path="support/owned_delivery_cases.rs"] mod cases;
#[cfg(windows)]
mod probe {
    use anyhow::{Context, Result, bail, ensure};
    use serde::Deserialize;
    use serde_json::{Value, json};
    use speakmode_lib::platform::{PlatformError, delivery::{capture_foreground_anchor,
        capture_target_for_anchor, dispatch_once, DeliveryPermit}};
    use std::{fs::{self, OpenOptions}, io::Write, path::PathBuf, thread, time::{Duration, Instant}};
    use windows::Win32::{Foundation::{CloseHandle, FILETIME, HWND},
        System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindow, SetForegroundWindow}};

    struct Args { scratch: PathBuf, request: PathBuf, out: PathBuf }
    impl Args {
        fn parse() -> Result<Self> {
            let (mut scratch, mut request, mut out) = (None, None, None);
            let mut values = std::env::args_os().skip(1);
            while let Some(flag) = values.next() {
                let destination = match flag.to_str() {
                    Some("--scratch-dir") => &mut scratch,
                    Some("--request") => &mut request,
                    Some("--out") => &mut out,
                    _ => bail!("expected --scratch-dir ABS --request ABS --out ABS"),
                };
                ensure!(destination.is_none(), "duplicate CLI argument");
                let path = PathBuf::from(values.next().context("missing absolute path")?);
                ensure!(path.is_absolute(), "explicit absolute paths are required");
                *destination = Some(path);
            }
            Ok(Self { scratch: scratch.context("--scratch-dir required")?,
                request: request.context("--request required")?, out: out.context("--out required")? })
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all="camelCase", deny_unknown_fields)]
    struct Request { fixture_id: String, process_id: u32, process_created_at: u64,
        root_hwnd: u64, scenario: String, fixture_file: PathBuf,
        fixture_url: String, profile_directory: PathBuf, alternate_hwnd: Option<u64> }

    fn owned(root: HWND, pid: u32, created: u64, marker: &str) -> bool {
        if !unsafe { IsWindow(root) }.as_bool() { return false; }
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(root, Some(&mut owner)); }
        if owner != pid { return false; }
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else { return false; };
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let accepted = unsafe { GetProcessTimes(process, &mut c, &mut e, &mut k, &mut u) }.is_ok();
        let _ = unsafe { CloseHandle(process) };
        if !accepted || (u64::from(c.dwHighDateTime) << 32 | u64::from(c.dwLowDateTime)) != created { return false; }
        let mut title = [0u16; 256];
        let length = unsafe { GetWindowTextW(root, &mut title) };
        length > 0 && String::from_utf16_lossy(&title[..length as usize]).contains(marker)
    }

    fn run(args: &Args, phase: &mut &'static str) -> Result<Value> {
        *phase = "fixtureIdentity";
        let directory = fs::canonicalize(&args.scratch).context("fixture directory")?;
        let requests = fs::canonicalize(directory.join("requests")).context("owned requests directory")?;
        let request_path = fs::canonicalize(&args.request).context("owned request")?;
        ensure!(request_path.parent() == Some(requests.as_path()), "request outside owned fixture requests directory");
        let request: Request = serde_json::from_slice(&fs::read(&request_path)?)?;
        ensure!(request.fixture_id.len() == 32 && request.fixture_id.bytes().all(|c| c.is_ascii_hexdigit()), "invalid UUID fixture ID");
        ensure!(directory.file_name().and_then(|name| name.to_str()) == Some(request.fixture_id.as_str()), "UUID fixture directory mismatch");
        let (host, variant) = request.scenario.split_once("--").unwrap_or((&request.scenario, ""));
        ensure!(matches!(host, "input" | "textarea" | "contenteditable" | "readonly" | "password" | "noneditable" | "contenteditable-false" | "nested-readonly" | "document"), "unsupported fixture host");
        ensure!(variant.is_empty() || (matches!(host,"textarea"|"contenteditable") && matches!(variant,"emoji"|"long"|"selection"|"cancel"|"window")), "unsupported fixture variant");
        ensure!(request_path.file_name().and_then(|name| name.to_str()) == Some(format!("{}.json", request.scenario).as_str()), "request scenario filename mismatch");
        let fixture_file = fs::canonicalize(&request.fixture_file)?;
        ensure!(fixture_file == fs::canonicalize(directory.join("fixture.html"))?, "HTML outside owned fixture directory");
        ensure!(fs::canonicalize(&request.profile_directory)? == fs::canonicalize(directory.join("profile"))?, "browser profile directory mismatch");
        let url = reqwest::Url::parse(&request.fixture_url)?;
        ensure!(url.scheme() == "file" && url.query().is_none() && url.fragment().is_none(), "only exact local file URL allowed");
        let url_file = url.to_file_path().map_err(|_| anyhow::anyhow!("local URL path unavailable"))?;
        ensure!(fs::canonicalize(url_file)? == fixture_file, "local URL and HTML path mismatch");
        let marker = format!("SpeakMode owned browser {}", request.fixture_id);
        ensure!(fs::read_to_string(fixture_file)?.contains(&format!("<title>{marker}</title>")), "fixture HTML marker mismatch");
        let root = HWND(request.root_hwnd as _);
        ensure!(owned(root, request.process_id, request.process_created_at, &marker), "owned browser PID/lifetime/title mismatch");
        *phase = "foregroundReadiness";
        ensure!(unsafe { GetForegroundWindow() } == root, "owned browser is not foreground; no input attempted");
        let positive = matches!(host, "input" | "textarea" | "contenteditable");
        *phase = "targetCapture";
        let anchor = match capture_foreground_anchor() {
            Ok(anchor) => anchor,
            Err(PlatformError::TargetHigherIntegrity) => return Ok(json!({"scenario":request.scenario,
                "passed":false,"inputEvents":0,"clipboardChanged":false,
                "failurePhase":"targetCapture","failureScope":"testPrecondition",
                "failure":"owned fixture unexpectedly has higher integrity"})),
            Err(error) => return Err(error).context("owned browser stop anchor"),
        };
        ensure!(anchor.process_id == request.process_id && anchor.hwnd == root.0 as usize,
            "production anchor left owned browser");
        let target = match capture_target_for_anchor(&anchor) {
            Ok(target) => target,
            Err(PlatformError::TargetHigherIntegrity) => return Ok(json!({"scenario":request.scenario,
                "passed":false,"captureAccepted":false,"inputEvents":0,"clipboardChanged":false,
                "failurePhase":"targetCapture","failureScope":"testPrecondition",
                "failure":"owned fixture unexpectedly has higher integrity"})),
            Err(error) => return Ok(json!({"scenario":request.scenario,"passed":!positive,
                "captureAccepted":false,"inputEvents":0,"clipboardChanged":false,
                "failurePhase":if positive {Some("targetCapture")} else {None::<&str>},
                "failureScope":if positive {Some("productPath")} else {None::<&str>},
                "captureReason":error.to_string()})),
        };
        ensure!(target.process_id == request.process_id && target.hwnd == root.0 as usize,
            "production target left owned browser");
        if !positive {
            // Never dispatch to an unexpectedly accepted protected fixture.
            return Ok(json!({"scenario":request.scenario,"passed":false,
                "captureAccepted":true,"inputEvents":0,"clipboardChanged":false,
                "failurePhase":"targetCapture","failureScope":"productPath",
                "failure":"protected fixture was accepted; dispatch deliberately withheld"}));
        }
        ensure!(target.selection == Some((6, if variant == "selection" {7} else {6})), "known synthetic selection mismatch");
        let permit = DeliveryPermit::from_committed_attempt(format!("owned-browser-{}-{}", request.fixture_id, request.scenario), 1);
        let text = crate::cases::payload(variant);
        let mut alternate = None;
        if variant == "cancel" { ensure!(permit.cancel_before_injection(), "prepared attempt cancellation failed"); }
        if variant == "window" {
            *phase = "foregroundReadiness";
            let other = HWND(request.alternate_hwnd.context("owned alternate window required")? as _);
            ensure!(other != root && owned(other, request.process_id, request.process_created_at, &format!("{marker} alternate")), "alternate window ownership mismatch");
            let _ = unsafe { SetForegroundWindow(other) };
            let deadline = Instant::now() + Duration::from_millis(500);
            while unsafe { GetForegroundWindow() } != other && Instant::now() < deadline { thread::sleep(Duration::from_millis(20)); }
            ensure!(unsafe { GetForegroundWindow() } == other, "owned alternate window did not become foreground");
            alternate = Some(other);
        }
        *phase = "nativeDispatch";
        let receipt = dispatch_once(&permit, &target, &text, || {
            let active = unsafe { GetForegroundWindow() };
            (active == root || alternate == Some(active)) && owned(root, request.process_id, request.process_created_at, &marker)
                && alternate.is_none_or(|other| owned(other, request.process_id, request.process_created_at, &format!("{marker} alternate")))
        });
        let duplicate = dispatch_once(&permit, &target, &text, || true);
        let blocked = matches!(variant,"cancel"|"window");
        let expected_reason = match variant { "cancel" => "attemptConsumed", "window" => "targetChangedBeforeDelivery", _ => "inputDispatchedUnconfirmed" };
        let passed = receipt.input_events == if blocked {0} else {4}
            && receipt.reason_code.as_deref() == Some(expected_reason) && (!blocked || !receipt.clipboard_changed)
            && duplicate.input_events == 0 && duplicate.reason_code.as_deref() == Some("attemptConsumed");
        Ok(json!({"scenario":request.scenario,"passed":passed,"captureAccepted":true,
            "receipt":receipt,"payloadUtf16Units":text.encode_utf16().count(),"duplicateInputEvents":duplicate.input_events,
            "duplicateReasonCode":duplicate.reason_code,
            "failurePhase":if passed {None::<&str>} else {Some("nativeDispatch")},
            "failureScope":if passed {None::<&str>} else {Some("productPath")},
            "hostReadback":"NOT_PERFORMED_HERE","productionDispatchState":"Unconfirmed"}))
    }

    pub fn main() -> i32 {
        let args = match Args::parse() { Ok(args) => args, Err(error) => { eprintln!("{error:#}"); return 2; } };
        // Reserve an immutable receipt before clipboard or input mutations.
        let mut output = match OpenOptions::new().write(true).create_new(true).open(&args.out) {
            Ok(file) => file, Err(error) => { eprintln!("receipt create-new failed: {error}"); return 2; }
        };
        let mut phase = "fixtureIdentity";
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&args, &mut phase)));
        let report = match result {
            Ok(Ok(report)) => report,
            Ok(Err(error)) => json!({"passed":false,"failurePhase":phase,
                "failureScope":if matches!(phase,"fixtureIdentity"|"foregroundReadiness") {"testPrecondition"} else {"productPath"},"failure":format!("{error:#}")}),
            Err(_) => json!({"passed":false,"failurePhase":phase,"failureScope":"toolFailure","failure":"unexpected probe panic"}),
        };
        if serde_json::to_writer_pretty(&mut output, &report).is_err() || output.flush().is_err() { return 2; }
        println!("{}", json!({"passed":report["passed"],"receipt":args.out,"failurePhase":report["failurePhase"]}));
        if report["passed"] == true { 0 } else { 1 }
    }
}
#[cfg(windows)]
fn main() { std::process::exit(probe::main()); }
#[cfg(not(windows))]
fn main() { eprintln!("owned browser input acceptance requires Windows"); std::process::exit(2); }
