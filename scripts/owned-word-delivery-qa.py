"""Opt-in Word acceptance using a fresh COM instance and UUID-owned documents.

Requires Windows, Word, pywin32 and psutil. Never attaches to the user's Word
instance, opens user documents, records audio or calls a cloud service.
"""
import argparse
import ctypes
import gc
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time
import uuid

SCENARIOS = ("emoji", "long", "selection", "cancel", "window")


def payload(variant):
    if variant == "emoji":
        return "SpeakMode 中文 English 👩🏽‍💻 🚀 e\u0301，保留否定。"
    if variant == "long":
        return "\n".join(f"{line:02}. SpeakMode 中文 English 👩🏽‍💻；预算 3500，不删除原始录音。" for line in range(1, 41))
    return "SpeakMode 合成测试。"


def absolute(value):
    path = Path(value)
    if not path.is_absolute():
        raise argparse.ArgumentTypeError("explicit absolute path required")
    return path.resolve()


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "scratch-dir", "out"):
        parser.add_argument("--" + name, type=absolute, required=True)
    args = parser.parse_args()
    with args.out.open("x", encoding="utf-8") as stream:
        report = {"scope": "REAL_WINDOWS_OWNED_WORD_SYNTHETIC_TEXT_ONLY", "cloud": "NOT_USED",
                  "microphone": "NOT_USED", "userDocuments": "NOT_READ", "passed": False,
                  "registeredScenarios": SCENARIOS, "scenarios": [], "startedUnix": time.time()}
        app = doc = alternate = process = None
        owned = False
        phase = "fixtureStartup"
        try:
            if not args.binary.is_file():
                raise RuntimeError("explicit native probe binary must exist before Word starts")
            import psutil
            import win32com.client
            args.scratch_dir.mkdir(parents=True, exist_ok=True)
            fixture_id = uuid.uuid4().hex
            fixture = args.scratch_dir / fixture_id
            fixture.mkdir()
            (fixture / "requests").mkdir()
            marker = "speakmode-owned-word-" + fixture_id
            document = fixture / (marker + ".docx")
            user = ctypes.WinDLL("user32", use_last_error=True)
            user.GetForegroundWindow.restype = ctypes.c_void_p
            user.SetForegroundWindow.argtypes = [ctypes.c_void_p]
            user.SetForegroundWindow.restype = ctypes.c_bool
            user.AllowSetForegroundWindow.argtypes = [ctypes.c_ulong]
            user.AllowSetForegroundWindow.restype = ctypes.c_bool
            user.GetWindowThreadProcessId.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
            created_after = time.time()
            # DispatchEx requests a new COM server; independently reject a reused PID.
            app = win32com.client.DispatchEx("Word.Application")
            candidates = [p for p in psutil.process_iter() if p.name().lower() == "winword.exe"
                          and p.create_time() >= created_after]
            if len(candidates) != 1:
                raise RuntimeError("exactly one fresh Word COM process required")
            process = candidates[0]
            created = process.create_time()
            if created < created_after or Path(process.exe()).name.lower() != "winword.exe":
                raise RuntimeError("Word did not provide a fresh owned process")
            owned = True
            if app.Documents.Count != 0:
                raise RuntimeError("fresh Word already has documents; acceptance withheld")
            report["foregroundHandoffGranted"] = bool(user.AllowSetForegroundWindow(process.pid))
            app.DisplayAlerts = 0
            app.Visible = True
            doc = app.Documents.Add()
            # Word exposes HWND on Window, not on Application. Tie the COM
            # document to the independently discovered fresh process before input.
            pid = ctypes.c_ulong()
            user.GetWindowThreadProcessId(int(doc.ActiveWindow.Hwnd), ctypes.byref(pid))
            if pid.value != process.pid:
                owned = False
                doc.Close(SaveChanges=0)
                doc = None
                raise RuntimeError("COM document did not belong to the fresh process")
            doc.Content.Text = "alpha | omega"
            doc.SaveAs2(str(document), FileFormat=16)
            alternate = app.Documents.Add()
            alternate.Content.Text = "other owned window"
            alternate.SaveAs2(str(fixture / (marker + "-alternate.docx")), FileFormat=16)
            alternate_hwnd = int(alternate.ActiveWindow.Hwnd)
            hwnd = int(doc.ActiveWindow.Hwnd)
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_bool, ctypes.c_ulong]
            kernel.OpenProcess.restype = ctypes.c_void_p
            kernel.GetProcessTimes.argtypes = [ctypes.c_void_p] + [ctypes.POINTER(ctypes.c_ulonglong)] * 4
            kernel.CloseHandle.argtypes = [ctypes.c_void_p]
            handle = kernel.OpenProcess(0x1000, False, pid.value)
            if not handle:
                raise RuntimeError("owned process timing handle unavailable")
            times = [ctypes.c_ulonglong() for _ in range(4)]
            try:
                if not kernel.GetProcessTimes(handle, *(ctypes.byref(t) for t in times)):
                    raise RuntimeError("owned process creation time unavailable")
            finally:
                kernel.CloseHandle(handle)
            repo = Path(__file__).resolve().parents[1]
            sources = [Path(__file__).resolve(), repo / "app/src-tauri/examples/owned_word_delivery_probe.rs",
                       repo / "app/src-tauri/examples/support/owned_delivery_cases.rs",
                       repo / "app/src-tauri/src/platform/delivery.rs", repo / "app/src-tauri/src/platform/mod.rs"]
            report.update({"fixtureDirectory": str(fixture), "processId": pid.value,
                           "processCreatedAt": times[0].value, "wordVersion": app.Version,
                           "binarySha256": sha(args.binary),
                           "sourceInputs": {str(p.relative_to(repo)): sha(p) for p in sources}})
            print(json.dumps({"ownedWordPid": pid.value, "fixture": str(fixture)}), flush=True)

            def verify():
                if not process.is_running() or process.create_time() != created:
                    raise RuntimeError("owned Word lifetime changed")
                if Path(doc.FullName).resolve() != document or app.Documents.Count != 2:
                    raise RuntimeError("owned Word document boundary changed")

            for variant in SCENARIOS:
                case = {"scenario": variant, "passed": False}
                report["scenarios"].append(case)
                phase = "foregroundReadiness"
                try:
                    verify()
                    doc.Content.Text = "alpha | omega"
                    doc.Activate()
                    app.Activate()
                    doc.Range(6, 7 if variant == "selection" else 6).Select()
                    doc.Repaginate()
                    app.ScreenRefresh()
                    if app.Selection.Start != 6 or app.Selection.End != (7 if variant == "selection" else 6):
                        raise RuntimeError("owned Word selection is not ready")
                    case["activationAccepted"] = bool(user.SetForegroundWindow(hwnd))
                    deadline = time.monotonic() + .5
                    while user.GetForegroundWindow() != hwnd and time.monotonic() < deadline:
                        time.sleep(.02)
                    if user.GetForegroundWindow() != hwnd:
                        raise RuntimeError("owned Word document is not foreground")
                    request = fixture / "requests" / (variant + ".json")
                    native_path = fixture / (variant + ".native.json")
                    with request.open("x", encoding="utf-8") as file:
                        json.dump({"fixtureId": fixture_id, "processId": pid.value,
                                   "processCreatedAt": times[0].value, "rootHwnd": hwnd,
                                   "scenario": variant, "fixtureFile": str(document),
                                   "alternateHwnd": alternate_hwnd if variant == "window" else None}, file)
                    phase = "nativeDispatch"
                    result = subprocess.run([str(args.binary), "--scratch-dir", str(fixture),
                                             "--request", str(request), "--out", str(native_path)],
                                            capture_output=True, timeout=15,
                                            creationflags=subprocess.CREATE_NO_WINDOW)
                    for suffix, data in (("stdout", result.stdout), ("stderr", result.stderr)):
                        (fixture / (variant + "." + suffix + ".bin")).write_bytes(data)
                    native = json.loads(native_path.read_text(encoding="utf-8"))
                    case.update({"nativeReceipt": str(native_path), "native": native, "nativeExitCode": result.returncode})
                    if not native.get("passed") or result.returncode:
                        case.update({"failurePhase": native.get("failurePhase"),
                                     "failureScope": native.get("failureScope") or "productPath"})
                        continue
                    phase = "ownedReadback"
                    verify()
                    blocked = variant in {"cancel", "window"}
                    expected = "alpha | omega" if blocked else "alpha " + payload(variant) + (" omega" if variant == "selection" else "| omega")
                    read = lambda: doc.Content.Text.removesuffix("\r").replace("\r\n", "\n").replace("\r", "\n")
                    deadline = time.monotonic() + 2
                    while read() != expected and time.monotonic() < deadline:
                        time.sleep(.03)
                    if read() != expected:
                        raise RuntimeError("owned synthetic Word text did not match expected result")
                    if alternate.Content.Text != "other owned window\r":
                        raise RuntimeError("alternate document changed")
                    case.update({"passed": True, "ownedReadbackConfirmed": True,
                                 "alternateUnchangedReadbackConfirmed": True})
                except Exception as error:
                    case.update({"failure": str(error), "failurePhase": phase,
                                 "failureScope": "testPrecondition" if phase == "foregroundReadiness" else "productPath"})
                finally:
                    print(json.dumps({"scenario": variant, "passed": case["passed"],
                                      "failurePhase": case.get("failurePhase")}), flush=True)
            report["sourceInputsStable"] = report["sourceInputs"] == {str(p.relative_to(repo)): sha(p) for p in sources}
            report["binaryStable"] = report["binarySha256"] == sha(args.binary)
        except Exception as error:
            report.update({"failure": str(error), "failurePhase": phase, "failureScope": "testPrecondition"})
        finally:
            report["cleanup"] = {"completed": False}
            if owned:
                try:
                    # Preserve the instance if someone opened another document during QA.
                    if app.Documents.Count > sum(x is not None for x in (doc, alternate)):
                        raise RuntimeError("unexpected document in owned instance; automatic close withheld")
                    for document_object in (alternate, doc):
                        if document_object is not None:
                            document_object.Close(SaveChanges=0)
                    app.Quit(SaveChanges=0)
                    document_object = alternate = doc = app = None
                    gc.collect()
                    report["cleanup"]["quitReturned"] = True
                    try:
                        process.wait(timeout=3)
                    except psutil.TimeoutExpired:
                        # Office can retain an invisible COM server after Quit.
                        # Only terminate our same-lifetime process after all owned
                        # docs were closed and no visible window remains.
                        visible = []
                        callback_type = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
                        user.IsWindowVisible.argtypes = [ctypes.c_void_p]
                        user.EnumWindows.argtypes = [callback_type, ctypes.c_void_p]

                        @callback_type
                        def visit(window, _):
                            window_pid = ctypes.c_ulong()
                            user.GetWindowThreadProcessId(window, ctypes.byref(window_pid))
                            if window_pid.value == process.pid and user.IsWindowVisible(window):
                                visible.append(int(window))
                            return True

                        user.EnumWindows(visit, None)
                        if visible or process.create_time() != created:
                            raise RuntimeError("Word still has a visible window or changed lifetime; termination withheld")
                        process.terminate()
                        process.wait(timeout=3)
                        report["cleanup"]["ownedBackgroundProcessTerminated"] = True
                    report["cleanup"]["completed"] = True
                except Exception as error:
                    report["cleanup"]["error"] = str(error)
            report["passed"] = len(report["scenarios"]) == len(SCENARIOS) and all(c["passed"] for c in report["scenarios"]) \
                and report["cleanup"]["completed"] and report.get("sourceInputsStable", False) and report.get("binaryStable", False)
            report["completedUnix"] = time.time()
            json.dump(report, stream, ensure_ascii=False, indent=2)
    print(json.dumps({"passed": report["passed"], "receipt": str(args.out)}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
