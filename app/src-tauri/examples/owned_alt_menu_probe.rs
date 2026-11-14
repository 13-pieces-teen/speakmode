//! Opt-in Right Alt / native-menu experiment in one separately owned child.
//! Default synthetic mode observes untreated OS menu behavior and production
//! capture/dispatch only. Injected Alt is deliberately ignored by the production
//! hotkey hook: it does NOT establish that physical shortcut handling works.
//! --physical-alt starts the real HotkeyService and waits for two real taps.
//! Neither mode opens a microphone, calls a cloud provider or reads user text.

#[cfg(windows)]
mod probe {
    use anyhow::{Context, Result, bail, ensure};
    use serde_json::{Value, json};
    use speakmode_lib::platform::{delivery::{capture_foreground_anchor,
        capture_target_for_anchor, dispatch_once, DeliveryPermit},
        hotkeys::{HotkeyEvent, HotkeyService}};
    use std::{fs::{self, OpenOptions}, io::Write, mem::size_of,
        os::windows::process::CommandExt, path::{Path, PathBuf}, process::{Child, Command},
        sync::mpsc, thread, time::{Duration, Instant}};
    use windows::Win32::{Foundation::{CloseHandle, ERROR_NO_MORE_FILES, HWND, LPARAM, WPARAM},
        System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW,
            Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        UI::{Input::KeyboardAndMouse::{GetAsyncKeyState, SendInput, INPUT,
            INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
            KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_RMENU},
            WindowsAndMessaging::{AllowSetForegroundWindow, GUITHREADINFO,
                GetForegroundWindow, GetGUIThreadInfo, GetWindowTextW,
                GetWindowThreadProcessId, IsWindow, SendMessageTimeoutW,
                SetForegroundWindow, SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT}}};

    const INSERT: &str = "SpeakMode 菜单回归合成测试。";
    const SYNTHETIC_MARKER: usize = 0x534d_414c;

    #[derive(Debug)]
    struct OwnedPrecondition(&'static str);
    impl std::fmt::Display for OwnedPrecondition {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) }
    }
    impl std::error::Error for OwnedPrecondition {}
    fn require_owned(condition: bool, message: &'static str) -> Result<()> {
        if condition { Ok(()) } else { Err(OwnedPrecondition(message).into()) }
    }

    struct Args { fixture: PathBuf, scratch: PathBuf, out: PathBuf, physical: bool }
    impl Args {
        fn parse() -> Result<Self> {
            let (mut fixture, mut scratch, mut out) = (None, None, None);
            let mut physical = false;
            let mut args = std::env::args_os().skip(1);
            while let Some(flag) = args.next() {
                if flag == "--physical-alt" {
                    ensure!(!physical, "duplicate --physical-alt"); physical = true; continue;
                }
                let slot = match flag.to_str() {
                    Some("--fixture-script") => &mut fixture,
                    Some("--scratch-dir") => &mut scratch,
                    Some("--out") => &mut out,
                    _ => bail!("expected --fixture-script ABS --scratch-dir ABS --out ABS [--physical-alt]"),
                };
                ensure!(slot.is_none(), "duplicate path argument");
                let path = PathBuf::from(args.next().context("missing absolute path")?);
                ensure!(path.is_absolute(), "explicit absolute paths required");
                *slot = Some(path);
            }
            Ok(Self { fixture: fixture.context("--fixture-script required")?,
                scratch: scratch.context("--scratch-dir required")?,
                out: out.context("--out required")?, physical })
        }
    }

    struct Fixture { child: Child, root: HWND, editor: HWND, id: String, scratch: PathBuf }
    impl Fixture {
        fn owned(&mut self) -> bool {
            if !matches!(self.child.try_wait(), Ok(None)) { return false; }
            for hwnd in [self.root, self.editor] {
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
        fn message(&mut self, hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Result<usize> {
            require_owned(self.owned() && [self.root, self.editor].contains(&hwnd), "owned fixture identity changed")?;
            let mut result = 0;
            ensure!(unsafe { SendMessageTimeoutW(hwnd, msg, w, l,
                SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT, 500, Some(&mut result)) }.0 != 0,
                "owned fixture message timed out");
            Ok(result)
        }
        fn state(&mut self) -> Result<Value> {
            require_owned(self.active(), "owned fixture lost foreground")?;
            let mut pid = 0;
            let tid = unsafe { GetWindowThreadProcessId(self.root, Some(&mut pid)) };
            require_owned(pid == self.child.id(), "owned fixture thread identity changed")?;
            let mut gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            unsafe { GetGUIThreadInfo(tid, &mut gui) }.context("owned GUI thread snapshot")?;
            let menu_entries = self.message(self.root, 0x8033, WPARAM(0), LPARAM(0))?;
            let key_menu_requests = self.message(self.root, 0x8035, WPARAM(0), LPARAM(0))?;
            require_owned(self.active(), "owned fixture lost foreground during GUI snapshot")?;
            Ok(json!({"guiFlags":gui.flags.0, "inMenuMode":gui.flags.0 & 0x1c != 0,
                "focusedOwnedEditor":gui.hwndFocus == self.editor,
                "menuEntries":menu_entries,"keyMenuRequests":key_menu_requests}))
        }
        fn reset(&mut self) -> Result<()> {
            require_owned(self.active(), "cannot reset a non-foreground owned fixture")?;
            self.message(self.root, 0x8034, WPARAM(0), LPARAM(0))?;
            let state = self.state()?;
            ensure!(state["focusedOwnedEditor"] == true && state["inMenuMode"] == false,
                "owned fixture focus/menu reset did not complete");
            Ok(())
        }
        fn read_owned_text(&mut self) -> Result<String> {
            let mut text = [0u16; 2048];
            self.message(self.editor, 0x000d, WPARAM(text.len()), LPARAM(text.as_mut_ptr() as isize))?;
            let length = text.iter().position(|c| *c == 0).context("owned synthetic text exceeded bound")?;
            Ok(String::from_utf16_lossy(&text[..length]))
        }
        fn capture_and_dispatch(&mut self, attempt: &str) -> Result<Value> {
            require_owned(self.active(), "owned fixture lost foreground before capture")?;
            let anchor_result = capture_foreground_anchor();
            require_owned(self.active(), "owned fixture lost foreground during anchor capture")?;
            let anchor = anchor_result.context("owned stop anchor")?;
            require_owned(anchor.process_id == self.child.id() && anchor.hwnd == self.root.0 as usize
                && anchor.focused_hwnd == self.editor.0 as usize && anchor.selection == Some((6, 7)),
                "production anchor does not match owned caret")?;
            let target_result = capture_target_for_anchor(&anchor);
            require_owned(self.active(), "owned fixture lost foreground during input inspection")?;
            let target = target_result.context("owned editable capture")?;
            require_owned(target.process_id == self.child.id() && target.hwnd == self.root.0 as usize,
                "production target left owned fixture")?;
            // This probe exercises mechanics; it does not exercise a Store
            // commit. Production coordinator ledger acceptance is separate.
            let permit = DeliveryPermit::from_committed_attempt(format!("owned-alt-{}-{attempt}",self.id), 1);
            let receipt = dispatch_once(&permit, &target, INSERT, ||
                unsafe { GetForegroundWindow() } == self.root);
            require_owned(self.active(), "owned fixture lost foreground during native dispatch")?;
            ensure!(receipt.input_events == 4 && receipt.reason_code.as_deref() == Some("inputDispatchedUnconfirmed"),
                "production native dispatch did not send exactly one paste chord");
            let expected = format!("alpha {INSERT} omega");
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.read_owned_text()? != expected && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            ensure!(self.read_owned_text()? == expected, "owned synthetic readback did not confirm insertion");
            let duplicate = dispatch_once(&permit, &target, INSERT, || true);
            ensure!(duplicate.input_events == 0 && duplicate.reason_code.as_deref() == Some("attemptConsumed"),
                "duplicate native attempt was not rejected");
            ensure!(self.read_owned_text()? == expected, "duplicate attempt changed owned text");
            Ok(json!({"passed":true,"receipt":receipt,"ownedReadbackConfirmed":true,
                "duplicateInputEvents":duplicate.input_events,"productionDispatchState":"Unconfirmed",
                "coordinatorLedger":"NOT_EXERCISED"}))
        }
        fn capture_after_alt(&mut self, state: &Value) -> Result<Value> {
            require_owned(self.active(), "owned fixture lost foreground before post-Alt inspection")?;
            let anchor_result = capture_foreground_anchor();
            require_owned(self.active(), "owned fixture lost foreground during post-Alt anchor capture")?;
            let anchor = match anchor_result {
                Ok(anchor) => anchor,
                Err(error) => return Ok(json!({"accepted":false,"inputInspectionAttempted":false,
                    "reason":error.to_string()})),
            };
            require_owned(anchor.process_id == self.child.id() && anchor.hwnd == self.root.0 as usize,
                "post-Alt anchor left owned fixture")?;
            if state["inMenuMode"] == true || state["focusedOwnedEditor"] != true
                || anchor.focused_hwnd != self.editor.0 as usize {
                // Expected untreated menu/focus effects are metadata only.
                // Never inspect another focused control to discover its text.
                return Ok(json!({"accepted":false,"inputInspectionAttempted":false,
                    "reason":"ownedFixtureMenuOrFocusUnavailable"}));
            }
            require_owned(self.active(), "owned fixture lost foreground before post-Alt input inspection")?;
            let result = capture_target_for_anchor(&anchor);
            require_owned(self.active(), "owned fixture lost foreground during post-Alt input inspection")?;
            match result {
                Ok(target) => {
                    require_owned(target.process_id == self.child.id() && target.hwnd == self.root.0 as usize
                        && target.focused_hwnd == self.editor.0 as usize, "post-Alt target left owned editor")?;
                    Ok(json!({"accepted":true,"inputInspectionAttempted":true,"ownedEditor":true,
                        "selection":target.selection}))
                },
                Err(error) => Ok(json!({"accepted":false,"inputInspectionAttempted":true,"reason":error.to_string()})),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Ok(mut file) = OpenOptions::new().write(true).create_new(true)
                .open(self.scratch.join(format!("{}-exit",self.id))) { let _ = file.write_all(b"exit"); }
            let deadline = Instant::now() + Duration::from_secs(3);
            while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            if matches!(self.child.try_wait(), Ok(None)) { let _ = self.child.kill(); }
            let _ = self.child.wait();
        }
    }

    fn start_fixture(args: &Args) -> Result<Fixture> {
        ensure!(args.fixture.is_file(), "fixture script missing");
        fs::create_dir_all(&args.scratch)?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let powershell = Path::new(&std::env::var_os("SystemRoot").context("Windows SystemRoot unavailable")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut command = Command::new(powershell);
        command.creation_flags(0x0800_0000)
            .args(["-NoProfile","-NonInteractive","-STA","-ExecutionPolicy","Bypass","-File"])
            .arg(&args.fixture).args(["-FixtureId",&id,"-Kind","edit","-NativeMenu","-LifetimeSeconds","100"])
            .arg("-ScratchDir").arg(&args.scratch).arg("-ProbeProcessId").arg(std::process::id().to_string());
        if args.physical { command.arg("-PhysicalAlt"); }
        let child = command.spawn().context("owned menu child startup")?;
        let mut fixture = Fixture { child, root: HWND::default(), editor: HWND::default(), id, scratch:args.scratch.clone() };
        let _ = unsafe { AllowSetForegroundWindow(fixture.child.id()) };
        let ready = args.scratch.join(format!("{}-ready.json",fixture.id));
        let deadline = Instant::now() + Duration::from_secs(8);
        while !ready.is_file() && Instant::now() < deadline {
            ensure!(fixture.child.try_wait()?.is_none(), "owned menu child exited before metadata");
            thread::sleep(Duration::from_millis(30));
        }
        let metadata: Value = serde_json::from_slice(&fs::read(&ready).context("owned menu child metadata")?)?;
        ensure!(metadata["processId"].as_u64() == Some(u64::from(fixture.child.id()))
            && metadata["fixtureId"] == fixture.id && metadata["kind"] == "edit"
            && metadata["nativeMenu"] == true, "owned menu child metadata mismatch");
        fixture.root = HWND(metadata["hwnd"].as_i64().context("fixture root HWND")? as _);
        fixture.editor = HWND(metadata["editor"].as_i64().context("fixture Edit HWND")? as _);
        ensure!(fixture.owned(), "owned menu child identity not verified");
        Ok(fixture)
    }

    fn known_desktop_app_running() -> Result<bool> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }?;
        let result = (|| {
            let mut entry = PROCESSENTRY32W { dwSize:size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            unsafe { Process32FirstW(snapshot, &mut entry) }.context("process inventory unavailable; physical test refused")?;
            loop {
                let length = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..length]).eq_ignore_ascii_case("SpeakMode.exe") { return Ok(true); }
                if let Err(error) = unsafe { Process32NextW(snapshot, &mut entry) } {
                    if error.code() == windows::core::HRESULT::from_win32(ERROR_NO_MORE_FILES.0) { return Ok(false); }
                    return Err(error).context("process inventory incomplete; physical test refused");
                }
            }
        })();
        let _ = unsafe { CloseHandle(snapshot) };
        result
    }

    fn physical_policy(fixture: &mut Fixture, phase: &mut &'static str,
        retained_hook_owners: &mut Vec<HotkeyService>, observed_physical_taps: &mut u32) -> Result<Value> {
        *phase = "physicalHookStartup";
        // A second running SpeakMode could record a real tap. Never terminate
        // it from this tool; physical mode instead refuses to start.
        ensure!(!known_desktop_app_running()?, "quit SpeakMode yourself before the optional physical test; no microphone action attempted");
        let (tx,rx) = mpsc::channel();
        let mut service = match HotkeyService::start("alt_right", Duration::from_secs(1), move |event| {
            if matches!(event,HotkeyEvent::Toggle(_)) { let _ = tx.send(capture_foreground_anchor()); }
        }) {
            Ok(service) => service,
            Err(error) => {
                let (error,owner) = error.into_parts();
                if let Some(mut owner) = owner {
                    if let Err(cleanup_error) = owner.shutdown(Duration::from_secs(6)) {
                        *phase = "physicalHookCleanup";
                        retained_hook_owners.push(owner);
                        return Err(cleanup_error).context("failed-start physical hook owner did not acknowledge shutdown");
                    }
                }
                return Err(error).context("production physical hotkey startup");
            }
        };
        let result = (|| {
            require_owned(fixture.active(), "owned fixture lost foreground before physical taps")?;
            OpenOptions::new().write(true).create_new(true)
                .open(fixture.scratch.join(format!("{}-physical-ready",fixture.id)))?.write_all(b"ready")?;
            eprintln!("Owned fixture ready: press physical Right Alt twice here, two seconds apart. Close other dictation/hotkey apps yourself first.");
            let mut states = Vec::new();
            for tap in 1..=2 {
                *phase = "physicalGestureReadiness";
                let deadline = Instant::now() + Duration::from_secs(30);
                let anchor = loop {
                    require_owned(fixture.active(), "owned fixture lost foreground while waiting for physical tap")?;
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(anchor) => {
                            *observed_physical_taps += 1;
                            *phase = "physicalPolicy";
                            break anchor.context("production stop anchor at physical gesture")?;
                        },
                        Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {},
                        Err(_) => bail!("physical Right Alt gesture not observed; synthetic events cannot satisfy this test"),
                    }
                };
                *phase = "physicalPolicy";
                require_owned(fixture.active() && anchor.process_id == fixture.child.id()
                    && anchor.hwnd == fixture.root.0 as usize, "physical gesture anchor left owned fixture")?;
                ensure!(anchor.process_id == fixture.child.id() && anchor.hwnd == fixture.root.0 as usize
                    && anchor.focused_hwnd == fixture.editor.0 as usize && anchor.selection == Some((6,7)),
                    "physical gesture did not preserve the owned input anchor");
                let state = fixture.state()?;
                ensure!(state["focusedOwnedEditor"] == true && state["inMenuMode"] == false,
                    "physical Right Alt left the owned input in menu mode");
                states.push(json!({"tap":tap,"state":state}));
            }
            *phase = "physicalDispatch";
            let delivery = fixture.capture_and_dispatch("physical")?;
            Ok(json!({"status":"PASSED","passed":true,"gestures":states,"delivery":delivery,
                "events":"PHYSICAL_ONLY","cloud":"NOT_USED","microphone":"NOT_USED"}))
        })();
        if let Err(error) = service.shutdown(Duration::from_secs(6)) {
            *phase = "physicalHookCleanup";
            retained_hook_owners.push(service);
            return Err(error).context("physical hook owner did not acknowledge shutdown");
        }
        result
    }

    fn inject_owned_alt(fixture: &mut Fixture) -> Result<u32> {
        require_owned(fixture.active(), "owned fixture not active before synthetic Alt")?;
        ensure!([0x10,0x11,0x12,0x5b,0x5c].iter().all(|vk|
            unsafe { GetAsyncKeyState(*vk) } as u16 & 0x8000 == 0), "physical modifiers held; no synthetic Alt attempted");
        let input = |up| INPUT { r#type:INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT {
            wVk:VIRTUAL_KEY(VK_RMENU.0), wScan:0x38,
            dwFlags:if up { KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP } else { KEYEVENTF_EXTENDEDKEY },
            dwExtraInfo:SYNTHETIC_MARKER, ..Default::default() } } };
        // One full pair has no opportunity to intentionally hold Alt across a
        // user focus change. This is an OS baseline, never a physical tap.
        let count = unsafe { SendInput(&[input(false),input(true)],size_of::<INPUT>() as i32) };
        if count == 1 { let _ = unsafe { SendInput(&[input(true)],size_of::<INPUT>() as i32) }; }
        ensure!(count == 2, "synthetic Alt pair was not completely accepted");
        Ok(count)
    }

    fn run(args: &Args, phase: &mut &'static str,
        retained_hook_owners: &mut Vec<HotkeyService>, observed_physical_taps: &mut u32) -> Result<Value> {
        if args.physical {
            *phase = "physicalHookStartup";
            ensure!(!known_desktop_app_running()?, "quit SpeakMode yourself before the optional physical test; no microphone action attempted");
        }
        *phase = "fixtureStartup";
        let mut fixture = start_fixture(args)?;
        *phase = "foregroundReadiness";
        let accepted = unsafe { SetForegroundWindow(fixture.root) }.as_bool();
        let deadline = Instant::now() + Duration::from_millis(500);
        while !fixture.active() && Instant::now() < deadline { thread::sleep(Duration::from_millis(20)); }
        require_owned(fixture.active(), "owned child did not become foreground; no input attempted")?;
        *phase = "fixtureReset";
        fixture.reset()?;
        *phase = "directCaptureDispatch";
        let direct = fixture.capture_and_dispatch("direct")?;
        *phase = "fixtureReset";
        fixture.reset()?;
        if args.physical {
            let policy = physical_policy(&mut fixture,phase,retained_hook_owners,observed_physical_taps)?;
            return Ok(json!({"status":"PASSED","passed":true,"mode":"physicalAlt",
                "fixtureId":fixture.id,"fixtureProcessId":fixture.child.id(),"singleChild":true,
                "activationAccepted":accepted,"direct":direct,"physicalPolicy":policy}));
        }
        *phase = "syntheticAltBaseline";
        let before = fixture.state()?;
        let alt_events = inject_owned_alt(&mut fixture)?;
        thread::sleep(Duration::from_millis(200));
        let after = fixture.state()?;
        let capture = fixture.capture_after_alt(&after)?;
        let observed = after["inMenuMode"] == true || after["focusedOwnedEditor"] == false
            || after["menuEntries"].as_u64() > before["menuEntries"].as_u64()
            || after["keyMenuRequests"].as_u64() > before["keyMenuRequests"].as_u64();
        *phase = "fixtureReset";
        fixture.reset()?;
        *phase = "postResetCaptureDispatch";
        let after_reset = fixture.capture_and_dispatch("after-reset")?;
        Ok(json!({"status":"PARTIAL","passed":false,"mechanicsPassed":true,"mode":"syntheticBaseline",
            "fixtureId":fixture.id,"fixtureProcessId":fixture.child.id(),"singleChild":true,
            "activationAccepted":accepted,"direct":direct,"afterFixtureReset":after_reset,
            "untreatedAlt":{"inputEvents":alt_events,"extraInfo":SYNTHETIC_MARKER,
                "before":before,"after":after,"menuSideEffectObserved":observed,"captureAfterAlt":capture},
            "physicalPolicy":{"status":"NOT_RUN","reason":"production hook ignores all injected keyboard events; real physical taps required"},
            "fixtureResetIsProductionRecovery":false}))
    }

    pub fn main() -> i32 {
        let args = match Args::parse() { Ok(args)=>args,Err(error)=>{eprintln!("{error:#}");return 2;} };
        let mut output = match OpenOptions::new().write(true).create_new(true).open(&args.out) {
            Ok(file)=>file,Err(error)=>{eprintln!("receipt create-new failed: {error}");return 2;}
        };
        let mut phase = "fixtureStartup";
        // A cleanup timeout is not resource release. Failed owners keep their
        // join handles until this probe process exits, even after receipt write.
        let mut retained_hook_owners = std::mem::ManuallyDrop::new(Vec::new());
        let mut observed_physical_taps = 0;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
            run(&args,&mut phase,&mut retained_hook_owners,&mut observed_physical_taps)));
        let mut report = match result {
            Ok(Ok(value))=>value,
            Ok(Err(error))=> {
                let precondition=error.downcast_ref::<OwnedPrecondition>().is_some()
                    || matches!(phase,"fixtureStartup"|"foregroundReadiness"|"fixtureReset"|"physicalHookStartup"|"physicalGestureReadiness"|"syntheticAltBaseline");
                let physical_status=if observed_physical_taps == 0 {"NOT_RUN"}
                    else if precondition || phase == "physicalHookCleanup" {"PARTIAL"} else {"FAIL"};
                json!({"status":if precondition && observed_physical_taps > 0 {"PARTIAL"}else if precondition{"NOT_RUN"}else{"FAIL"},"passed":false,
                    "failurePhase":phase,"failureScope":if phase == "physicalHookCleanup"{"toolFailure"}else if precondition{"testPrecondition"}else{"productPath"},
                    "failure":format!("{error:#}"),"physicalPolicy":{"status":physical_status}})
            },
            Err(_)=>json!({"status":"FAIL","passed":false,"failurePhase":phase,
                "failureScope":"toolFailure","failure":"unexpected owned probe panic",
                "physicalPolicy":{"status":if observed_physical_taps > 0 {"PARTIAL"}else{"NOT_RUN"}}}),
        };
        report["scope"]=json!("REAL_WINDOWS_SINGLE_OWNED_CHILD_SYNTHETIC_TEXT_ONLY");
        report["cloud"]=json!("NOT_USED");report["microphone"]=json!("NOT_USED");
        report["userApplicationText"]=json!("NOT_READ");
        report["retainedHookOwnerCountUntilProcessExit"]=json!(retained_hook_owners.len());
        report["observedPhysicalTapCount"]=json!(observed_physical_taps);
        if serde_json::to_writer_pretty(&mut output,&report).is_err() || output.flush().is_err() { return 2; }
        println!("{}",json!({"status":report["status"],"mechanicsPassed":report["mechanicsPassed"],"receipt":args.out}));
        if report["passed"] == true || report["mechanicsPassed"] == true {0}else{1}
    }
}
#[cfg(windows)]
fn main() { std::process::exit(probe::main()); }
#[cfg(not(windows))]
fn main() { eprintln!("owned Alt/menu acceptance requires Windows");std::process::exit(2); }
