"""Recorder Mode regression checks against the production React build.

Uses isolated synthetic sessions and the existing restore-delivery test bridge.
Native resize requests drive a real browser viewport through a queued harness
acknowledgement. This is NOT native transparent-window compositor acceptance,
microphone/provider testing, or permission to read a user's profile.
"""
from __future__ import annotations

import argparse
from collections import deque
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import time
import traceback
from datetime import datetime, timezone
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("restore_delivery_qa", ROOT / "scripts/restore-delivery-browser-qa.py")
base = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(base)

LONG_NAME = "研究项目会议记录与跨团队同步专用模式这是一段很长的合成名称"


def fixture(*, personal=0, caption=False, error=False, limit=False):
    data = base.fixture()
    snapshot = data["snapshot"]
    for index, mode in enumerate(snapshot["modes"]):
        mode["revision"] = index + 11
    snapshot["modes"].extend({"id": f"synthetic-personal-{i}", "name": LONG_NAME if i == 0 else f"合成个人模式 {i + 1:02}",
                              "baseMode": "structure", "rules": "仅供隔离浏览器验证。", "revision": 27 + i, "builtin": False}
                             for i in range(personal))
    snapshot["session"] = {"sessionId": "synthetic-recording-session", "generation": 7, "state": "recording",
                           "modeSnapshot": {"modeId": "auto", "name": "自动", "baseMode": "auto", "rules": "保留原意", "revision": 11},
                           "level": 0.45, "items": [], "captions": [], "workerReleased": False}
    if caption:
        snapshot["session"]["captions"] = [{"id": "synthetic-caption", "text": "这是合成字幕，验证展开阅读与模式选择同时出现时的空间分配。" * 7,
                                              "revision": 1, "final": True, "captureOrder": 0}]
    if error:
        snapshot["session"]["error"] = "合成服务提示：请先选择 Key 所属的服务地域。"
    if limit:
        snapshot["session"]["recordingLimitRemainingMs"] = 9000
    return data


EXTENSION = r"""
(() => {
 const data=__FIXTURE__, copy=value=>structuredClone(value), qa=window.__speakmodeQA, native=window.__TAURI_INTERNALS__;
 const originalInvoke=native.invoke, channels=[], pending=new Map();
 let resizeId=0, active=0, sequence=data.snapshot.sequence;
 qa.fixtureVersion='recorder-mode-browser-v1';qa.animations=[];qa.maxResizeInFlight=0;qa.resizeAcks=[];
 qa.resolveResize=id=>{const finish=pending.get(id);if(!finish)throw Error('Unknown synthetic resize receipt');pending.delete(id);active--;qa.resizeAcks.push(id);finish(null);};
 qa.emit=(kind,payload)=>{const event={kind,payload:copy(payload),sequence:++sequence};data.snapshot.sequence=sequence;for(const channel of channels)channel.onmessage(copy(event));};
 qa.openFromTray=()=>qa.emit('recorder.openMode',{sessionId:data.snapshot.session.sessionId,generation:data.snapshot.session.generation});
 qa.updateSession=patch=>{Object.assign(data.snapshot.session,copy(patch));qa.emit('session',data.snapshot.session);};
 native.invoke=async(command,args={})=>{
  if(command==='subscribe_state'){channels.push(args.channel);return copy(data.snapshot);}
  const request=args.request;
  if(command==='app_command'&&request?.name==='state.snapshot'){
   qa.ledger.push(copy(request));return copy(data.snapshot);
  }
  if(command==='app_command'&&request?.name==='window.resizeRecorder'){
   const payload=copy(request.payload);qa.ledger.push(copy(request));const id=++resizeId;
   active++;qa.maxResizeInFlight=Math.max(qa.maxResizeInFlight,active);
   return new Promise(resolve=>{pending.set(id,resolve);void window.__requestSyntheticResize({id,...payload});});
  }
  if(command==='app_command'&&request?.name==='session.selectMode'){
   qa.ledger.push(copy(request));const p=request.payload,s=data.snapshot.session,m=data.snapshot.modes.find(mode=>mode.id===p.modeId);
   if(p.sessionId!==s.sessionId||p.expectedGeneration!==s.generation||!m||m.revision!==p.revision)throw Error('Wrong synthetic mode/session/revision');
   s.modeSnapshot={modeId:m.id,name:m.name,baseMode:m.baseMode,rules:m.rules,revision:m.revision};qa.emit('session',s);return null;
  }
  if(command==='app_command'&&request?.name==='session.captionRendered'){qa.ledger.push(copy(request));return null;}
  return originalInvoke(command,args);
 };
 const tracked=new WeakSet();
 new MutationObserver(()=>{
  const menu=document.querySelector('.recorder-mode-menu');if(!menu||tracked.has(menu))return;tracked.add(menu);
  const trace={frames:[],keyframes:[],startedAt:performance.now()};qa.animations.push(trace);
  const sample=()=>{
   if(!menu.isConnected)return;const style=getComputedStyle(menu);
   const rect=menu.getBoundingClientRect();
   trace.frames.push({at:performance.now()-trace.startedAt,opacity:Number(style.opacity),background:style.backgroundColor,visibility:style.visibility,transform:style.transform,animationName:style.animationName,animationDuration:style.animationDuration,transitionProperty:style.transitionProperty,transitionDuration:style.transitionDuration,
     menu:{x:rect.x,y:rect.y,width:rect.width,height:rect.height,right:rect.right,bottom:rect.bottom},viewport:{width:innerWidth,height:innerHeight},resizeAckCount:qa.resizeAcks.length});
   for(const animation of menu.getAnimations())if(animation.effect){const frames=animation.effect.getKeyframes();if(!trace.keyframes.some(old=>JSON.stringify(old)===JSON.stringify(frames)))trace.keyframes.push(frames);}
   if(performance.now()-trace.startedAt<260)requestAnimationFrame(sample);
  };sample();
 }).observe(document,{subtree:true,childList:true});
})();
"""


class Surface:
    def __init__(self, page, queue):
        self.page, self.queue, self.resizes = page, queue, []

    def settle(self, *, minimum=0.12, timeout=5):
        """Apply viewport changes outside the binding callback, then acknowledge IPC."""
        start, last = time.monotonic(), time.monotonic()
        while time.monotonic() - start < timeout:
            while self.queue:
                request = self.queue.popleft()
                assert request["width"] in (336, 344) and 66 <= request["height"] <= 420, request
                self.page.set_viewport_size({"width": request["width"], "height": request["height"]})
                self.page.evaluate("id=>window.__speakmodeQA.resolveResize(id)", request["id"])
                self.resizes.append(request)
                last = time.monotonic()
            self.page.wait_for_timeout(20)
            if time.monotonic() - last >= minimum:
                return
        raise TimeoutError("Synthetic native resize did not settle")

    def open(self, keyboard=False):
        trigger = self.page.locator(".recorder-mode-trigger")
        if keyboard:
            trigger.focus()
            trigger.press("Enter")
        else:
            trigger.click()
        self.settle(minimum=0.3)
        expect(self.page.locator(".recorder-mode-menu")).to_be_visible()

    def close(self):
        self.page.keyboard.press("Escape")
        self.settle()
        expect(self.page.locator(".recorder-mode-menu")).to_have_count(0)

    def ledger(self, name=None):
        ledger = base.command_ledger(self.page)
        return ledger if name is None else [call for call in ledger if call["name"] == name]

    def geometry(self):
        return self.page.evaluate("""()=>{
          const box=el=>{const r=el.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height,right:r.right,bottom:r.bottom};};
          const menu=document.querySelector('.recorder-mode-menu'),options=document.querySelector('.recorder-mode-options');
          return {viewport:{width:innerWidth,height:innerHeight},menu:menu?box(menu):null,options:options?{...box(options),clientHeight:options.clientHeight,scrollHeight:options.scrollHeight,scrollTop:options.scrollTop}:null,
            toolbar:box(document.querySelector('.recorder-toolbar')),document:{scrollX,scrollY,width:document.documentElement.scrollWidth,height:document.documentElement.scrollHeight}};
        }""")

    def assert_inside(self):
        bounds = self.geometry()
        for name in ("menu", "toolbar"):
            box = bounds[name]
            assert box and box["x"] >= -0.5 and box["y"] >= -0.5, bounds
            assert box["right"] <= bounds["viewport"]["width"] + 0.5 and box["bottom"] <= bounds["viewport"]["height"] + 0.5, bounds
        assert bounds["menu"]["bottom"] <= bounds["toolbar"]["y"] + 0.5, bounds
        assert bounds["document"]["scrollX"] == 0 and bounds["document"]["scrollY"] == 0, bounds
        return bounds


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True, help="Production preview URL at http://127.0.0.1")
    parser.add_argument("--out", required=True, type=Path, help="New absolute evidence directory")
    parser.add_argument("--serve", action="store_true", help="Own an isolated Vite preview process; reject occupied ports")
    args = parser.parse_args()
    parsed = urlparse(args.url)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or parsed.username or parsed.password or parsed.query or parsed.fragment:
        parser.error("Use an independent http://127.0.0.1 URL without query/fragment")
    if not args.out.is_absolute():
        parser.error("--out must be absolute")
    args.out.mkdir(parents=True, exist_ok=False)
    rows, errors, blocked = [], [], []
    receipt = {"status": "RUNNING", "scope": "Production React, isolated synthetic IPC and browser viewport. No native compositor, microphone, user profile, model call or host delivery acceptance.",
               "startedAt": datetime.now(timezone.utc).isoformat(), "out": str(args.out), "url": args.url, "tests": rows}
    sources = ["app/src/windows/RecorderWindow.tsx", "app/src/windows/recorder.css", "app/src/windows/useRecorderSize.ts", "scripts/recorder-mode-browser-qa.py"]
    receipt["sourceSha256"] = {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in sources}
    receipt["distSha256"] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted((ROOT / "app/dist").rglob("*")) if path.is_file()}
    with base.managed_preview(args.url, args.out, args.serve) as server, sync_playwright() as playwright:
        receipt["server"] = server
        browser = playwright.chromium.launch(channel="msedge", headless=True)
        receipt["browserVersion"] = browser.version
        try:
            def run(name, check, *, data=None, reduced=False):
                row = {"name": name, "status": "RUNNING"}
                rows.append(row)
                context = browser.new_context(viewport={"width": 336, "height": 66}, locale="zh-CN", reduced_motion="reduce" if reduced else "no-preference")
                queue = deque()
                context.expose_binding("__requestSyntheticResize", lambda source, request: queue.append(request))
                value = json.dumps(data or fixture(), ensure_ascii=False)
                context.add_init_script(base.BRIDGE.replace("__FIXTURE__", value) + EXTENSION.replace("__FIXTURE__", value))
                def route(request):
                    if urlparse(request.request.url).hostname not in ("127.0.0.1", None):
                        blocked.append(request.request.url);request.abort()
                    else:
                        request.continue_()
                context.route("**/*", route)
                page = context.new_page()
                page.on("pageerror", lambda error: errors.append({"test": name, "error": str(error)}))
                surface = Surface(page, queue)
                try:
                    page.goto(args.url.rstrip("/") + "/?window=recorder", wait_until="networkidle")
                    surface.settle()
                    expect(page.locator(".recorder-mode-trigger")).to_be_enabled()
                    row["observations"] = check(surface)
                    forbidden = [call for call in surface.ledger() if call["name"] in ("session.stop", "session.cancel", "session.start", "mode.setDefault")]
                    assert not forbidden, forbidden
                    assert page.evaluate("window.__speakmodeQA.maxResizeInFlight") == 1
                    row["status"] = "PASS"
                except Exception as error:
                    row.update(status="FAIL", error=str(error), traceback=traceback.format_exc())
                finally:
                    row["commands"] = surface.ledger()
                    row["resizeRequests"] = surface.resizes
                    row["animationTraces"] = page.evaluate("window.__speakmodeQA.animations")
                    row["finalFocus"] = page.evaluate("()=>{const el=document.activeElement;return {tag:el?.tagName,classes:el?.className,label:el?.getAttribute('aria-label'),text:el?.textContent?.slice(0,100)}}")
                    row["screenshot"] = base.screenshot(page, args.out / f"{name}-{row['status'].lower()}.png")
                    base.write_new(args.out / f"{name}.html", page.content())
                    context.close()
                print(f"{name}: {row['status']}", flush=True)

            def opaque(surface):
                surface.open()
                styles = surface.page.evaluate("""()=>{
                  const menu=document.querySelector('.recorder-mode-menu'),style=getComputedStyle(menu);
                  const shells=[document.documentElement,document.body,document.querySelector('#root')];
                  return {background:style.backgroundColor,opacity:style.opacity,shells:shells.map(el=>({name:el.tagName+'#'+el.id,background:getComputedStyle(el).backgroundColor})),
                    ancestors:[...function*(el){while(el){yield {tag:el.tagName,opacity:getComputedStyle(el).opacity};el=el.parentElement;}}(menu)]};
                }""")
                assert styles["background"] in ("rgb(255, 255, 255)", "rgba(255, 255, 255, 1)"), styles
                assert all(float(a["opacity"]) == 1 for a in styles["ancestors"]), styles
                assert all("rgba" in a["background"] and a["background"].endswith(", 0)") for a in styles["shells"]), styles
                geometry = surface.assert_inside()
                assert 176 <= geometry["menu"]["width"] <= 192 and geometry["menu"]["height"] <= 300, geometry
                choices = surface.page.locator(".recorder-mode-option")
                heights = choices.evaluate_all("els=>els.map(el=>el.getBoundingClientRect().height)")
                assert len(heights) == 3 and all(32 <= height <= 40 for height in heights), heights
                assert choices.locator("small").count() == 0
                assert all(title for title in choices.evaluate_all("els=>els.map(el=>el.title)"))
                return {"styles": styles, "geometry": geometry, "choiceHeights": heights}
            run("01-opaque-compact-menu", opaque)

            def select(surface, index, expected_id, revision):
                surface.open()
                surface.page.locator(".recorder-mode-option").nth(index).click()
                surface.settle()
                expect(surface.page.locator(".recorder-mode-menu")).to_have_count(0)
                calls = surface.ledger("session.selectMode")
                assert len(calls) == 1, calls
                p = calls[0]["payload"]
                assert p["sessionId"] == "synthetic-recording-session" and p["expectedGeneration"] == 7 and p["modeId"] == expected_id and p["revision"] == revision, p
                assert surface.resizes[-1]["menuOpen"] is False
                return {"request": p, "closedViewport": surface.page.viewport_size}
            run("02-select-current-mode", lambda s: select(s, 0, "auto", 11))
            run("03-select-other-mode", lambda s: select(s, 2, "structure", 13))
            run("04-select-long-personal-mode", lambda s: select(s, 3, "synthetic-personal-0", 27), data=fixture(personal=1))

            def arrows(surface):
                surface.open(keyboard=True)
                choices = surface.page.locator(".recorder-mode-option")
                expect(choices.first).to_be_focused()
                surface.page.keyboard.press("ArrowDown");expect(choices.nth(1)).to_be_focused()
                surface.page.keyboard.press("End");expect(choices.last).to_be_focused()
                end = surface.assert_inside()
                assert end["options"]["scrollTop"] > 0 and end["options"]["scrollHeight"] > end["options"]["clientHeight"], end
                final = choices.last.bounding_box()
                assert final["y"] + final["height"] <= end["options"]["bottom"] + 1, (final, end)
                surface.page.keyboard.press("Home");expect(choices.first).to_be_focused()
                assert surface.geometry()["options"]["scrollTop"] == 0
                surface.page.keyboard.press("ArrowUp");expect(choices.last).to_be_focused()
                surface.assert_inside()
                return {"endGeometry": end, "focusKeys": ["ArrowDown", "End", "Home", "ArrowUp"]}
            run("05-keyboard-bounded-option-scroll", arrows, data=fixture(personal=24))

            def escape(surface):
                surface.open();surface.close()
                expect(surface.page.locator(".recorder-mode-trigger")).to_be_focused()
                assert surface.page.viewport_size == {"width": 336, "height": 66}
                return {"focusReturned": True, "viewport": surface.page.viewport_size}
            run("06-escape-closes-without-stopping", escape)

            def blur(surface):
                surface.open()
                surface.page.evaluate("window.dispatchEvent(new Event('blur'))")
                surface.settle()
                expect(surface.page.locator(".recorder-mode-menu")).to_have_count(0)
                return {"scope": "Synthetic window blur event; no native OS focus acceptance", "closed": True}
            run("07-blur-closes-without-stopping", blur)

            def motion(surface, *, keyboard=False, tray=False, reduced=False):
                if tray:
                    surface.page.evaluate("window.__speakmodeQA.openFromTray()")
                    surface.settle(minimum=0.3)
                else:
                    surface.open(keyboard=keyboard)
                surface.assert_inside()
                traces = surface.page.evaluate("window.__speakmodeQA.animations")
                assert traces and traces[-1]["frames"], traces
                trace = traces[-1]
                assert all(frame["opacity"] == 1 for frame in trace["frames"]), trace
                partially_clipped = [frame for frame in trace["frames"] if frame["visibility"] == "visible"
                    and frame["menu"]["bottom"] > 0 and frame["menu"]["y"] < frame["viewport"]["height"]
                    and (frame["menu"]["x"] < -0.5 or frame["menu"]["y"] < -0.5
                         or frame["menu"]["right"] > frame["viewport"]["width"] + 0.5
                         or frame["menu"]["bottom"] > frame["viewport"]["height"] + 0.5)]
                assert not partially_clipped, {"partiallyClippedOpeningFrames": partially_clipped}
                if keyboard or tray or reduced:
                    assert all(frame["animationName"] == "none" for frame in trace["frames"]), trace
                    assert not trace["keyframes"], trace
                else:
                    assert any(frame["transform"] != "none" for frame in trace["frames"]), trace
                    assert any(len({frame.get("transform") for frame in frames}) > 1 for frames in trace["keyframes"]), trace
                    assert all(float(part.strip().removesuffix("s")) <= 0.22 for frame in trace["frames"] for part in (frame["animationDuration"] + "," + frame["transitionDuration"]).split(",")), trace
                    assert all("opacity" not in frame for frames in trace["keyframes"] for frame in frames), trace
                return {"input": "tray" if tray else "keyboard" if keyboard else "pointer", "reducedMotion": reduced, "frames": len(trace["frames"]), "opacityAlwaysOne": True}
            run("08-pointer-transform-only-animation", motion)
            run("09-keyboard-immediate-animation", lambda s: motion(s, keyboard=True))
            run("10-reduced-motion-immediate", lambda s: motion(s, reduced=True), reduced=True)
            run("11-tray-immediate-animation", lambda s: motion(s, tray=True))

            def rapid(surface):
                trigger = surface.page.locator(".recorder-mode-trigger")
                for _ in range(6):
                    trigger.press("Enter")
                    surface.page.keyboard.press("Escape")
                surface.settle()
                expect(surface.page.locator(".recorder-mode-menu")).to_have_count(0)
                surface.open()
                bounds = surface.assert_inside()
                surface.close()
                assert surface.resizes[-1]["menuOpen"] is False
                return {"cycles": 6, "maxResizeInFlight": surface.page.evaluate("window.__speakmodeQA.maxResizeInFlight"), "finalViewport": surface.page.viewport_size, "reopenedGeometry": bounds}
            run("12-rapid-open-close-final-state", rapid)

            def error_caption(surface):
                surface.page.get_by_role("button", name="查看当前识别字幕；点击展开或收起", exact=True).click()
                surface.settle()
                surface.open()
                bounds = surface.assert_inside()
                assert bounds["viewport"]["height"] <= 420
                assert bounds["options"]["clientHeight"] >= 30, bounds
                expect(surface.page.get_by_role("alert")).to_contain_text("合成服务提示")
                expect(surface.page.locator(".recorder-caption")).to_have_attribute("data-expanded", "true")
                surface.page.keyboard.press("End")
                expect(surface.page.locator(".recorder-mode-option").last).to_be_focused()
                surface.assert_inside()
                return {"geometry": bounds, "captionExpanded": True, "errorVisible": True}
            run("13-error-expanded-caption-bounds", error_caption, data=fixture(personal=24, caption=True, error=True))
            run("14-error-caption-limit-combined-bounds", error_caption, data=fixture(personal=24, caption=True, error=True, limit=True))

            def worst_tray(surface):
                surface.page.get_by_role("button", name="查看当前识别字幕；点击展开或收起", exact=True).click()
                surface.settle()
                surface.page.evaluate("window.__speakmodeQA.openFromTray()")
                surface.settle(minimum=0.3)
                bounds = surface.assert_inside()
                assert bounds["options"]["clientHeight"] >= 30, bounds
                surface.page.keyboard.press("End")
                option = surface.page.locator(".recorder-mode-option").last
                expect(option).to_be_focused()
                row = option.bounding_box()
                bounds = surface.assert_inside()
                assert row["y"] >= bounds["options"]["y"] - 0.5 and row["y"] + row["height"] <= bounds["options"]["bottom"] + 0.5, (row, bounds)
                return {"geometry": bounds, "fullyVisibleFocusedLastRow": row, "tray": True, "expandedCaption": True, "multiLineError": True, "limitWarning": True}
            worst = fixture(personal=24, caption=True, error=True, limit=True)
            worst["snapshot"]["session"]["error"] = "合成服务提示：当前服务地域尚未选择，请打开主面板确认云端配置，完成后再继续录音。" * 3
            run("15-tray-all-warnings-minimum-choice-row", worst_tray, data=worst)

            def dynamic_focus(surface):
                surface.open(keyboard=True)
                surface.page.keyboard.press("End")
                choices=surface.page.locator(".recorder-mode-option")
                expect(choices.last).to_be_focused()
                surface.page.evaluate("""window.__speakmodeQA.updateSession({
                    captions:[{id:'dynamic-synthetic-caption',text:'合成字幕更新，不应打断正在选择的模式。',revision:1,final:true,captureOrder:0}],
                    error:'合成动态提示：麦克风状态变更。'
                })""")
                surface.settle(minimum=0.3)
                expect(choices.last).to_be_focused()
                bounds=surface.assert_inside()
                row=choices.last.bounding_box()
                assert row["y"] >= bounds["options"]["y"] - 0.5 and row["y"] + row["height"] <= bounds["options"]["bottom"] + 0.5, (row,bounds)
                return {"focusedModeRetained": True, "focusedRow": row, "geometry": bounds}
            run("16-live-caption-error-keeps-choice-focus", dynamic_focus, data=fixture(personal=24))

            def visual_backdrop(surface):
                surface.page.evaluate("""()=>{
                 const sample=document.createElement('div');sample.id='synthetic-backdrop';sample.setAttribute('aria-hidden','true');
                 sample.style.cssText='position:fixed;inset:0;z-index:-1;background:repeating-linear-gradient(0deg,#d4dce8 0,#d4dce8 1px,#f5f7fb 1px,#f5f7fb 28px);font:12px sans-serif;color:#7a465a;padding:12px;pointer-events:none';
                 sample.textContent='SYNTHETIC BACKGROUND · 合成背景文字，仅供透明回归检查';document.body.prepend(sample);
                }""")
                surface.open()
                bounds=surface.assert_inside()
                return {"syntheticBackdrop": True, "geometry": bounds, "nativeCompositorVerified": False}
            run("17-synthetic-background-visual", visual_backdrop)
            receipt["status"] = "PASS" if all(row["status"] == "PASS" for row in rows) else "FAIL"
        except Exception as error:
            receipt.update(status="HARNESS_ERROR", error=str(error), traceback=traceback.format_exc())
        finally:
            browser.close()
    receipt["pageErrors"], receipt["blockedExternalRequests"] = errors, blocked
    if errors or blocked:
        receipt["status"] = "FAIL"
    receipt["counts"] = {"registered": len(rows), "passed": sum(row["status"] == "PASS" for row in rows), "failed": sum(row["status"] == "FAIL" for row in rows)}
    receipt["completedAt"] = datetime.now(timezone.utc).isoformat()
    base.write_new(args.out / "receipt.json", receipt, json_value=True)
    print(json.dumps({key: receipt.get(key) for key in ("status", "counts", "out", "pageErrors", "blockedExternalRequests", "error")}, ensure_ascii=False, indent=2))
    return 0 if receipt["status"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
