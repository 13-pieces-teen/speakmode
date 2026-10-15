"""Opt-in real Windows input test, confined to a new isolated Edge/local fixture.

Requires psutil and Playwright in the caller's Python runtime. No browser download,
microphone, cloud request, user profile, password readback or user-page inspection.
Use explicit --binary ABS --edge ABS --scratch-dir ABS --out ABS.
Use --context with owned_browser_context_probe for read-only input-position QA.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time
import uuid

SCENARIOS = ["input", "textarea", "contenteditable", "readonly", "password",
             "noneditable", "contenteditable-false", "nested-readonly", "document"]
SCENARIOS += [host + "--" + variant for host in ("textarea", "contenteditable")
              for variant in ("emoji", "long", "selection", "cancel", "window")]
CONTEXT_SCENARIOS = SCENARIOS[:9] + [host + "--" + variant for host in ("textarea", "contenteditable")
                                  for variant in ("chat", "document", "empty", "emoji", "long", "selection", "window")]
POSITIVE = {"input", "textarea", "contenteditable"}
INSERT = "SpeakMode 合成测试。"


def payload(variant):
    if variant == "emoji":
        return "SpeakMode 中文 English 👩🏽‍💻 🚀 e\u0301，保留否定。"
    if variant == "long":
        return "\n".join(f"{line:02}. SpeakMode 中文 English 👩🏽‍💻；预算 3500，不删除原始录音。" for line in range(1, 41))
    return INSERT


class ToolFailure(RuntimeError):
    """The acceptance harness could not run; not a product-path failure."""


def process_connections(process, expected_created):
    # Older psutil exposes Process.connections instead of net_connections.
    # Both query only the already verified owned PID; do not scan other apps.
    if process.create_time() != expected_created:
        raise RuntimeError("owned connection-query PID lifetime changed")
    alive = getattr(process, "is_running", None)
    if callable(alive) and not alive():
        raise RuntimeError("owned connection-query PID exited or was reused")
    for name in ("net_connections", "connections"):
        query = getattr(process, name, None)
        if callable(query):
            connections = list(query(kind="tcp"))
            if process.create_time() != expected_created:
                raise RuntimeError("owned connection-query PID lifetime changed")
            if callable(alive) and not alive():
                raise RuntimeError("owned connection-query PID exited or was reused")
            return name, connections
    raise ToolFailure("psutil has neither Process.net_connections nor Process.connections")


def error_scope(error, default):
    return "toolFailure" if isinstance(error, (ToolFailure, AttributeError, ImportError,
                                              json.JSONDecodeError)) else default


def profile_processes(psutil, edge, profile, launched_at):
    """Find only newly created exact-profile Edge owners, including reparenting."""
    profile_arg = f"--user-data-dir={profile}".casefold()
    matches = []
    for candidate in psutil.process_iter():
        try:
            process = psutil.Process(candidate.pid)
            created = process.create_time()
            if created < launched_at or Path(process.exe()).resolve() != edge:
                continue
            # Never print or retain arguments from an unrelated process.
            if profile_arg not in [argument.casefold() for argument in process.cmdline()]:
                continue
            if not process.is_running() or psutil.Process(process.pid).create_time() != created:
                continue
            matches.append((process, created))
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    return matches


def absolute(value):
    path = Path(value)
    if not path.is_absolute():
        raise argparse.ArgumentTypeError("explicit absolute paths required")
    return path.resolve()


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_new(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=absolute)
    parser.add_argument("--edge", required=True, type=absolute)
    parser.add_argument("--scratch-dir", required=True, type=absolute)
    parser.add_argument("--out", required=True, type=absolute)
    parser.add_argument("--context", action="store_true", help="Run the read-only context probe against the owned fixture")
    args = parser.parse_args()
    scenarios = CONTEXT_SCENARIOS if args.context else SCENARIOS
    # An existing output cannot launch a window or mutate the clipboard.
    with args.out.open("x", encoding="utf-8") as receipt:
        report = {"scope": "REAL_WINDOWS_OWNED_BROWSER_SYNTHETIC_TEXT_ONLY",
                  "cloud": "NOT_USED", "microphone": "NOT_USED",
                  "passwordText": "NOT_READ", "userApplicationText": "NOT_READ",
                  "productionDispatchState": "NOT_ATTEMPTED" if args.context else "Unconfirmed", "scenarios": [],
                  "registeredScenarios": scenarios, "startedUnix": time.time(),
                  "cleanup": {"completed": False}, "passed": False}
        browser_process = browser = owner = playwright = None
        root_created = None
        launched_at = None
        known_processes = {}
        phase = "fixtureStartup"
        try:
            if os.name != "nt":
                raise RuntimeError("real owned browser acceptance requires Windows")
            if not args.binary.is_file() or not args.edge.is_file():
                raise RuntimeError("explicit native binary and Edge executable must exist")
            import psutil
            from playwright.sync_api import sync_playwright
            report["psutilVersion"] = getattr(psutil, "__version__", "unavailable")
            repo = Path(__file__).resolve().parents[1]
            source_paths = [repo / "app/src-tauri/src/platform/delivery.rs",
                            repo / "app/src-tauri/src/platform/mod.rs",
                            repo / ("app/src-tauri/examples/owned_browser_context_probe.rs" if args.context else "app/src-tauri/examples/owned_browser_delivery_probe.rs"),
                            repo / "app/src-tauri/examples/support/owned_delivery_cases.rs",
                            Path(__file__).resolve()]
            source_hashes = {str(path.relative_to(repo)): sha(path) for path in source_paths}
            report.update({"sourceInputs": source_hashes, "binary": str(args.binary),
                           "binarySha256": sha(args.binary), "edge": str(args.edge)})
            args.scratch_dir.mkdir(parents=True, exist_ok=True)
            fixture_id = uuid.uuid4().hex
            fixture = args.scratch_dir / fixture_id
            fixture.mkdir(exist_ok=False)
            profile = fixture / "profile"
            profile.mkdir(exist_ok=False)
            requests = fixture / "requests"
            requests.mkdir(exist_ok=False)
            html = fixture / "fixture.html"
            marker = f"SpeakMode owned browser {fixture_id}"
            html_text = f'''<!DOCTYPE html><meta charset="utf-8"><title>{marker}</title>
<style>body{{font:18px sans-serif;margin:30px}}input,textarea,[contenteditable],.field{{display:block;width:550px;min-height:32px;margin:10px 0}}textarea{{height:55px}}</style>
<input id="input" value="alpha | omega"><textarea id="textarea">alpha | omega</textarea>
<div id="contenteditable" contenteditable="true">alpha | omega</div>
<input id="readonly" readonly value="alpha | omega">
<input id="password" type="password" value="known synthetic password">
<div id="noneditable" class="field" tabindex="0">alpha | omega</div>
<div id="contenteditable-false" class="field" contenteditable="false" tabindex="0">alpha | omega</div>
<div contenteditable="true"><span id="nested-readonly" contenteditable="false" tabindex="0">alpha | omega</span></div>
<div class="field">Click only the named synthetic fields.</div>'''
            with html.open("x", encoding="utf-8") as stream:
                stream.write(html_text)
            fixture_url = html.as_uri()
            report.update({"fixtureId": fixture_id, "fixtureDirectory": str(fixture),
                           "fixtureUrl": fixture_url, "fixtureSha256": sha(html)})
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            command = [str(args.edge), f"--user-data-dir={profile}",
                       f"--remote-debugging-port={port}", "--remote-debugging-address=127.0.0.1",
                       "--no-first-run", "--no-default-browser-check", "--disable-extensions",
                       "--disable-background-networking", "--window-size=920,850",
                       "--window-position=150,70", fixture_url]
            launched_at = time.time()
            browser_process = subprocess.Popen(command, stdout=subprocess.DEVNULL,
                                               stderr=subprocess.DEVNULL,
                                               creationflags=subprocess.CREATE_NO_WINDOW)
            user32 = ctypes.WinDLL("user32", use_last_error=True)
            user32.GetForegroundWindow.restype = ctypes.c_void_p
            user32.SetForegroundWindow.argtypes = [ctypes.c_void_p]
            user32.SetForegroundWindow.restype = ctypes.c_bool
            user32.AllowSetForegroundWindow.argtypes = [ctypes.c_ulong]
            user32.AllowSetForegroundWindow.restype = ctypes.c_bool
            user32.GetWindowThreadProcessId.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
            user32.IsWindow.argtypes = [ctypes.c_void_p]
            user32.IsWindow.restype = ctypes.c_bool
            user32.GetWindowTextW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_int]
            user32.GetWindowTextW.restype = ctypes.c_int
            user32.IsWindowVisible.argtypes = [ctypes.c_void_p]
            user32.IsWindowVisible.restype = ctypes.c_bool
            # Specific owned child only; no ASFW_ANY, fake Alt, AttachThreadInput
            # or foreground-policy bypass. Denial remains a recorded precondition.
            granted_child = bool(user32.AllowSetForegroundWindow(browser_process.pid))
            try:
                root_process = psutil.Process(browser_process.pid)
                created = root_process.create_time()
                if created >= launched_at and Path(root_process.exe()).resolve() == args.edge \
                        and f"--user-data-dir={profile}".casefold() in [arg.casefold() for arg in root_process.cmdline()] \
                        and root_process.is_running():
                    root_created = created
                    known_processes[root_process.pid] = root_created
            except psutil.NoSuchProcess:
                # Edge's broker may exit after handing its isolated profile to
                # a new process. An exited launch PID is not a product failure.
                pass
            report.update({"launchPid": browser_process.pid, "browserLaunchedUnix": launched_at,
                           "cdpPort": port, "ownerDiscovery": "exactNewUuidProfileExeCreationTimeAndCdpListener"})
            deadline = time.monotonic() + 12
            while time.monotonic() < deadline:
                for process, created in profile_processes(psutil, args.edge, profile, launched_at):
                    try:
                        known_processes[process.pid] = created
                        connection_api, connections = process_connections(process, created)
                        report["processConnectionsApi"] = connection_api
                        if any(connection.laddr and connection.laddr.port == port
                               and connection.laddr.ip == "127.0.0.1"
                               and connection.status == psutil.CONN_LISTEN
                               for connection in connections):
                            owner = process
                            break
                    except (psutil.NoSuchProcess, psutil.AccessDenied):
                        continue
                if owner is not None:
                    break
                time.sleep(.05)
            if owner is None:
                raise RuntimeError("isolated exact-profile CDP owner was not verified")
            owner_created = psutil.Process(owner.pid).create_time()
            known_processes[owner.pid] = owner_created
            granted_owner = bool(user32.AllowSetForegroundWindow(owner.pid))

            def verify_owner():
                if psutil.Process(owner.pid).create_time() != owner_created or not owner.is_running():
                    raise RuntimeError("owned browser PID lifetime changed")
                if f"--user-data-dir={profile}".casefold() not in [arg.casefold() for arg in owner.cmdline()]:
                    raise RuntimeError("owned browser profile command line changed")
                if Path(owner.exe()).resolve() != args.edge:
                    raise RuntimeError("owned browser executable mismatch")

            playwright = sync_playwright().start()
            browser = playwright.chromium.connect_over_cdp(f"http://127.0.0.1:{port}", timeout=12000)
            pages = [page for context in browser.contexts for page in context.pages if page.url == fixture_url]
            if len(pages) != 1:
                raise RuntimeError("exactly one owned local fixture page required")
            page = pages[0]
            page.wait_for_load_state("domcontentloaded")
            if page.title() != marker:
                raise RuntimeError("local fixture title mismatch")
            verify_owner()
            windows = []
            enum_callback = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
            user32.EnumWindows.argtypes = [enum_callback, ctypes.c_void_p]
            user32.EnumWindows.restype = ctypes.c_bool

            @enum_callback
            def visit(hwnd, _):
                pid = ctypes.c_ulong()
                user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
                if pid.value == owner.pid and user32.IsWindowVisible(hwnd):
                    title = ctypes.create_unicode_buffer(256)
                    user32.GetWindowTextW(hwnd, title, 256)
                    if marker in title.value:
                        windows.append(int(hwnd))
                return True

            user32.EnumWindows(visit, None)
            if len(windows) != 1:
                raise RuntimeError("exactly one owned fixture HWND required")
            hwnd = windows[0]
            kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel32.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_bool, ctypes.c_ulong]
            kernel32.OpenProcess.restype = ctypes.c_void_p
            kernel32.GetProcessTimes.argtypes = [ctypes.c_void_p] + [ctypes.POINTER(ctypes.c_ulonglong)] * 4
            kernel32.GetProcessTimes.restype = ctypes.c_bool
            kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
            handle = kernel32.OpenProcess(0x1000, False, owner.pid)
            if not handle:
                raise RuntimeError("owned process creation time handle unavailable")
            times = [ctypes.c_ulonglong() for _ in range(4)]
            try:
                if not kernel32.GetProcessTimes(handle, *(ctypes.byref(value) for value in times)):
                    raise RuntimeError("owned process creation time unavailable")
                created_filetime = times[0].value
            finally:
                kernel32.CloseHandle(handle)
            report.update({"browserPid": owner.pid, "processCreatedAt": created_filetime,
                           "rootHwnd": hwnd, "foregroundHandoff": {"probeGrantedChild": granted_child,
                           "probeGrantedVerifiedOwner": granted_owner}})

            alternate_html = fixture / "alternate.html"
            alternate_marker = marker + " alternate"
            with alternate_html.open("x", encoding="utf-8") as stream:
                stream.write(f'<!DOCTYPE html><meta charset="utf-8"><title>{alternate_marker}</title>'
                             '<textarea id="alternate">other owned window</textarea>')
            alternate_url = alternate_html.as_uri()
            cdp = browser.new_browser_cdp_session()
            cdp.send("Target.createTarget", {"url": alternate_url, "newWindow": True})
            deadline = time.monotonic() + 5
            alternate_pages = []
            while time.monotonic() < deadline:
                alternate_pages = [p for c in browser.contexts for p in c.pages if p.url == alternate_url]
                if len(alternate_pages) == 1:
                    break
                page.wait_for_timeout(50)
            if len(alternate_pages) != 1:
                raise RuntimeError("exactly one owned alternate fixture required")
            alternate_page = alternate_pages[0]
            alternate_page.wait_for_load_state("domcontentloaded")
            alternate_windows = []

            @enum_callback
            def visit_alternate(window, _):
                pid = ctypes.c_ulong()
                user32.GetWindowThreadProcessId(window, ctypes.byref(pid))
                if pid.value == owner.pid and user32.IsWindowVisible(window):
                    title = ctypes.create_unicode_buffer(256)
                    user32.GetWindowTextW(window, title, 256)
                    if alternate_marker in title.value:
                        alternate_windows.append(int(window))
                return True

            user32.EnumWindows(visit_alternate, None)
            if len(alternate_windows) != 1 or alternate_windows[0] == hwnd:
                raise RuntimeError("distinct owned alternate HWND required")
            alternate_hwnd = alternate_windows[0]
            report["alternateHwnd"] = alternate_hwnd

            def verify_fixture():
                verify_owner()
                if page.url != fixture_url or page.title() != marker or sha(html) != report["fixtureSha256"]:
                    raise RuntimeError("fixture URL/title/file changed")
                pid = ctypes.c_ulong()
                user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
                if not user32.IsWindow(hwnd) or pid.value != owner.pid:
                    raise RuntimeError("fixture HWND ownership changed")

            for scenario in scenarios:
                host, _, variant = scenario.partition("--")
                case = {"scenario": scenario, "host": host, "variant": variant, "passed": False}
                report["scenarios"].append(case)
                phase = "foregroundReadiness"
                try:
                    verify_fixture()
                    # Ordinary activation by the already verified owned
                    # browser. No global foreground-policy workaround.
                    page.bring_to_front()
                    case["activationAccepted"] = bool(user32.SetForegroundWindow(hwnd))
                    deadline = time.monotonic() + .5
                    while user32.GetForegroundWindow() != hwnd and time.monotonic() < deadline:
                        verify_fixture()
                        time.sleep(.02)
                    if user32.GetForegroundWindow() != hwnd:
                        raise RuntimeError("owned browser did not become foreground; no input attempted")
                    phase = "fixtureFocus"
                    page.evaluate("""({id, variant, context}) => {
                      if(id==='document'){document.body.tabIndex=0;document.body.focus();return;}
                      const el=document.getElementById(id);
                      if(['input','textarea','contenteditable'].includes(id)) {
                        let value='alpha | omega';
                        if(context && variant==='empty')value='';
                        if(context && variant==='emoji')value='前😀ab | 後😀';
                        if(context && variant==='long')value='x'.repeat(1200)+'| omega';
                        if(el.value===undefined)el.textContent=value; else el.value=value;
                        el.removeAttribute('aria-label');
                        if(context && variant==='chat')el.setAttribute('aria-label','聊天输入');
                        if(context && variant==='document')el.setAttribute('aria-label','文档正文');
                      }
                      el.focus(); const start=context && variant==='empty'?0:context && variant==='long'?1200:6;
                      const end=variant==='selection'?start+1:start;
                      if(el.setSelectionRange)el.setSelectionRange(start,end);
                      else if(id==='contenteditable'){
                        const range=document.createRange();range.setStart(el.firstChild||el,start);range.setEnd(el.firstChild||el,end);
                        const selection=getSelection();selection.removeAllRanges();selection.addRange(range);
                      }
                    }""", {"id": host, "variant": variant, "context": args.context})
                    active_id = page.evaluate("document.activeElement.id || (document.activeElement===document.body?'document':'')")
                    if active_id != host:
                        raise RuntimeError("owned DOM focus did not match the registered fixture")
                    verify_fixture()
                    if user32.GetForegroundWindow() != hwnd:
                        raise RuntimeError("native foreground left fixture before request")
                    request_path = requests / f"{scenario}.json"
                    native_path = fixture / f"{scenario}.native.json"
                    write_new(request_path, {"fixtureId": fixture_id, "processId": owner.pid,
                              "processCreatedAt": created_filetime, "rootHwnd": hwnd,
                              "scenario": scenario, "fixtureFile": str(html),
                              "fixtureUrl": fixture_url, "profileDirectory": str(profile),
                              "alternateHwnd": alternate_hwnd if variant == "window" else None})
                    phase = "inputContext" if args.context else "nativeDispatch"
                    native = subprocess.run([str(args.binary), "--scratch-dir", str(fixture),
                                             "--request", str(request_path), "--out", str(native_path)],
                                            capture_output=True, timeout=12,
                                            creationflags=subprocess.CREATE_NO_WINDOW)
                    case.update({"nativeExitCode": native.returncode, "nativeReceipt": str(native_path)})
                    with (fixture / f"{scenario}.stdout.bin").open("xb") as stream:
                        stream.write(native.stdout)
                    with (fixture / f"{scenario}.stderr.bin").open("xb") as stream:
                        stream.write(native.stderr)
                    if not native_path.is_file():
                        case["failureScope"] = "toolFailure"
                        raise RuntimeError("native probe did not write its create-new receipt")
                    result = json.loads(native_path.read_text(encoding="utf-8"))
                    case["native"] = result
                    if not result.get("passed") or native.returncode != 0:
                        case.update({"failureScope": result.get("failureScope") or "productPath",
                                     "failurePhase": result.get("failurePhase") or ("inputContext" if args.context else "nativeDispatch")})
                        print(json.dumps({"scenario": scenario, "passed": False,
                                          "failurePhase": case["failurePhase"]}), flush=True)
                        continue
                    phase = "ownedReadback"
                    verify_fixture()
                    if host in POSITIVE:
                        blocked = args.context or variant in {"cancel", "window"}
                        if args.context:
                            expected = "" if variant == "empty" else "前😀ab | 後😀" if variant == "emoji" else "x" * 1200 + "| omega" if variant == "long" else "alpha | omega"
                        else:
                            expected = "alpha | omega" if blocked else "alpha " + payload(variant) + (" omega" if variant == "selection" else "| omega")
                        read = lambda: page.locator("#" + host).evaluate("el => el.value===undefined?el.innerText:el.value").replace("\r\n", "\n")
                        deadline = time.monotonic() + 2
                        while read() != expected and time.monotonic() < deadline:
                            time.sleep(.03)
                        case["ownedReadbackConfirmed"] = read() == expected
                        if not case["ownedReadbackConfirmed"]:
                            raise RuntimeError("known synthetic Unicode text not confirmed in owned field")
                        if blocked:
                            case["unchangedReadbackConfirmed"] = True
                        if variant == "window":
                            if alternate_page.url != alternate_url or alternate_page.title() != alternate_marker:
                                raise RuntimeError("owned alternate fixture changed")
                            if alternate_page.locator("#alternate").input_value() != "other owned window":
                                raise RuntimeError("alternate window unexpectedly received input")
                            case["alternateUnchangedReadbackConfirmed"] = True
                    elif host != "password" and host != "document":
                        value = page.locator("#" + host).evaluate("el => el.value===undefined?el.textContent:el.value")
                        if value != "alpha | omega":
                            raise RuntimeError("protected synthetic field unexpectedly changed")
                    case["passed"] = True
                except Exception as error:
                    case.update({"failure": str(error), "failurePhase": case.get("failurePhase", phase),
                                 "failureScope": error_scope(error, case.get("failureScope", "testPrecondition" if phase in
                                  {"foregroundReadiness", "fixtureFocus"} else "productPath"))})
                print(json.dumps({"scenario": scenario, "passed": case["passed"],
                                  "failurePhase": case.get("failurePhase")}), flush=True)
        except Exception as error:
            report.update({"error": str(error), "failurePhase": phase,
                           "failureScope": error_scope(error, "testPrecondition")})
        finally:
            cleanup_errors = []
            # A reparented Edge still belongs to this new UUID profile. Verify
            # exact EXE/profile/new creation time before tracking its children.
            if launched_at is not None:
                try:
                    for process, created in profile_processes(psutil, args.edge, profile, launched_at):
                        known_processes[process.pid] = created
                        for child in process.children(recursive=True):
                            known_processes[child.pid] = child.create_time()
                except Exception as error:
                    cleanup_errors.append(str(error))
            # Original launch descendants retain separate PID lifetime checks.
            if browser_process is not None and root_created is not None:
                try:
                    root = psutil.Process(browser_process.pid)
                    if root.create_time() == root_created:
                        for process in root.children(recursive=True):
                            known_processes[process.pid] = process.create_time()
                except psutil.NoSuchProcess:
                    pass
                except Exception as error:
                    cleanup_errors.append(str(error))
            if browser is not None:
                try:
                    browser.close()
                except Exception as error:
                    cleanup_errors.append(str(error))
            if playwright is not None:
                try:
                    playwright.stop()
                except Exception as error:
                    cleanup_errors.append(str(error))
            if browser_process is not None and browser_process.poll() is None:
                try:
                    browser_process.terminate()
                    try:
                        browser_process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        browser_process.kill()
                        browser_process.wait(timeout=3)
                except Exception as error:
                    cleanup_errors.append(str(error))
            remaining = []
            if known_processes:
                for pid, created in reversed(list(known_processes.items())):
                    try:
                        process = psutil.Process(pid)
                        if process.create_time() != created:
                            continue
                        try:
                            process.wait(timeout=2)
                        except psutil.TimeoutExpired:
                            process.terminate()
                            try:
                                process.wait(timeout=2)
                            except psutil.TimeoutExpired:
                                process.kill()
                                process.wait(timeout=2)
                    except psutil.NoSuchProcess:
                        pass
                    except Exception as error:
                        remaining.append(pid)
                        cleanup_errors.append(str(error))
            if launched_at is not None:
                try:
                    for process, _ in profile_processes(psutil, args.edge, profile, launched_at):
                        if process.pid not in remaining:
                            remaining.append(process.pid)
                except Exception as error:
                    cleanup_errors.append(str(error))
                    remaining.append("ownershipAftercheckUnavailable")
            report["cleanup"] = {"completed": not remaining, "remainingOwnedPids": remaining,
                                 "errors": cleanup_errors, "fixtureArtifactsRetained": True}
            if "sourceInputs" in report:
                try:
                    report["sourceInputsStable"] = report["sourceInputs"] == {str(path.relative_to(repo)): sha(path) for path in source_paths}
                    report["binaryStable"] = report["binarySha256"] == sha(args.binary)
                except Exception as error:
                    report.update({"sourceInputsStable": False, "binaryStable": False,
                                   "stabilityCheckError": str(error)})
            for scope, count_name in (("testPrecondition", "testPreconditionFailures"),
                                      ("productPath", "productPathFailures"), ("toolFailure", "toolFailures")):
                report[count_name] = sum(case.get("failureScope") == scope for case in report["scenarios"]) \
                    + int(bool(report.get("error")) and report.get("failureScope") == scope)
            report["passed"] = len(report["scenarios"]) == len(scenarios) and not report.get("error") \
                and all(case["passed"] for case in report["scenarios"]) and report["cleanup"]["completed"] \
                and report.get("sourceInputsStable", False) and report.get("binaryStable", False)
            report["completedUnix"] = time.time()
            json.dump(report, receipt, ensure_ascii=False, indent=2)
            receipt.flush()
    print(json.dumps({"passed": report["passed"], "receipt": str(args.out),
                      "scenarios": len(report["scenarios"]), "preconditionFailures": report["testPreconditionFailures"],
                      "productPathFailures": report["productPathFailures"]}, ensure_ascii=True))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
