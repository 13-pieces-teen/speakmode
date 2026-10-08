//! Opt-in real Windows delivery acceptance, restricted to our child fixture.
//! Writes known synthetic text to clipboard and the owned edit controls only.
//! No microphone, cloud, credentials, history, or user application text is read.
//! Usage: owned_delivery_probe --fixture-script ABS --scratch-dir ABS --out ABS

#[cfg(windows)]
mod probe {
    use anyhow::{Context, Result, bail, ensure};
    use serde_json::{Value, json};
    use speakmode_lib::platform::delivery::{capture_target, capture_foreground_anchor,
        capture_target_for_anchor, dispatch_once, DeliveryPermit};
    use std::{fs::{self, OpenOptions}, io::Write, path::{Path, PathBuf},
        process::{Child, Command}, thread, time::{Duration, Instant}};
    use std::os::windows::process::CommandExt;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, GetForegroundWindow, GetWindowThreadProcessId,
        GetWindowTextW, IsWindow, SendMessageTimeoutW, SetForegroundWindow,
        SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT};

    const SCENARIOS: [&str; 8] = ["edit", "rich", "readonly", "password", "caret-change",
        "field-change", "anchor-caret-change", "anchor-field-change"];

    struct Args { fixture_script: PathBuf, scratch_dir: PathBuf, out: PathBuf }
    impl Args {
        fn parse() -> Result<Self> {
            let mut fixture_script = None;
            let mut scratch_dir = None;
            let mut out = None;
            let mut args = std::env::args_os().skip(1);
            while let Some(flag) = args.next() {
                let destination = match flag.to_str() {
                    Some("--fixture-script") => &mut fixture_script,
                    Some("--scratch-dir") => &mut scratch_dir,
                    Some("--out") => &mut out,
                    _ => bail!("unknown argument; expected --fixture-script ABS --scratch-dir ABS --out ABS"),
                };
                ensure!(destination.is_none(), "duplicate CLI argument");
                let path = PathBuf::from(args.next().context("missing absolute path argument")?);
                ensure!(path.is_absolute(), "all CLI paths must be absolute");
                *destination = Some(path);
            }
            Ok(Self {
                fixture_script: fixture_script.context("--fixture-script is required")?,
                scratch_dir: scratch_dir.context("--scratch-dir is required")?,
                out: out.context("--out is required")?,
            })
        }
    }

    struct Fixture { child: Child, root: HWND, editor: HWND, second: HWND,
        id: String, scratch_dir: PathBuf }
    impl Fixture {
        fn owned(&mut self) -> bool {
            if !matches!(self.child.try_wait(), Ok(None)) { return false; }
            for hwnd in [self.root, self.editor, self.second] {
                if !unsafe { IsWindow(hwnd) }.as_bool() { return false; }
                let mut pid = 0;
                unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)); }
                if pid != self.child.id() { return false; }
            }
            let mut title = [0u16; 128];
            let length = unsafe { GetWindowTextW(self.root, &mut title) };
            length > 0 && String::from_utf16_lossy(&title[..length as usize])
                == format!("SpeakMode owned native fixture {}", self.id)
        }
        fn active(&mut self) -> bool {
            self.owned() && unsafe { GetForegroundWindow() } == self.root
        }
        fn message(&mut self, hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Result<()> {
            ensure!(self.owned(), "owned fixture identity changed before message");
            ensure!([self.root, self.editor, self.second].contains(&hwnd), "foreign message target rejected");
            let mut result = 0;
            ensure!(unsafe { SendMessageTimeoutW(hwnd, msg, w, l,
                SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT, 500, Some(&mut result)) }.0 != 0,
                "owned fixture message 0x{msg:x} timed out or was rejected");
            Ok(())
        }
        fn read_owned_text(&mut self) -> Result<String> {
            ensure!(self.owned(), "owned fixture identity changed before synthetic readback");
            let mut text = [0u16; 2048];
            self.message(self.editor, 0x000D, WPARAM(text.len()), LPARAM(text.as_mut_ptr() as isize))?;
            let end = text.iter().position(|value| *value == 0).context("owned fixture text exceeded limit")?;
            Ok(String::from_utf16_lossy(&text[..end]).replace("\r\n", "\n").replace('\r', "\n"))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // UUID-scoped exit markers cannot overwrite another run's file.
            if let Ok(mut file) = OpenOptions::new().write(true).create_new(true)
                .open(self.scratch_dir.join(format!("{}-exit", self.id))) {
                let _ = file.write_all(b"exit");
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            if matches!(self.child.try_wait(), Ok(None)) { let _ = self.child.kill(); }
            let _ = self.child.wait();
        }
    }

    fn metadata_hwnd(metadata: &Value, field: &str) -> Result<HWND> {
        let value = metadata[field].as_i64().with_context(|| format!("fixture {field} missing"))?;
        ensure!(value != 0, "fixture {field} must not be null");
        Ok(HWND(value as _))
    }

    fn run_scenario(args: &Args, kind: &str, phase: &mut &'static str) -> Result<Value> {
        *phase = "fixtureStartup";
        let actual_kind = if matches!(kind, "caret-change" | "field-change" |
            "anchor-caret-change" | "anchor-field-change") { "edit" } else { kind };
        let id = uuid::Uuid::new_v4().simple().to_string();
        let windows_dir = std::env::var_os("SystemRoot").context("Windows SystemRoot unavailable")?;
        let powershell = Path::new(&windows_dir).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let child = Command::new(powershell).creation_flags(0x0800_0000)
            .args(["-NoProfile", "-NonInteractive", "-STA", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&args.fixture_script).args(["-FixtureId", &id, "-Kind", actual_kind])
            .arg("-ScratchDir").arg(&args.scratch_dir)
            .arg("-ProbeProcessId").arg(std::process::id().to_string())
            .spawn().context("owned fixture process startup")?;
        // Ordinary foreground-rights handoff to our specific child only.
        // This can fail; the test must not bypass Windows focus policy.
        let child_permission = unsafe { AllowSetForegroundWindow(child.id()) }.is_ok();
        // Establish cleanup ownership before waiting on or parsing metadata.
        let mut fixture = Fixture { child, root: HWND::default(), editor: HWND::default(),
            second: HWND::default(), id, scratch_dir: args.scratch_dir.clone() };
        let ready = args.scratch_dir.join(format!("{}-ready.json", fixture.id));
        let deadline = Instant::now() + Duration::from_secs(8);
        while !ready.is_file() && Instant::now() < deadline {
            ensure!(fixture.child.try_wait()?.is_none(), "owned fixture exited before startup metadata");
            thread::sleep(Duration::from_millis(30));
        }
        let metadata: Value = serde_json::from_slice(&fs::read(&ready).context("owned fixture startup metadata")?)?;
        ensure!(metadata["processId"].as_u64() == Some(u64::from(fixture.child.id())), "fixture PID mismatch");
        ensure!(metadata["fixtureId"].as_str() == Some(fixture.id.as_str()), "fixture UUID mismatch");
        ensure!(metadata["kind"].as_str() == Some(actual_kind), "fixture kind mismatch");
        fixture.root = metadata_hwnd(&metadata, "hwnd")?;
        fixture.editor = metadata_hwnd(&metadata, "editor")?;
        fixture.second = metadata_hwnd(&metadata, "second")?;
        ensure!(fixture.owned(), "fixture window PID/title ownership mismatch");
        *phase = "foregroundReadiness";
        let parent_activation = unsafe { SetForegroundWindow(fixture.root) }.as_bool();
        fixture.message(fixture.root, 0x8031, WPARAM(0), LPARAM(0))?;
        // A newly shown WinForms child can publish its metadata just before
        // Windows completes foreground activation. Wait only for this already
        // verified child, bounded to 500 ms; never inspect or accept another
        // application's active window and never bypass the foreground guard.
        let activation_deadline = Instant::now() + Duration::from_millis(500);
        while !fixture.active() && Instant::now() < activation_deadline {
            ensure!(fixture.owned(), "fixture ownership changed while waiting for activation");
            thread::sleep(Duration::from_millis(20));
        }
        ensure!(fixture.active(), "only our fixture may be the active target");
        *phase = "targetCapture";
        let target = if matches!(kind, "readonly" | "password") { capture_target() } else {
            let anchor = capture_foreground_anchor().context("owned synchronous stop anchor")?;
            ensure!(anchor.process_id == fixture.child.id(), "anchor PID mismatch");
            ensure!(anchor.selection == Some((6, 7)), "anchor selection mismatch");
            ensure!(anchor.focused_hwnd == fixture.editor.0 as usize, "anchor control mismatch");
            if kind == "anchor-caret-change" { fixture.message(fixture.editor, 0x00B1, WPARAM(0), LPARAM(0))?; }
            if kind == "anchor-field-change" { fixture.message(fixture.root, 0x8032, WPARAM(0), LPARAM(0))?; }
            let result = capture_target_for_anchor(&anchor);
            if matches!(kind, "anchor-caret-change" | "anchor-field-change") {
                let error = result.err().context("changed asynchronous stop target was accepted")?;
                ensure!(error.to_string().contains("stop target changed"), "unexpected stop-target rejection");
                ensure!(fixture.read_owned_text()? == "alpha | omega", "rejected anchor changed owned text");
                return Ok(json!({"scenario":kind,"passed":true,"clipboardChanged":false,
                    "inputEvents":0,"asyncTargetChangeRejected":true,
                    "foregroundHandoff":{"probeGrantedChild":child_permission,"childGrantedProbe":metadata["parentForegroundPermissionGranted"],"parentActivationAccepted":parent_activation}}));
            }
            result
        };
        if matches!(kind, "readonly" | "password") {
            let error = target.err().context("protected native target was accepted")?.to_string();
            ensure!(error.contains(if kind == "readonly" { "read-only" } else { "password" }),
                "unexpected protected-target rejection");
            if kind == "readonly" { ensure!(fixture.read_owned_text()? == "alpha | omega", "read-only fixture changed"); }
            // Password contents are never read, even in this synthetic fixture.
            return Ok(json!({"scenario":kind,"passed":true,"clipboardChanged":false,"inputEvents":0,
                "foregroundHandoff":{"probeGrantedChild":child_permission,"childGrantedProbe":metadata["parentForegroundPermissionGranted"],"parentActivationAccepted":parent_activation}}));
        }
        let target = target.context("owned editable target capture")?;
        ensure!(target.process_id == fixture.child.id(), "target PID mismatch");
        ensure!(target.selection == Some((6, 7)), "target selection mismatch");
        ensure!(target.focused_hwnd == fixture.editor.0 as usize, "target control mismatch");
        let permit = DeliveryPermit::from_committed_attempt(format!("owned-fixture-{}", fixture.id), 1);
        if kind == "caret-change" { fixture.message(fixture.editor, 0x00B1, WPARAM(0), LPARAM(0))?; }
        if kind == "field-change" { fixture.message(fixture.root, 0x8032, WPARAM(0), LPARAM(0))?; }
        let text = "SpeakMode 合成测试。\r\n1. 保留否定。";
        *phase = "nativeDispatch";
        let receipt = dispatch_once(&permit, &target, text, || unsafe { GetForegroundWindow() } == fixture.root);
        if matches!(kind, "caret-change" | "field-change") {
            ensure!(receipt.reason_code.as_deref() == Some("targetChangedBeforeDelivery"), "target change not rejected");
            ensure!(receipt.input_events == 0 && !receipt.clipboard_changed, "rejected target received native mutation");
            ensure!(fixture.read_owned_text()? == "alpha | omega", "rejected target changed owned text");
        } else {
            ensure!(receipt.input_events == 4, "expected one four-event paste chord");
            ensure!(receipt.reason_code.as_deref() == Some("inputDispatchedUnconfirmed"), "unexpected dispatch result");
            let expected = format!("alpha {} omega", text.replace("\r\n", "\n"));
            *phase = "ownedReadback";
            let deadline = Instant::now() + Duration::from_secs(2);
            while fixture.read_owned_text()? != expected && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            ensure!(fixture.read_owned_text()? == expected, "controlled owned edit readback did not confirm insertion");
            let duplicate = dispatch_once(&permit, &target, text, || true);
            ensure!(duplicate.input_events == 0 && duplicate.reason_code.as_deref() == Some("attemptConsumed"),
                "duplicate paste attempt was not rejected");
            ensure!(fixture.read_owned_text()? == expected, "duplicate attempt changed owned text");
        }
        Ok(json!({"scenario":kind,"passed":true,"receipt":receipt,
            "foregroundHandoff":{"probeGrantedChild":child_permission,"childGrantedProbe":metadata["parentForegroundPermissionGranted"],"parentActivationAccepted":parent_activation},
            "ownedReadbackConfirmed":!matches!(kind,"caret-change" | "field-change"),
            "repeatPasteRejected":!matches!(kind,"caret-change" | "field-change")}))
    }

    fn run(args: &Args) -> Result<Value> {
        ensure!(args.fixture_script.is_file(), "fixture script does not exist");
        fs::create_dir_all(&args.scratch_dir).context("create owned fixture scratch directory")?;
        let mut receipts = Vec::new();
        for kind in SCENARIOS {
            eprintln!("owned native fixture scenario: {kind}");
            let mut phase = "fixtureStartup";
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_scenario(args, kind, &mut phase)));
            let failure_scope = if matches!(phase,"fixtureStartup"|"foregroundReadiness"){"testPrecondition"}else{"productPath"};
            receipts.push(match result {
                Ok(Ok(receipt)) => receipt,
                Ok(Err(error)) => json!({"scenario":kind,"passed":false,"failurePhase":phase,"failureScope":failure_scope,"failure":format!("{error:#}")}),
                Err(_) => json!({"scenario":kind,"passed":false,"failurePhase":phase,"failureScope":failure_scope,"failure":"unexpected scenario panic"}),
            });
        }
        let passed = receipts.iter().all(|receipt| receipt["passed"] == true);
        Ok(json!({"status":if passed {"PASS"} else {"FAIL"},
            "testPreconditionFailures":receipts.iter().filter(|receipt|receipt["failureScope"]=="testPrecondition").count(),
            "productPathFailures":receipts.iter().filter(|receipt|receipt["failureScope"]=="productPath").count(),
            "scope":"REAL_WINDOWS_OWNED_EDITORS_SYNTHETIC_TEXT_ONLY",
            "microphone":"NOT_USED","cloud":"NOT_USED","userApplicationText":"NOT_READ",
            "fixtureScript":args.fixture_script,"scratchDirectory":args.scratch_dir,"scenarios":receipts}))
    }

    pub fn main() -> i32 {
        let args = match Args::parse() { Ok(args) => args, Err(error) => { eprintln!("{error:#}"); return 2; } };
        // Reserve the output before any window, clipboard, or input mutation.
        // Invalid CLI/output paths cannot run scenarios or overwrite evidence.
        let mut output = match OpenOptions::new().write(true).create_new(true).open(&args.out) {
            Ok(file) => file, Err(error) => { eprintln!("create_new receipt failed: {error}"); return 2; }
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&args)));
        let report = match result {
            Ok(Ok(report)) => report,
            Ok(Err(error)) => json!({"status":"FAIL","scope":"REAL_WINDOWS_OWNED_EDITORS_SYNTHETIC_TEXT_ONLY",
                "failure":format!("{error:#}"),"scenarios":SCENARIOS.map(|scenario| json!({"scenario":scenario,"passed":false,"notRun":true}))}),
            Err(_) => json!({"status":"FAIL","scope":"REAL_WINDOWS_OWNED_EDITORS_SYNTHETIC_TEXT_ONLY",
                "failure":"unexpected probe panic","scenarios":SCENARIOS.map(|scenario| json!({"scenario":scenario,"passed":false,"notRun":true}))}),
        };
        if let Err(error) = serde_json::to_writer_pretty(&mut output, &report)
            .map_err(anyhow::Error::from).and_then(|_| { output.write_all(b"\n")?; output.sync_all()?; Ok(()) }) {
            eprintln!("reserved receipt write failed: {error:#}"); return 2;
        }
        println!("{}", json!({"status":report["status"],"receipt":args.out,"scenarios":8}));
        if report["status"] == "PASS" { 0 } else { 1 }
    }
}

#[cfg(windows)]
fn main() { std::process::exit(probe::main()); }
#[cfg(not(windows))]
fn main() { eprintln!("This opt-in owned editor acceptance requires Windows"); std::process::exit(2); }
