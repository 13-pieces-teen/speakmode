"""Exercise the production settings UI with an isolated synthetic Tauri bridge.

Browser reload persistence below belongs to the synthetic bridge's localStorage,
not native SQLite. Native persistence is covered by the separate Rust tests.
No real profile, microphone, provider, credentials, clipboard or global input.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time
import traceback
from urllib.parse import urlparse

from playwright.sync_api import sync_playwright, expect

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("settings_qa", ROOT / "scripts/ui-experience-browser-qa.py")
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)


def bridge(data):
    source = qa.BRIDGE.replace("__FIXTURE__", json.dumps(data, ensure_ascii=False))
    source = source.replace(" window.isTauri=true;", """
 const storageKey='speakmode-soft-voice-synthetic-preferences';
 const saved=JSON.parse(localStorage.getItem(storageKey)||'null');
 if(saved)fixture.snapshot.preferences=saved;
 const persist=()=>localStorage.setItem(storageKey,JSON.stringify(fixture.snapshot.preferences));
 window.isTauri=true;""")
    source = source.replace("fixture.snapshot.sequence++;return null;", "fixture.snapshot.sequence++;persist();return null;")
    source = source.replace("  if(name==='preferences.update'){", """
  if(name==='preferences.reset'){
   if(payload.expectedVersion!==fixture.snapshot.preferences.version)throw new Error('Synthetic stale preference version');
   fixture.snapshot.preferences={version:payload.expectedVersion+1,shortcut:'alt_right',microphoneId:null,language:'auto',defaultModeId:'auto',deliveryMode:'direct',softVoiceEnabled:false,keepHistory:true,launchAtLogin:false};
   fixture.snapshot.sequence++;persist();return copy(fixture.snapshot.preferences);
  }
  if(name==='preferences.update'){""")
    return source


@contextmanager
def managed_dev(url, out):
    """Own the Node PID directly, avoiding Windows shell grandchildren."""
    parsed = urlparse(url)
    port = parsed.port or 80
    def listening():
        with socket.socket() as sock:
            sock.settimeout(0.2)
            return sock.connect_ex(("127.0.0.1", port)) == 0
    if listening():
        raise RuntimeError(f"Port {port} already occupied; refusing to reuse or stop its owner")
    command = [shutil.which("node.exe") or shutil.which("node"), str(ROOT / "app/node_modules/vite/bin/vite.js"), "--host", "127.0.0.1", "--port", str(port), "--strictPort"]
    info = {"command": command, "scope": "Vite dev serves current source; no production build"}
    with (out / "dev-server.log").open("x", encoding="utf-8") as log:
        process = subprocess.Popen(command, cwd=ROOT / "app", stdout=log, stderr=subprocess.STDOUT,
                                   shell=False, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        info["pid"] = process.pid
        try:
            deadline = time.monotonic() + 30
            while not listening():
                if process.poll() is not None:
                    raise RuntimeError(f"Vite dev exited with {process.returncode}")
                if time.monotonic() > deadline:
                    raise TimeoutError("Vite dev did not become ready")
                time.sleep(0.1)
            yield info
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            info.update(exitCode=process.returncode, listeningAfterExit=listening())


def open_settings(page):
    page.get_by_role("navigation", name="主导航").get_by_role("button", name="设置", exact=True).click()
    expect(page.get_by_role("switch", name="轻语增强", exact=True)).to_be_visible()


def snapshot(page, out, tag):
    toggle = page.get_by_role("switch", name="轻语增强", exact=True)
    toggle.scroll_into_view_if_needed()
    values = page.evaluate("""() => {
      const main=document.querySelector('.app-main'),toggle=document.getElementById('soft-voice-enabled');
      const shown=el=>{const r=el.getBoundingClientRect();return r.width>0&&r.height>0&&getComputedStyle(el).visibility!=='hidden';};
      const overflow=[...document.querySelectorAll('.settings-page button,.settings-page input,.settings-page select,.settings-page [role="combobox"],.settings-page .setting-row-copy')].filter(shown).map(el=>({text:el.getAttribute('aria-label')||el.textContent,rect:el.getBoundingClientRect().toJSON()})).filter(x=>x.rect.left < -1||x.rect.right>innerWidth+1);
      const box=toggle.getBoundingClientRect(),mainBox=main.getBoundingClientRect();
      return {width:innerWidth,height:innerHeight,documentWidth:document.documentElement.scrollWidth,overflow,
        toggleVisible:box.top>=mainBox.top-1&&box.bottom<=mainBox.bottom+1,
        toggle:box.toJSON(),focused:document.activeElement?.id,checked:toggle.getAttribute('aria-checked')};
    }""")
    assert values["documentWidth"] <= values["width"] + 1 and not values["overflow"] and values["toggleVisible"], values
    page.screenshot(path=str(out / f"{tag}.png"))
    qa.base.write_new(out / f"{tag}.html", page.content())
    qa.base.write_new(out / f"{tag}.json", values, json_value=True)
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    parsed = urlparse(args.url)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not args.out.is_absolute():
        parser.error("Requires a dedicated 127.0.0.1 URL and fresh absolute --out")
    args.out.mkdir(parents=True, exist_ok=False)
    rows, errors, console_errors, external = [], [], [], []
    files = ["app/src/pages/SettingsPage.tsx", "app/src/preferences.ts", "app/src/services/contracts.ts", "app/src/components/SettingsControls.tsx", "app/src/pages/settings.css"]
    receipt = {"scope": __doc__, "tests": rows, "pageErrors": errors, "consoleErrors": console_errors, "blockedExternalRequests": external,
               "sourceSha256": {name: hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in files},
               "scriptSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    with managed_dev(args.url, args.out) as server, sync_playwright() as playwright:
        receipt["server"] = server
        browser = playwright.chromium.launch(channel="msedge", headless=True)
        receipt["browserVersion"] = browser.version
        try:
            for width, height in [(1280, 800), (800, 600)]:
                tag=f"{width}x{height}"
                row={"name":tag,"status":"RUNNING"};rows.append(row)
                data=qa.fixture()
                data["snapshot"]["preferences"]["softVoiceEnabled"]=False
                context=browser.new_context(viewport={"width":width,"height":height},locale="zh-CN",reduced_motion="reduce")
                context.add_init_script(bridge(data))
                def route(r):
                    if urlparse(r.request.url).hostname not in ("127.0.0.1",None):external.append(r.request.url);r.abort()
                    else:r.continue_()
                context.route("**/*",route)
                page=context.new_page()
                page.on("pageerror",lambda e:errors.append(str(e)))
                page.on("console",lambda msg:console_errors.append(msg.text) if msg.type=="error" else None)
                try:
                    page.goto(args.url,wait_until="networkidle")
                    open_settings(page)
                    toggle=page.get_by_role("switch",name="轻语增强",exact=True)
                    expect(toggle).not_to_be_checked()
                    expect(page.get_by_text("适合安静环境下轻声输入，下次录音生效。",exact=True)).to_be_visible()
                    row["default"]=snapshot(page,args.out,tag+"-default")
                    # Verify reachable tab order as well as Space activation of the real Radix switch.
                    page.get_by_role("combobox",name="麦克风",exact=True).focus()
                    page.keyboard.press("Tab")
                    expect(toggle).to_be_focused()
                    page.keyboard.press("Space")
                    expect(toggle).to_be_checked()
                    expect(page.locator('[data-setting-feedback="softVoiceEnabled"]')).to_have_attribute("data-feedback-state","saved")
                    before=page.evaluate("structuredClone(window.__speakmodeQA.fixture.snapshot.preferences)")
                    assert before["softVoiceEnabled"] is True and before["defaultModeId"]=="auto",before
                    row["enabled"]=snapshot(page,args.out,tag+"-enabled")
                    # Output Mode changes are independent of the input enhancement flag.
                    page.get_by_role("combobox",name="新口述默认 Mode",exact=True).click()
                    page.get_by_role("option",name="结构",exact=True).click()
                    expect(page.get_by_role("combobox",name="新口述默认 Mode",exact=True)).to_contain_text("结构")
                    expect(toggle).to_be_checked()
                    page.reload(wait_until="networkidle")
                    open_settings(page)
                    expect(toggle).to_be_checked()
                    expect(page.get_by_role("combobox",name="新口述默认 Mode",exact=True)).to_contain_text("结构")
                    row["reload"]=snapshot(page,args.out,tag+"-reload")
                    # A rejected native command must not show a successful toggle or reset the Mode.
                    page.evaluate("window.__speakmodeQA.failNextPreference=true")
                    toggle.click()
                    expect(toggle).to_be_checked()
                    expect(page.locator('[data-setting-feedback="softVoiceEnabled"]')).to_have_attribute("data-feedback-state","error")
                    expect(toggle).to_be_enabled()
                    toggle.click()
                    expect(toggle).not_to_be_checked()
                    expect(page.get_by_role("combobox",name="新口述默认 Mode",exact=True)).to_contain_text("结构")
                    toggle.click()
                    expect(toggle).to_be_checked()
                    page.get_by_role("button",name="恢复默认设置",exact=True).click()
                    expect(toggle).not_to_be_checked()
                    expect(page.get_by_role("combobox",name="新口述默认 Mode",exact=True)).to_contain_text("自动")
                    row["reset"]=snapshot(page,args.out,tag+"-reset")
                    row["ledger"]=page.evaluate("window.__speakmodeQA.ledger")
                    assert not any(e["name"].startswith(("session.","credential.")) or e["name"] in ("result.copy","mode.preview") for e in row["ledger"]),row["ledger"]
                    row["checks"]=["default off","keyboard tab and space","save acknowledgement","Mode independent","synthetic reload persistence","save rejection leaves current state","disable preserves Mode","reset","responsive no overflow"]
                    row["status"]="PASS"
                except Exception as error:
                    row.update(status="FAIL",error=str(error),traceback=traceback.format_exc())
                    page.screenshot(path=str(args.out/(tag+"-failure.png")))
                    qa.base.write_new(args.out/(tag+"-failure.html"),page.content())
                finally:
                    context.close()
        finally:
            browser.close()
    receipt["status"]="PASS" if rows and all(row["status"]=="PASS" for row in rows) and not errors and not console_errors and not external else "FAIL"
    qa.base.write_new(args.out/"receipt.json",receipt,json_value=True)
    print(json.dumps({key:receipt[key] for key in ("status","pageErrors","consoleErrors","blockedExternalRequests")},ensure_ascii=False,indent=2))
    print(json.dumps([{key:row[key] for key in ("name","status","error") if key in row} for row in rows],ensure_ascii=False,indent=2))
    return 0 if receipt["status"]=="PASS" else 1


if __name__=="__main__":
    sys.exit(main())
