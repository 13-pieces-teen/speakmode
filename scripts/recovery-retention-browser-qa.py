"""Real headless Edge checks for retained recovery using synthetic Tauri IPC.

The production React bundle is served only on 127.0.0.1. No native app,
microphone, clipboard, credentials, user profile or provider is accessed.
Activation and expiry are synthetic backend responses, not native retention
acceptance. Every evidence directory must be new; failed runs are preserved.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import sys
import traceback
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import sync_playwright, expect

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("restore_delivery_qa", ROOT / "scripts/restore-delivery-browser-qa.py")
_shared = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_shared)
managed_preview, screenshot, write_new = _shared.managed_preview, _shared.screenshot, _shared.write_new
RETAINED_ID, CURRENT_ID = "synthetic-retained", "synthetic-current"
READS = {"state.snapshot", "audio.listDevices", "history.list", "history.get", "personalization.get", "usage.today"}


def live(session_id, generation, state="failed", *, released=True):
    return {"id": session_id, "sessionId": session_id, "generation": generation,
            "state": state, "workerReleased": released, "modeSnapshot": {
                "modeId": "auto", "name": "自动", "baseMode": "auto", "rules": "保留原意", "revision": 1},
            "partial": "这是合成的待恢复口述。", "captions": [], "canRetryAsr": state == "failed",
            "canRetryRewrite": False, "canUseSource": False, "canCopyRecovery": state == "failed",
            "retryKind": "asr" if state == "failed" else None,
            "asrAttemptId": "synthetic-attempt-" + session_id, "recoverableContentId": session_id,
            "errorCode": "asr_incomplete" if state == "failed" else None,
            "error": "合成识别未完成，音频已暂存。" if state == "failed" else None}


def fixture(*, current_state="ready", released=True, retained=True, activate_error=False):
    snapshot = _shared.fixture()["snapshot"]
    snapshot.update({"session": live(CURRENT_ID, 42, current_state, released=released),
                     "recovery": {"session": live(RETAINED_ID, 41),
                                  "expiresAt": (datetime.now(timezone.utc) + timedelta(minutes=8)).isoformat()}
                     if retained else None})
    return {"snapshot": snapshot, "activateError": activate_error,
            "currentRecoveryExpiresAt": (datetime.now(timezone.utc) + timedelta(minutes=7)).isoformat()}


BRIDGE = r"""
(() => {
 const data=__FIXTURE__, copy=value=>structuredClone(value), ledger=[], events=[], callbacks=new Map(), channels=new Map();
 const entries=()=>data.snapshot.recoveries??(data.snapshot.recovery?[data.snapshot.recovery]:[]);
 const recoveryPatch=values=>({recoveries:values,recovery:values[0]??null});
 let callbackId=0;
 const liveNext=generation=>({sessionId:'synthetic-next',id:'synthetic-next',generation,state:'recording',workerReleased:false,captureReady:true});
 window.isTauri=true;
 function emit(event) {
  events.push(copy(event));
  for(const [id,index] of channels) {
   const callback=callbacks.get(id);if(!callback)continue;
   channels.set(id,index+1);callback({index,message:copy(event)});
  }
 }
 function publishSnapshot(patch) {
  Object.assign(data.snapshot,copy(patch));data.snapshot.sequence++;
  emit({kind:'snapshot',sequence:data.snapshot.sequence,payload:copy(data.snapshot)});
 }
 function publishSession(session) {
  data.snapshot.sequence++;
  emit({kind:'session',sequence:data.snapshot.sequence,sessionId:session.sessionId,generation:session.generation,payload:copy(session)});
 }
 window.__speakmodeQA={ledger,events,data,publishSnapshot,publishSession,expire(){
  Object.assign(data.snapshot,recoveryPatch([]));data.snapshot.sequence++;
  emit({kind:'recovery.changed',sequence:data.snapshot.sequence,payload:{expired:true}});
 }};
 window.__TAURI_INTERNALS__={
  transformCallback(fn){const id=++callbackId;callbacks.set(id,fn);return id;},
  unregisterCallback(id){callbacks.delete(id);channels.delete(id);},
  async invoke(command,args={}) {
   if(command==='subscribe_state'){channels.set(args.channel.id,0);return copy(data.snapshot);}
   if(command==='control_main_window')return {maximized:false,minimized:false};
   if(command!=='app_command')throw new Error('Blocked synthetic IPC: '+command);
   const {name,payload}=args.request;ledger.push({name,payload:copy(payload)});
   if(name==='state.snapshot')return copy(data.snapshot);
   if(name==='audio.listDevices')return [];
   if(name==='personalization.get')return {version:0,enabled:false,globalHabits:'',profiles:[]};
   if(name==='usage.today')return copy(data.snapshot.usage);
   if(name==='history.list')return {items:[],total:0,nextCursor:null};
   if(name==='recovery.activate') {
    if(data.activateError)throw {code:'stale_recovery',message:'这条恢复已变化，请刷新后重试。'};
    const retained=entries().find(item=>item.session.sessionId===payload.sessionId);
    if(!retained||payload.sessionId!==retained.session.sessionId||payload.expectedGeneration!==retained.session.generation)
     throw new Error('Synthetic stale recovery identity');
    const active=data.snapshot.session;
    if(active&&(!active.workerReleased||['starting','recording','finalizingasr','rewriting'].includes(active.state.toLowerCase())))
     throw new Error('Synthetic active worker is busy');
    const restored=copy(retained.session);if(!restored.sourceRevisionId)restored.generation=Math.max(restored.generation,active?.generation??0)+1;
    const remaining=entries().filter(item=>item!==retained);
    if(active&&['failed','incomplete'].includes(active.state.toLowerCase())&&(active.canRetryAsr||active.canCopyRecovery||active.canRetrySave||active.canRetryRewrite))remaining.push({session:copy(active),expiresAt:data.currentRecoveryExpiresAt});
    publishSnapshot({session:restored,...recoveryPatch(remaining)});return copy(restored);
   }
   if(name==='recovery.discard') {
    const retained=entries().find(item=>item.session.sessionId===payload.sessionId);
    if(!retained||payload.sessionId!==retained.session.sessionId||payload.expectedGeneration!==retained.session.generation)
     throw new Error('Synthetic stale discard identity');
    publishSnapshot(recoveryPatch(entries().filter(item=>item!==retained)));return null;
   }
   if(name==='session.start') {
    const active=data.snapshot.session;
    if(!active.workerReleased)throw new Error('Synthetic resources not released');
    const remaining=entries();remaining.push({session:copy(active),expiresAt:data.currentRecoveryExpiresAt});
    const next=liveNext(active.generation+1);
    publishSnapshot({session:next,...recoveryPatch(remaining)});return copy(next);
   }
   if(name==='asr.retry') {
    const session=data.snapshot.session;
    if(payload.sessionId!==session.sessionId||payload.expectedGeneration!==session.generation||payload.failedAttemptId!==session.asrAttemptId)
     throw new Error('Synthetic stale retry identity');
    publishSnapshot({session:{...session,generation:session.generation+1,state:'finalizingAsr',workerReleased:false,
     error:null,retryKind:null,canRetryAsr:false,canCopyRecovery:false}});return copy(data.snapshot.session);
   }
   throw new Error('Blocked synthetic app command: '+name);
  }
 };
})();
"""


def ledger(page):
    return page.evaluate("window.__speakmodeQA.ledger")


def actions(page):
    return [row for row in ledger(page) if row["name"] not in READS]


def retained_card(page):
    return page.get_by_role("region", name="待恢复口述", exact=True)


def current_card(page):
    return page.get_by_role("region", name="当前口述状态", exact=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True, help="Independent production preview on http://127.0.0.1")
    parser.add_argument("--out", required=True, type=Path, help="New absolute evidence directory")
    parser.add_argument("--serve", action="store_true", help="Own and clean up a Vite preview process; refuse occupied ports")
    parser.add_argument("--inspect-only", action="store_true", help="Capture initial DOM and controls without action assertions")
    parser.add_argument("--case", action="append", dest="cases", help="Run only this exact named case; repeat to select multiple")
    args = parser.parse_args()
    url = urlparse(args.url)
    if url.scheme != "http" or url.hostname != "127.0.0.1" or url.username or url.password:
        parser.error("--url must be an independent http://127.0.0.1 URL")
    if not args.out.is_absolute():
        parser.error("--out must be absolute")
    args.out.mkdir(parents=True, exist_ok=False)
    rows, errors, blocked = [], [], []
    sources = ["app/src/components/RetainedRecovery.tsx", "app/src/components/SessionRecovery.tsx",
               "app/src/services/DesktopProvider.tsx", "app/src/services/contracts.ts", "app/src/App.tsx"]
    receipt = {"status": "RUNNING", "scope": "Production React, actual browser clicks and subscribed Tauri Channel callbacks; synthetic backend only. No native retention, credentials, model, audio, clipboard or host-input acceptance.",
               "startedAt": datetime.now(timezone.utc).isoformat(), "url": args.url, "out": str(args.out), "tests": rows,
               "sourceSha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() if (ROOT / name).exists() else None for name in sources},
               "distSha256": {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted((ROOT / "app/dist").rglob("*")) if path.is_file()},
               "scriptSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    with managed_preview(args.url, args.out, args.serve) as server, sync_playwright() as playwright:
        receipt["server"] = server
        browser = playwright.chromium.launch(channel="msedge", headless=True)
        receipt["browserVersion"] = browser.version
        try:
            def run_case(name, fn, data=None):
                if args.cases and name not in args.cases:
                    return
                context = browser.new_context(viewport={"width": 1440, "height": 1080}, locale="zh-CN")
                context.add_init_script(BRIDGE.replace("__FIXTURE__", json.dumps(data or fixture(), ensure_ascii=False)))
                def route(request_route):
                    host = urlparse(request_route.request.url).hostname
                    if host not in ("127.0.0.1", None):
                        blocked.append(request_route.request.url);request_route.abort()
                    else:
                        request_route.continue_()
                context.route("**/*", route)
                page = context.new_page()
                page.set_default_timeout(5000)
                page.on("pageerror", lambda error: errors.append({"case": name, "error": str(error)}))
                row = {"name": name, "status": "RUNNING"};rows.append(row)
                try:
                    page.goto(args.url, wait_until="networkidle")
                    page.wait_for_load_state("networkidle")
                    screenshot(page, args.out / f"{name}-initial.png")
                    write_new(args.out / f"{name}-initial.html", page.content())
                    write_new(args.out / f"{name}-controls.json", page.locator("button,[role=region]").evaluate_all(
                        "els=>els.map(e=>({tag:e.tagName,role:e.getAttribute('role'),label:e.getAttribute('aria-label'),text:e.textContent,disabled:e.disabled}))"), json_value=True)
                    row["observations"] = fn(page) if not args.inspect_only else {"inspectionOnly": True}
                    row["status"] = "PASS" if not args.inspect_only else "INSPECTION_ONLY"
                except Exception as error:
                    row.update({"status": "FAIL", "error": str(error), "traceback": traceback.format_exc()})
                finally:
                    row["commands"] = ledger(page)
                    row["events"] = page.evaluate("window.__speakmodeQA.events")
                    row["screenshot"] = screenshot(page, args.out / f"{name}-{row['status'].lower()}.png")
                    write_new(args.out / f"{name}-final.html", page.content())
                    context.close()

            def visible(page):
                expect(retained_card(page)).to_be_visible()
                expect(retained_card(page)).to_contain_text("上一条口述可恢复")
                expect(retained_card(page).get_by_role("button", name="打开恢复", exact=True)).to_be_enabled()
                assert actions(page) == []
                return {"cardVisible": True, "automaticCommands": []}
            run_case("01-retained-card-no-automatic-work", visible)

            def activate_retry(page):
                retained_card(page).get_by_role("button", name="打开恢复", exact=True).click()
                expect(retained_card(page)).to_have_count(0)
                expect(current_card(page).get_by_role("button", name="重试识别", exact=True)).to_be_enabled()
                before = actions(page)
                assert [row["name"] for row in before] == ["recovery.activate"], before
                assert before[0]["payload"]["sessionId"] == RETAINED_ID
                assert before[0]["payload"]["expectedGeneration"] == 41
                current_card(page).get_by_role("button", name="重试识别", exact=True).click()
                expect(current_card(page)).to_contain_text("正在转成文字")
                calls = actions(page)
                assert [row["name"] for row in calls] == ["recovery.activate", "asr.retry"], calls
                assert calls[1]["payload"]["sessionId"] == RETAINED_ID
                assert calls[1]["payload"]["expectedGeneration"] == 43
                return {"actions": calls, "automaticCloudOnActivation": False}
            run_case("02-activate-before-explicit-retry", activate_retry)

            def discard(page):
                retained_card(page).get_by_role("button", name="放弃这条恢复", exact=True).click()
                expect(page.get_by_role("button", name="确认放弃", exact=True)).to_be_visible()
                assert actions(page) == []
                page.get_by_role("button", name="确认放弃", exact=True).click()
                expect(retained_card(page)).to_have_count(0)
                calls = actions(page)
                assert [row["name"] for row in calls] == ["recovery.discard"], calls
                assert calls[0]["payload"]["sessionId"] == RETAINED_ID
                assert calls[0]["payload"]["expectedGeneration"] == 41
                return {"twoStepConfirmation": True, "actions": calls}
            run_case("03-discard-requires-second-click", discard)

            def busy(page):
                expect(retained_card(page)).to_be_visible()
                expect(retained_card(page).get_by_role("button", name="打开恢复", exact=True)).to_be_disabled()
                expect(retained_card(page).get_by_role("button", name="放弃这条恢复", exact=True)).to_be_enabled()
                expect(current_card(page)).to_contain_text("准备录音")
                discard(page)
                expect(current_card(page)).to_contain_text("准备录音")
                return {"openDisabledDuringRecording": True, "discardPreservesCurrentRecording": True}
            run_case("04-new-recording-keeps-recovery", busy, fixture(current_state="recording", released=False))

            def unresolved_current(page):
                expect(retained_card(page)).to_be_visible()
                expect(retained_card(page).get_by_role("button", name="打开恢复", exact=True)).to_be_disabled()
                # The worker lease is independent of the displayed terminal phase.
                expect(retained_card(page)).to_contain_text("释放资源")
                checks = ["ready-with-worker-held"]
                for retry_kind in ["save", "rewrite"]:
                    session = live(CURRENT_ID, 42)
                    session.update({"canRetryAsr": False, "retryKind": retry_kind,
                                    "canRetrySave": retry_kind == "save", "canRetryRewrite": retry_kind == "rewrite",
                                    "pendingCommitId": "synthetic-pending" if retry_kind == "save" else None})
                    page.evaluate("patch=>window.__speakmodeQA.publishSnapshot(patch)", {"session": session})
                    expect(retained_card(page).get_by_role("button", name="打开恢复", exact=True)).to_be_enabled()
                    expect(retained_card(page).get_by_role("button", name="放弃这条恢复", exact=True)).to_be_enabled()
                    checks.append(retry_kind)
                assert actions(page) == []
                return {"workerBlockedThenFailuresCanSwap": checks, "currentWorkDisplaced": False}
            run_case("04b-current-worker-and-unsaved-work-protected", unresolved_current, fixture(released=False))

            def empty_preflight_failure(page):
                expect(current_card(page)).to_contain_text("合成云端凭据暂不可读")
                expect(retained_card(page).get_by_role("button", name="打开恢复", exact=True)).to_be_enabled()
                retained_card(page).get_by_role("button", name="打开恢复", exact=True).click()
                expect(retained_card(page)).to_have_count(0)
                expect(current_card(page).get_by_role("button", name="重试识别", exact=True)).to_be_enabled()
                calls = actions(page)
                assert [row["name"] for row in calls] == ["recovery.activate"], calls
                assert calls[0]["payload"]["sessionId"] == RETAINED_ID
                return {"emptyCurrentFailureDidNotBlockRecovery": True, "automaticCloud": False}
            empty_failure = fixture(current_state="failed")
            empty_failure["snapshot"]["session"].update({"partial": "", "captions": [], "canRetryAsr": False,
                "canCopyRecovery": False, "retryKind": None, "sourceRevisionId": None, "pendingCommitId": None,
                "error": "合成云端凭据暂不可读", "errorCode": "key_required"})
            run_case("04d-empty-preflight-failure-allows-recovery", empty_preflight_failure, empty_failure)

            def cancel_discard(page):
                retained_card(page).get_by_role("button", name="放弃这条恢复", exact=True).click()
                expect(retained_card(page).get_by_role("button", name="确认放弃", exact=True)).to_be_visible()
                retained_card(page).get_by_role("button", name="保留恢复", exact=True).click()
                expect(retained_card(page).get_by_role("button", name="确认放弃", exact=True)).to_have_count(0)
                expect(retained_card(page).get_by_role("button", name="打开恢复", exact=True)).to_be_enabled()
                expect(retained_card(page).get_by_role("button", name="放弃这条恢复", exact=True)).to_be_enabled()
                assert actions(page) == []
                return {"discardCancelled": True, "commandsIssued": []}
            run_case("04c-cancel-discard-is-nondestructive", cancel_discard)

            def expires(page):
                expect(retained_card(page)).to_be_visible()
                before = len(ledger(page))
                page.evaluate("window.__speakmodeQA.expire()")
                expect(retained_card(page)).to_have_count(0)
                assert actions(page) == []
                assert any(call["name"] == "state.snapshot" for call in ledger(page)[before:])
                return {"expiredEventClearedCard": True, "eventTransport": "actual subscribed Channel callback"}
            run_case("05-expiry-event-resync", expires)

            def stale(page):
                retained_card(page).get_by_role("button", name="打开恢复", exact=True).click()
                expect(retained_card(page)).to_be_visible()
                expect(retained_card(page)).to_contain_text("这条恢复已变化，请刷新后重试。")
                expect(retained_card(page).get_by_role("button", name="打开恢复", exact=True)).to_be_enabled()
                assert [row["name"] for row in actions(page)] == ["recovery.activate"]
                return {"staleErrorVisible": True, "cardRetained": True, "implicitRetry": False}
            run_case("06-stale-activation-preserves-card", stale, fixture(activate_error=True))

            def revived_session(page):
                # First observe A, then B. A is now retired in the frontend.
                expect(current_card(page)).to_contain_text("合成识别未完成")
                page.evaluate("patch=>window.__speakmodeQA.publishSnapshot(patch)",
                              {"session": live(CURRENT_ID, 42, "ready"), "recovery": fixture()["snapshot"]["recovery"]})
                expect(retained_card(page)).to_be_visible()
                retained_card(page).get_by_role("button", name="打开恢复", exact=True).click()
                expect(retained_card(page)).to_have_count(0)
                expect(current_card(page).get_by_role("button", name="重试识别", exact=True)).to_be_enabled()
                # Later publication sequence is not a newer session generation.
                for late in [live(RETAINED_ID, 41, "recording", released=False), live(CURRENT_ID, 42, "recording", released=False)]:
                    page.evaluate("session=>window.__speakmodeQA.publishSession(session)", late)
                expect(current_card(page).get_by_role("button", name="重试识别", exact=True)).to_be_enabled()
                expect(current_card(page).get_by_role("button", name="结束口述", exact=True)).to_have_count(0)
                current_card(page).get_by_role("button", name="重试识别", exact=True).click()
                expect(current_card(page)).to_contain_text("正在转成文字")
                request = actions(page)[-1]
                assert request["name"] == "asr.retry" and request["payload"]["sessionId"] == RETAINED_ID and request["payload"]["expectedGeneration"] == 43, request
                return {"resurrectedGeneration": 43, "lateGenerationsRejected": [41, 42], "explicitRetryRequest": request}
            old = fixture(retained=False);old["snapshot"]["session"] = live(RETAINED_ID, 41)
            run_case("07-authoritative-reactivation-and-late-events", revived_session, old)

            def swap_recovery(page):
                retained_card(page).get_by_role("button", name="放弃这条恢复", exact=True).click()
                expect(retained_card(page).get_by_role("button", name="确认放弃", exact=True)).to_be_visible()
                retained_card(page).get_by_role("button", name="打开恢复", exact=True).click()
                expect(retained_card(page)).to_be_visible()
                expect(retained_card(page).get_by_role("button", name="确认放弃", exact=True)).to_have_count(0)
                expect(retained_card(page).get_by_role("button", name="放弃这条恢复", exact=True)).to_be_enabled()
                expect(current_card(page).get_by_role("button", name="重试识别", exact=True)).to_be_enabled()
                # The replacement card addresses B@42, never the old A@41.
                retained_card(page).get_by_role("button", name="打开恢复", exact=True).click()
                expect(current_card(page).get_by_role("button", name="重试识别", exact=True)).to_be_enabled()
                current_card(page).get_by_role("button", name="重试识别", exact=True).click()
                expect(current_card(page)).to_contain_text("正在转成文字")
                calls = actions(page)
                assert [row["name"] for row in calls] == ["recovery.activate", "recovery.activate", "asr.retry"], calls
                assert [(row["payload"]["sessionId"], row["payload"]["expectedGeneration"]) for row in calls] == [
                    (RETAINED_ID, 41), (CURRENT_ID, 42), (CURRENT_ID, 44)], calls
                return {"roundTripIdentities": [[RETAINED_ID, 41], [CURRENT_ID, 42], [CURRENT_ID, 44]],
                        "oldDiscardIntentClearedOnReplacement": True, "nativeSwapAcceptance": False}
            run_case("07b-swap-keeps-current-and-clears-old-confirmation", swap_recovery, fixture(current_state="failed"))

            def pending_delivery(page):
                expect(current_card(page)).to_have_count(0)
                expect(current_card(page).get_by_role("button", name="查看成稿", exact=True)).to_have_count(0)
                expect(current_card(page).get_by_role("button", name="复制成稿", exact=True)).to_have_count(0)
                assert actions(page) == []
                return {"competingNavigationAndCopy": False}
            for state in ["prepared", "dispatched"]:
                data = fixture(retained=False);data["snapshot"]["session"]["deliveryState"] = state
                run_case("08-delivery-" + state, pending_delivery, data)

            def responsive(page):
                expect(retained_card(page)).to_be_visible()
                sizes = []
                for width in [430, 800]:
                    page.set_viewport_size({"width": width, "height": 900})
                    expect(retained_card(page)).to_be_visible()
                    retained_card(page).scroll_into_view_if_needed()
                    geometry = page.evaluate("({width:innerWidth,scrollWidth:document.documentElement.scrollWidth,bodyWidth:document.body.scrollWidth})")
                    assert geometry["scrollWidth"] <= width + 1 and geometry["bodyWidth"] <= width + 1, geometry
                    screenshot(page, args.out / f"10-responsive-{width}.png")
                    sizes.append(geometry)
                return {"viewports": sizes, "nativeWindowAcceptance": False}
            run_case("09-responsive-retained-card", responsive)
            def start_next(page):
                expect(current_card(page).get_by_role("button", name="开始下一条口述", exact=True)).to_be_enabled()
                current_card(page).get_by_role("button", name="开始下一条口述", exact=True).click()
                expect(retained_card(page)).to_have_count(2)
                expect(current_card(page)).to_contain_text("正在聆听")
                for card in retained_card(page).all():
                    expect(card.get_by_role("button", name="打开恢复", exact=True)).to_be_disabled()
                assert [row["name"] for row in actions(page)] == ["session.start"]
                return {"bothFailuresRetained": True, "newRecordingStarted": True, "cloudRetry": False}
            run_case("10-start-next-keeps-multiple-failures", start_next, fixture(current_state="incomplete"))

            def source_reactivation(page):
                source=live(RETAINED_ID,41)
                source.update({"canRetryAsr":False,"canRetryRewrite":True,"canUseSource":True,"sourceRevisionId":"source-immutable","retryKind":"rewrite"})
                page.evaluate("patch=>window.__speakmodeQA.publishSnapshot(patch)",{"session":source,"recovery":None})
                page.evaluate("patch=>window.__speakmodeQA.publishSnapshot(patch)",{"session":live(CURRENT_ID,42,"ready"),"recoveries":[{"session":source,"expiresAt":fixture()["snapshot"]["recovery"]["expiresAt"]}]})
                retained_card(page).get_by_role("button", name="打开恢复", exact=True).click()
                expect(retained_card(page)).to_have_count(0)
                expect(current_card(page).get_by_role("button",name="重试整理",exact=True)).to_be_enabled()
                assert page.evaluate("window.__speakmodeQA.data.snapshot.session.generation")==41
                assert [row["name"] for row in actions(page)]==["recovery.activate"]
                return {"immutableSourceGenerationRestored":True,"noAutomaticRewrite":True}
            run_case("11-source-restores-with-immutable-generation", source_reactivation)

            if args.cases and set(args.cases) - {row["name"] for row in rows}:
                raise ValueError("Unknown case selection: " + ", ".join(sorted(set(args.cases) - {row["name"] for row in rows})))
            receipt["status"] = "INSPECTION_ONLY" if args.inspect_only else "PASS" if all(row["status"] == "PASS" for row in rows) else "FAIL"
        except Exception as error:
            receipt.update({"status": "HARNESS_ERROR", "error": str(error), "traceback": traceback.format_exc()})
        finally:
            browser.close()
    receipt.update({"pageErrors": errors, "blockedExternalRequests": blocked, "completedAt": datetime.now(timezone.utc).isoformat(),
                    "counts": {"registered": len(rows), "passed": sum(row["status"] == "PASS" for row in rows), "failed": sum(row["status"] == "FAIL" for row in rows)}})
    if errors or blocked:
        receipt["status"] = "FAIL"
    write_new(args.out / "receipt.json", receipt, json_value=True)
    print(json.dumps({key: receipt.get(key) for key in ["status", "out", "counts", "pageErrors", "blockedExternalRequests", "error"]}, ensure_ascii=False, indent=2))
    return 0 if receipt["status"] in ["PASS", "INSPECTION_ONLY"] else 1


if __name__ == "__main__":
    sys.exit(main())
