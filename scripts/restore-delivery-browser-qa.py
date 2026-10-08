"""Production React smoke with an isolated synthetic Tauri bridge.

Run against a separately managed `vite preview` on 127.0.0.1. Every --out must
name a new absolute directory. No native app, storage, microphone, clipboard,
provider, credentials or host input is accessed. The mock restore/undo only
checks frontend requests and makes their synthetic responses visible in React;
it is not a replacement for native Store or delivery acceptance.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import socket
import subprocess
import sys
import time
import traceback
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import sync_playwright, expect


ROOT = Path(__file__).resolve().parents[1]
SOURCE_LABEL = "选择识别稿中的恢复范围"
TARGET_LABEL = "选择当前文本中的插入或替换位置"
NOW = "2026-10-08T04:00:00Z"


def fixture(delivery_state=None, reason=None):
    mode = {"modeId": "structure", "name": "结构", "baseMode": "structure", "rules": "保留所有条件和否定。", "revision": 1}
    source = {"id": "synthetic-source-r1", "sessionId": "synthetic-session", "asrAttemptId": "synthetic-asr", "generation": 1,
              "provider": "synthetic", "model": "synthetic", "adapterVersion": "synthetic-browser-v1", "complete": True,
              "items": [{"id": f"synthetic-source-{i}", "providerItemId": f"synthetic-{i}", "captureOrder": i,
                         "text": text, "final": True, "startMs": None, "endMs": None}
                        for i, text in enumerate(["今天", "不要", "第二段来源保持。"], 1)], "createdAt": NOW}
    output = {"id": "synthetic-output-r1", "sessionId": "synthetic-session", "parentRevisionId": None, "baseRevisionId": None,
              "sourceRevisionId": source["id"], "modeSnapshot": mode, "resolvedMode": "structure", "kind": "initial",
              "creationReason": "normalInitial", "createdAt": NOW, "blocks": [
                  {"id": "synthetic-block-1", "text": "🙂进行测试。余下内容保持不变。", "sourceRefs": ["synthetic-source-1", "synthetic-source-2"]},
                  {"id": "synthetic-block-2", "text": "第二段保持不变。", "sourceRefs": ["synthetic-source-3"]}]}
    detail = {"session": {"id": "synthetic-session", "generation": 1, "startedAt": NOW, "endedAt": NOW,
                         "captureDurationMs": 4200, "endLocalDate": "2026-10-08", "timeZone": "Asia/Shanghai", "storagePolicy": "history",
                         "modeSnapshot": mode, "status": "ready", "currentSourceId": source["id"], "currentOutputId": output["id"], "undoTargetId": None},
              "source": source, "output": output, "canUndo": False}
    live = None if delivery_state is None else {"sessionId": "synthetic-session", "generation": 1, "state": "ready", "modeSnapshot": mode,
                                               "workerReleased": True, "deliveryState": delivery_state, "deliveryFailureReason": reason,
                                               "items": [], "captions": []}
    snapshot = {"sequence": 0, "preferences": {"version": 1, "shortcut": "alt_right", "microphoneId": None, "language": "auto",
                 "defaultModeId": "auto", "deliveryMode": "direct", "keepHistory": True, "launchAtLogin": False},
                "modes": [{"id": mid, "name": name, "baseMode": mid, "rules": "保留原意", "revision": 1, "builtin": True}
                          for mid, name in [("auto", "自动"), ("light", "轻整理"), ("structure", "结构")]],
                "usage": {"date": "2026-10-08", "count": 1, "sourceCharCount": 16, "captureDurationMs": 4200},
                "provider": {"configured": True, "credentialConfigured": False, "config": {"version": 1, "provider": "synthetic", "region": None,
                             "workspace": None, "asrModel": "synthetic", "rewriteModel": "synthetic", "adapterVersion": "synthetic-browser-v1"}},
                "dictionary": {"version": 1, "entries": []}, "devices": [], "session": live,
                "temporaryResult": None, "capabilities": {}}
    return {"detail": detail, "snapshot": snapshot}


BRIDGE = r"""
(() => {
 const fixture = __FIXTURE__, copy = value => structuredClone(value);
 const ledger = [], callbacks = new Map(), original = copy(fixture.detail.output);
 let callbackId = 0, revision = 1;
 window.isTauri = true;
 window.__speakmodeQA = {ledger, detail:fixture.detail, fixtureVersion:'restore-delivery-browser-v1'};
 function summary() {
  const d=fixture.detail,o=d.output;
  return {sessionId:d.session.id,title:o.blocks.map(b=>b.text).join(' ').slice(0,48),preview:o.blocks.map(b=>b.text).join(' '),
   endedAt:d.session.endedAt,endLocalDate:d.session.endLocalDate,modeId:o.modeSnapshot.modeId,modeName:o.modeSnapshot.name,
   resolvedMode:o.resolvedMode,currentRevisionId:o.id,sourceRevisionId:d.source.id,status:'ready',sourceCharCount:16,captureDurationMs:4200};
 }
 window.__TAURI_INTERNALS__ = {
  transformCallback(fn) {const id=++callbackId;callbacks.set(id,fn);return id;},
  unregisterCallback(id) {callbacks.delete(id);},
  async invoke(command,args={}) {
   if(command==='subscribe_state')return copy(fixture.snapshot);
   if(command==='control_main_window')return {maximized:false,minimized:false};
   if(command!=='app_command')throw new Error('Blocked unexpected synthetic IPC: '+command);
   const {name,payload}=args.request;ledger.push({name,payload:copy(payload)});
   if(name==='state.snapshot')return copy(fixture.snapshot);
   if(name==='audio.listDevices')return [];
   if(name==='history.list')return {items:[summary()],total:1,nextCursor:null};
   if(name==='history.get') {if(payload.sessionId!==fixture.detail.session.id)throw new Error('Unknown synthetic history ID');return copy(fixture.detail);}
   if(name==='window.resizeRecorder'||name==='app.showMain')return null;
   if(name==='result.copy')return null; // Ledger only: never touches the browser/native clipboard.
   if(name==='revision.restore') {
    const d=fixture.detail,o=d.output,p=payload.patch;
    if(payload.expectedCurrentRevisionId!==o.id)throw new Error('Synthetic stale revision');
    const block=o.blocks.find(b=>b.id===p.blockId);if(!block)throw new Error('Synthetic missing block');
    const scalars=Array.from(block.text);
    if(!Number.isInteger(p.start)||!Number.isInteger(p.end)||p.start<0||p.end<p.start||p.end>scalars.length)throw new Error('Invalid synthetic target range');
    const inserted=p.sourceSelections.map(selection=>{
     const item=d.source.items.find(s=>s.id===selection.itemId);if(!item)throw new Error('Unknown synthetic source');
     const text=Array.from(item.text);
     if(selection.start<0||selection.end<=selection.start||selection.end>text.length)throw new Error('Invalid synthetic source range');
     return text.slice(selection.start,selection.end).join('');
    }).join('');
    const previous=copy(o);block.text=scalars.slice(0,p.start).join('')+inserted+scalars.slice(p.end).join('');
    o.id='synthetic-output-r'+(++revision);o.parentRevisionId=previous.id;o.kind='restore';d.canUndo=true;
    d.session.currentOutputId=o.id;d.session.undoTargetId=previous.id;
    return copy(d);
   }
   if(name==='revision.undo') {
    const d=fixture.detail;if(payload.expectedCurrentRevisionId!==d.output.id||!d.canUndo)throw new Error('Synthetic stale/no undo');
    const previous=d.output.id;d.output=copy(original);d.output.id='synthetic-output-r'+(++revision);d.output.parentRevisionId=previous;d.output.kind='undo';
    d.session.currentOutputId=d.output.id;d.session.undoTargetId=null;d.canUndo=false;
    window.__speakmodeQA.detail=d;return copy(d);
   }
   throw new Error('Blocked unexpected synthetic app command: '+name);
  }
 };
})();
"""


def write_new(path, value, *, json_value=False):
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(json.dumps(value, ensure_ascii=False, indent=2) + "\n" if json_value else value)


def screenshot(page, path):
    if path.exists():
        raise FileExistsError(path)
    page.screenshot(path=str(path), full_page=True)
    return str(path)


@contextmanager
def managed_preview(url, out, serve):
    """Own the actual Node process, avoiding Windows shell grandchildren.

    The skill's with_server.py was tried first. Its shell=True termination left
    a listening Node child on Windows, so this direct process is necessary for
    reliable cleanup. An already occupied port is never killed or reused.
    """
    parsed = urlparse(url)
    port = parsed.port or 80
    info = {"managed": serve, "port": port, "started": False, "exited": False}
    process = None
    log = None
    def listening():
        with socket.socket() as sock:
            sock.settimeout(0.2)
            return sock.connect_ex(("127.0.0.1", port)) == 0
    try:
        if serve:
            if listening():
                raise RuntimeError(f"Port {port} already in use; refusing to reuse or stop its owner")
            node = shutil.which("node.exe") or shutil.which("node")
            if not node:
                raise RuntimeError("Node runtime not found")
            command = [node, str(ROOT / "app/node_modules/vite/bin/vite.js"), "preview", "--host", "127.0.0.1", "--port", str(port), "--strictPort"]
            log = (out / "preview-server.log").open("x", encoding="utf-8")
            process = subprocess.Popen(command, cwd=ROOT / "app", stdout=log, stderr=subprocess.STDOUT, shell=False)
            info.update({"pid": process.pid, "command": command, "started": True})
            deadline = time.monotonic() + 30
            while not listening():
                if process.poll() is not None:
                    raise RuntimeError(f"Preview server exited with {process.returncode}")
                if time.monotonic() >= deadline:
                    raise TimeoutError("Independent preview server was not ready within 30 seconds")
                time.sleep(0.1)
        yield info
    finally:
        if process:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            info.update({"exitCode": process.returncode, "exited": True, "listeningAfterExit": listening()})
        if log:
            log.close()


def command_ledger(page):
    return page.evaluate("window.__speakmodeQA.ledger")


def select_paragraph(page, label, number):
    page.get_by_role("combobox", name=label, exact=True).click()
    page.get_by_role("option", name=re.compile(rf"^第 {number} 段 ·")).click()


def choose_source_target(page, source=1, target=1):
    select_paragraph(page, "选择识别来源", source)
    select_paragraph(page, "选择恢复目标", target)


def pointer_caret(area, start, end):
    """Click a measured glyph boundary in the fixture's first line.

    Current Edge does not move a readonly textarea caret for ArrowRight or
    Ctrl+Home; that failed harness precondition is preserved in earlier evidence.
    Use a real browser pointer click instead of calling callbacks, synthesizing
    DOM events, or setting a selection through JavaScript. The test fixture has
    no wrapping before these short offsets. Native scalar requests are checked
    separately from the asserted UTF-16 DOM selection.
    """
    if start != end:
        raise ValueError("This helper only places collapsed pointer carets")
    area.scroll_into_view_if_needed()
    position = area.evaluate("""(el,offset)=>{
      const style=getComputedStyle(el),canvas=document.createElement('canvas'),ctx=canvas.getContext('2d');
      ctx.font=style.font;const fontSize=parseFloat(style.fontSize),lineHeight=parseFloat(style.lineHeight)||fontSize*1.2;
      return {x:parseFloat(style.paddingLeft)+ctx.measureText(el.value.slice(0,offset)).width+0.5,
              y:parseFloat(style.paddingTop)+lineHeight/2};
    }""", start)
    area.click(position=position)
    expect(area).to_have_js_property("selectionStart", start)
    expect(area).to_have_js_property("selectionEnd", end)


def target_intent(page, start, end):
    pointer_caret(page.get_by_role("textbox", name=TARGET_LABEL, exact=True), start, end)


def source_intent(page, start, end):
    pointer_caret(page.get_by_role("textbox", name=SOURCE_LABEL, exact=True), start, end)


def preview(page):
    return page.locator(".history-restore-preview")


def restore_names(page):
    return page.get_by_role("button", name=re.compile(r"^确认(插入|替换)这部分$"))


def open_history(page):
    page.get_by_role("button", name="查看与调整", exact=True).click()
    expect(page.locator(".history-detail")).to_be_visible()
    page.get_by_role("button", name=re.compile("^对照识别稿")).click()
    expect(page.get_by_role("combobox", name="选择恢复目标", exact=True)).to_be_visible()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True, help="Independent production preview URL on http://127.0.0.1")
    parser.add_argument("--out", required=True, type=Path, help="New absolute evidence directory; existing paths are rejected")
    parser.add_argument("--serve", action="store_true", help="Own a production vite preview Node process with guaranteed cleanup; refuses occupied ports")
    parser.add_argument("--inspect-only", action="store_true", help="Preserve initial production DOM/screenshots before choosing action selectors")
    args = parser.parse_args()
    parsed = urlparse(args.url)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or parsed.username or parsed.password:
        parser.error("--url must be an independent http://127.0.0.1 URL")
    if not args.out.is_absolute():
        parser.error("--out must be absolute")
    args.out.mkdir(parents=True, exist_ok=False)
    started = datetime.now(timezone.utc).isoformat()
    rows, errors, blocked_requests = [], [], []
    receipt = {"status": "RUNNING", "scope": "Production React in headless Edge with isolated synthetic Tauri IPC. No native Store/provider/audio/clipboard/host delivery acceptance.",
               "startedAt": started, "url": args.url, "out": str(args.out), "inspectOnly": args.inspect_only, "tests": rows}
    sources = ["app/src/components/HistoryDetail.tsx", "app/src/components/restoreSelection.ts", "app/src/services/deliveryFeedback.ts", "app/src/windows/RecorderWindow.tsx"]
    receipt["sourceSha256"] = {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in sources}
    receipt["scriptSha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    receipt["distSha256"] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted((ROOT / "app/dist").rglob("*")) if path.is_file()}

    with managed_preview(args.url, args.out, args.serve) as server, sync_playwright() as playwright:
        receipt["server"] = server
        browser = playwright.chromium.launch(channel="msedge", headless=True)
        receipt["browserVersion"] = browser.version
        try:
            def new_page(data, *, recorder=False, tag):
                context = browser.new_context(viewport={"width": 1440, "height": 1080}, locale="zh-CN")
                context.add_init_script(BRIDGE.replace("__FIXTURE__", json.dumps(data, ensure_ascii=False)))
                def route(request_route):
                    host = urlparse(request_route.request.url).hostname
                    if host not in ("127.0.0.1", None):
                        blocked_requests.append(request_route.request.url)
                        request_route.abort()
                    else:
                        request_route.continue_()
                context.route("**/*", route)
                page = context.new_page()
                page.on("pageerror", lambda error: errors.append({"tag": tag, "error": str(error)}))
                page.goto(args.url + ("?window=recorder" if recorder else ""), wait_until="networkidle")
                page.wait_for_load_state("networkidle")
                screenshot(page, args.out / f"{tag}-initial.png")
                write_new(args.out / f"{tag}-initial.html", page.content())
                write_new(args.out / f"{tag}-initial-controls.json", page.locator("button,input,textarea,[role=combobox]").evaluate_all(
                    "nodes => nodes.map(el => ({tag:el.tagName,role:el.getAttribute('role'),name:el.getAttribute('aria-label'),text:el.textContent,disabled:el.disabled}))"), json_value=True)
                return context, page

            if args.inspect_only:
                context, page = new_page(fixture(), tag="history-recon")
                context.close()
                context, page = new_page(fixture("unconfirmed"), recorder=True, tag="recorder-recon")
                context.close()
                receipt["status"] = "INSPECTION_ONLY"
            else:
                def run_case(name, fn, *, data=None, recorder=False):
                    context, page = new_page(data or fixture(), recorder=recorder, tag=name)
                    row = {"name": name, "status": "RUNNING", "initialScreenshot": str(args.out / f"{name}-initial.png")}
                    rows.append(row)
                    try:
                        row["observations"] = fn(page)
                        row["status"] = "PASS"
                    except Exception as error:
                        row["status"] = "FAIL"
                        row["error"] = str(error)
                        row["traceback"] = traceback.format_exc()
                    finally:
                        row["commands"] = command_ledger(page)
                        row["domSelections"] = page.locator("textarea").evaluate_all("nodes=>nodes.map(el=>({name:el.getAttribute('aria-label'),start:el.selectionStart,end:el.selectionEnd,focused:document.activeElement===el}))")
                        focus = page.locator(".history-restore-form")
                        if focus.count():
                            focus.scroll_into_view_if_needed()
                        row["screenshot"] = screenshot(page, args.out / f"{name}-{row['status'].lower()}.png")
                        write_new(args.out / f"{name}-final.html", page.content())
                        context.close()

                def no_default(page):
                    open_history(page)
                    select_paragraph(page, "选择识别来源", 1)
                    expect(preview(page)).to_have_count(0)
                    select_paragraph(page, "选择恢复目标", 1)
                    expect(preview(page)).to_have_count(0)
                    expect(page.get_by_text("请在此文本中点击插入位置，或划选要替换的文字。", exact=True)).to_be_visible()
                    return {"targetChosen": True, "previewCount": 0, "wholeBlockReplacementDefault": False}
                run_case("01-no-default-target", no_default)

                def today_restore_undo(page):
                    open_history(page);choose_source_target(page)
                    target_intent(page, 2, 2)  # Emoji occupies two UTF-16 code units, one scalar.
                    expect(preview(page)).to_contain_text("插入后的目标段预览")
                    expect(preview(page)).to_contain_text("🙂今天进行测试。余下内容保持不变。")
                    page.get_by_role("button", name="确认插入这部分", exact=True).click()
                    expect(page.locator(".history-current")).to_contain_text("🙂今天进行测试。余下内容保持不变。")
                    expect(page.locator(".history-current")).to_contain_text("第二段保持不变。")
                    calls = [call for call in command_ledger(page) if call["name"] == "revision.restore"]
                    assert len(calls) == 1, calls
                    request = calls[0]["payload"]
                    assert request["expectedCurrentRevisionId"] == "synthetic-output-r1", request
                    assert request["patch"] == {"blockId": "synthetic-block-1", "start": 1, "end": 1,
                                                 "sourceSelections": [{"itemId": "synthetic-source-1", "start": 0, "end": 2}]}, request
                    restored = page.evaluate("window.__speakmodeQA.detail.output.blocks")
                    page.locator(".history-current").scroll_into_view_if_needed()
                    screenshot(page, args.out / "02-restored-before-undo.png")
                    page.get_by_role("button", name="撤销上次修改", exact=True).click()
                    expect(page.locator(".history-current")).to_contain_text("🙂进行测试。余下内容保持不变。")
                    assert "今天" not in page.locator(".history-current").inner_text()
                    expect(page.locator(".history-current")).to_contain_text("第二段保持不变。")
                    assert len([call for call in command_ledger(page) if call["name"] == "revision.undo"]) == 1
                    return {"patch": request["patch"], "restoredBlocks": restored, "undoBlocks": page.evaluate("window.__speakmodeQA.detail.output.blocks"), "nativeStoreAcceptance": False}
                run_case("02-today-insert-and-undo", today_restore_undo)

                def collapse_target(page):
                    open_history(page);choose_source_target(page)
                    area = page.get_by_role("textbox", name=TARGET_LABEL, exact=True)
                    area.focus();area.press("Control+a")
                    expect(preview(page)).to_contain_text("替换后的目标段预览")
                    # Edge's readonly Ctrl+A -> pointer collapse differs from
                    # editable textareas (also reproduced in a plain textarea).
                    # Exact DOM range setup is explicit for this one scenario;
                    # a native browser key delivers the real React event. The
                    # independent insertion/undo case uses actual pointer input.
                    area.evaluate("el=>el.setSelectionRange(2,2)")
                    area.press("Shift")
                    expect(area).to_have_js_property("selectionStart", 2)
                    expect(area).to_have_js_property("selectionEnd", 2)
                    expect(preview(page)).to_contain_text("插入后的目标段预览")
                    expect(preview(page)).to_contain_text("🙂今天进行测试。余下内容保持不变。")
                    expect(page.get_by_role("button", name="确认替换这部分", exact=True)).to_have_count(0)
                    preview(page).scroll_into_view_if_needed()
                    screenshot(page, args.out / "03-collapsed-precise-caret-preview.png")
                    page.get_by_role("button", name="确认插入这部分", exact=True).click()
                    expect(page.locator(".history-current")).to_contain_text("🙂今天进行测试。余下内容保持不变。")
                    expect(page.locator(".history-current")).to_contain_text("第二段保持不变。")
                    calls = [call for call in command_ledger(page) if call["name"] == "revision.restore"]
                    assert len(calls) == 1 and calls[0]["payload"]["patch"]["start"] == 1 and calls[0]["payload"]["patch"]["end"] == 1, calls
                    return {"collapsedAtUtf16": 2, "scalarOffset": 1, "rangeSetup": "DOM setSelectionRange followed by native Shift key; not pointer gesture acceptance",
                            "patch": calls[0]["payload"]["patch"], "submittedWithRealButtonClick": True, "otherBlockUnchanged": True}
                run_case("03-whole-selection-to-emoji-caret", collapse_target)

                def collapse_source(page):
                    open_history(page);choose_source_target(page);target_intent(page, 2, 2)
                    expect(preview(page)).to_have_count(1)
                    source_intent(page, 1, 1)
                    expect(preview(page)).to_have_count(0)
                    expect(restore_names(page)).to_have_count(0)
                    return {"sourceCaret": 1, "previewCount": 0}
                run_case("04-collapsed-source-clears-intent", collapse_source)

                def changing_paragraph(page):
                    open_history(page);choose_source_target(page);target_intent(page, 2, 2)
                    expect(preview(page)).to_have_count(1)
                    select_paragraph(page, "选择恢复目标", 2)
                    expect(preview(page)).to_have_count(0)
                    target_intent(page, 0, 0);expect(preview(page)).to_have_count(1)
                    select_paragraph(page, "选择识别来源", 2)
                    expect(preview(page)).to_have_count(0)
                    return {"targetChangeCleared": True, "sourceChangeCleared": True}
                run_case("05-paragraph-switch-clears-intent", changing_paragraph)

                def close_reopen(page):
                    open_history(page);choose_source_target(page);target_intent(page, 2, 2);expect(preview(page)).to_have_count(1)
                    page.get_by_role("button", name=re.compile("^收起识别稿对照")).click()
                    expect(page.get_by_role("textbox", name=TARGET_LABEL, exact=True)).to_have_count(0)
                    page.get_by_role("button", name=re.compile("^对照识别稿")).click()
                    expect(preview(page)).to_have_count(0)
                    expect(page.get_by_role("textbox", name=TARGET_LABEL, exact=True)).to_have_count(0)
                    expect(page.get_by_role("combobox", name="选择恢复目标", exact=True)).to_contain_text("选择结果段")
                    expect(page.get_by_role("combobox", name="选择识别来源", exact=True)).to_contain_text("选择识别段")
                    return {"reopenedPreviewCount": 0, "sourceAndTargetReset": True}
                run_case("06-comparison-close-reopen-clears-intent", close_reopen)

                def unconfirmed(page):
                    expect(page.get_by_text("输入已派发", exact=True)).to_be_visible()
                    expect(page.get_by_text("请核对目标中的文本，应用不会自动重复输入。", exact=True)).to_be_visible()
                    expect(page.get_by_role("button", name="查看成稿", exact=True)).to_be_enabled()
                    before = len(command_ledger(page))
                    page.get_by_role("button", name="查看成稿", exact=True).click()
                    page.wait_for_function("window.__speakmodeQA.ledger.some(call=>call.name==='app.showMain')")
                    delta = command_ledger(page)[before:]
                    actions = [call for call in delta if call["name"] != "window.resizeRecorder"]
                    assert [call["name"] for call in actions] == ["app.showMain"], actions
                    assert not re.search("已粘贴|已成功输入|目标已收到|已插入", page.locator(".recorder-assembly").inner_text())
                    assert all(call["name"] not in ("result.copy", "session.stop", "session.start", "rewrite.retry") for call in delta), delta
                    return {"actionCommands": actions, "hostReceiptClaimed": False, "automaticRetry": False}
                run_case("07-unconfirmed-view-without-resend", unconfirmed, data=fixture("unconfirmed"), recorder=True)

                def pending(page):
                    expect(page.get_by_text("正在发送到输入位置…", exact=True)).to_be_visible()
                    expect(page.get_by_role("button", name="查看成稿", exact=True)).to_have_count(0)
                    expect(page.get_by_role("button", name="取消本次口述", exact=True)).to_be_disabled()
                    expect(page.get_by_role("button", name="结束本次口述", exact=True)).to_be_disabled()
                    expect(page.get_by_role("button", name=re.compile("复制|重试|再次|重发"))).to_have_count(0)
                    return {"visibleResultAction": False, "stopAndCancelDisabled": True}
                for state in ("prepared", "dispatched"):
                    run_case(f"08-pending-{state}-no-competing-action", pending, data=fixture(state), recorder=True)

                def admin_recorder(page):
                    expect(page.get_by_text("目标应用以管理员权限运行，本次未自动输入。成稿已保留，请查看成稿后手动粘贴。", exact=True)).to_be_visible()
                    page.get_by_role("button", name="查看成稿", exact=True).click()
                    page.wait_for_function("window.__speakmodeQA.ledger.some(call=>call.name==='app.showMain')")
                    assert not re.search("自动提权|以管理员身份运行|自动重试", page.locator(".recorder-assembly").inner_text())
                    return {"reason": "targetHigherIntegrity", "canViewOutput": True, "automaticElevation": False}
                run_case("09-admin-target-recorder-view", admin_recorder, data=fixture("skipped", "targetHigherIntegrity"), recorder=True)

                def admin_main(page):
                    recovery = page.get_by_role("region", name="当前口述状态", exact=True)
                    expect(recovery).to_contain_text("目标应用以管理员权限运行")
                    expect(recovery.get_by_role("button", name="复制成稿", exact=True)).to_be_enabled()
                    recovery.get_by_role("button", name="查看成稿", exact=True).click()
                    expect(page.locator(".history-current")).to_contain_text("🙂进行测试。余下内容保持不变。")
                    expect(page.get_by_role("button", name="复制文本", exact=True)).to_be_enabled()
                    before = len(command_ledger(page))
                    page.get_by_role("button", name="复制文本", exact=True).click()
                    page.wait_for_function("window.__speakmodeQA.ledger.some(call=>call.name==='result.copy')")
                    actions = [call for call in command_ledger(page)[before:] if call["name"] not in ("history.get", "state.snapshot", "audio.listDevices", "history.list")]
                    assert [call["name"] for call in actions] == ["result.copy"], actions
                    return {"copyCommand": actions[0], "retainedFinalTextVisible": True, "nativeClipboardTouched": False}
                run_case("10-admin-target-main-retains-output", admin_main, data=fixture("skipped", "targetHigherIntegrity"))
                receipt["status"] = "PASS" if all(row["status"] == "PASS" for row in rows) else "FAIL"
        except Exception as error:
            receipt["status"] = "HARNESS_ERROR"
            receipt["error"] = str(error)
            receipt["traceback"] = traceback.format_exc()
        finally:
            browser.close()

    receipt["pageErrors"] = errors
    receipt["blockedExternalRequests"] = blocked_requests
    if errors or blocked_requests:
        receipt["status"] = "FAIL"
    receipt["completedAt"] = datetime.now(timezone.utc).isoformat()
    receipt["counts"] = {"registered": len(rows), "passed": sum(row["status"] == "PASS" for row in rows), "failed": sum(row["status"] == "FAIL" for row in rows)}
    write_new(args.out / "receipt.json", receipt, json_value=True)
    print(json.dumps({key: receipt.get(key) for key in ("status", "out", "browserVersion", "counts", "pageErrors", "blockedExternalRequests", "error")}, ensure_ascii=False, indent=2))
    return 0 if receipt["status"] in ("PASS", "INSPECTION_ONLY") else 1


if __name__ == "__main__":
    sys.exit(main())
