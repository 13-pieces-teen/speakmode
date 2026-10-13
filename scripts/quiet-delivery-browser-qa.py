"""Production React checks for quiet delivery completion with synthetic Tauri IPC.

No app, user profile, microphone, provider, host target or clipboard is accessed.
Use a new output directory and a separately owned localhost production preview.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import sys
import traceback
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("restore_delivery_qa", ROOT / "scripts/restore-delivery-browser-qa.py")
_shared = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_shared)

# Reuse the established synthetic history and IPC fixture. Add current-date
# usage reads so the home screen has no unrelated fixture-generated errors.
BRIDGE = _shared.BRIDGE.replace(
    "if(name==='audio.listDevices')return [];",
    """if(name==='audio.listDevices')return [];
    if(name==='usage.today'||name==='usage.total') {
     const today=new Date(),date=`${today.getFullYear()}-${String(today.getMonth()+1).padStart(2,'0')}-${String(today.getDate()).padStart(2,'0')}`;
     return {...copy(fixture.snapshot.usage),date};
    }
    if(name==='recoverable.copy')return null;
    """,
)
READS = {"state.snapshot", "audio.listDevices", "history.list", "history.get", "usage.today", "usage.total"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--serve", action="store_true")
    args = parser.parse_args()
    parsed = urlparse(args.url)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or parsed.username or parsed.password:
        parser.error("--url must be an independent http://127.0.0.1 URL")
    if not args.out.is_absolute():
        parser.error("--out must be absolute")
    args.out.mkdir(parents=True, exist_ok=False)
    tests, errors, blocked = [], [], []
    receipt = {"status": "RUNNING", "startedAt": datetime.now(timezone.utc).isoformat(),
               "scope": "Production React in headless Edge with synthetic IPC. No native paste, audio, credential or user-data acceptance.",
               "tests": tests, "pageErrors": errors, "blockedRequests": blocked}
    source_paths = ["app/src/components/SessionRecovery.tsx", "app/src/services/deliveryFeedback.ts"]
    receipt["sourceSha256"] = {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in source_paths}
    receipt["distSha256"] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                             for path in sorted((ROOT / "app/dist").rglob("*")) if path.is_file()}
    receipt["scriptSha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()

    with _shared.managed_preview(args.url, args.out, args.serve) as server, sync_playwright() as playwright:
        receipt["server"] = server
        browser = playwright.chromium.launch(channel="msedge", headless=True)
        receipt["browserVersion"] = browser.version
        try:
            def run_case(name, data, action, width=1440):
                row = {"name": name, "status": "RUNNING"}
                tests.append(row)
                context = browser.new_context(viewport={"width": width, "height": 1000}, locale="zh-CN")
                context.add_init_script(BRIDGE.replace("__FIXTURE__", json.dumps(data, ensure_ascii=False)))
                def route(request):
                    host = urlparse(request.request.url).hostname
                    if host not in ("127.0.0.1", None):
                        blocked.append(request.request.url)
                        request.abort()
                    else:
                        request.continue_()
                context.route("**/*", route)
                page = context.new_page()
                page.on("pageerror", lambda error: errors.append({"case": name, "error": str(error)}))
                try:
                    page.goto(args.url, wait_until="networkidle")
                    expect(page.locator(".latest-card")).to_be_visible()
                    row["initialScreenshot"] = _shared.screenshot(page, args.out / f"{name}-initial.png")
                    # Preserve observed production controls before interaction.
                    _shared.write_new(args.out / f"{name}-controls.json", page.locator("button,[role=region]").evaluate_all(
                        "nodes=>nodes.map(el=>({tag:el.tagName,name:el.getAttribute('aria-label'),text:el.textContent}))"), json_value=True)
                    row["observations"] = action(page)
                    row["status"] = "PASS"
                except Exception as error:
                    row.update(status="FAIL", error=str(error), traceback=traceback.format_exc())
                finally:
                    row["commands"] = page.evaluate("window.__speakmodeQA?.ledger??[]")
                    row["screenshot"] = _shared.screenshot(page, args.out / f"{name}-{row['status'].lower()}.png")
                    _shared.write_new(args.out / f"{name}.html", page.content())
                    context.close()

            def actions(page):
                return [call for call in page.evaluate("window.__speakmodeQA.ledger") if call["name"] not in READS]

            def quiet(page):
                expect(page.get_by_role("region", name="当前口述状态", exact=True)).to_have_count(0)
                expect(page.get_by_text("输入已派发", exact=True)).to_have_count(0)
                expect(page.get_by_role("button", name="查看成稿", exact=True)).to_have_count(0)
                expect(page.get_by_role("button", name="查看与调整", exact=True)).to_be_visible()
                assert actions(page) == [], actions(page)
                page.get_by_role("button", name="查看与调整", exact=True).click()
                expect(page.locator(".history-detail")).to_be_visible()
                expect(page.get_by_role("region", name="当前口述状态", exact=True)).to_have_count(0)
                assert actions(page) == [], actions(page)
                return {"statusCardVisible": False, "historyStillAccessible": True, "automaticActions": []}

            for state in ("prepared", "dispatched", "unconfirmed", "confirmed"):
                run_case(f"quiet-{state}", _shared.fixture(state, "inputDispatchedUnconfirmed" if state == "unconfirmed" else None), quiet)
            fallback = _shared.fixture("unconfirmed", "inputDispatchedUnconfirmed")
            fallback["snapshot"]["session"]["rewriteFallback"] = "timeout"
            run_case("quiet-source-fallback-narrow", fallback, quiet, width=900)

            def failure(page):
                card = page.get_by_role("region", name="当前口述状态", exact=True)
                expect(card).to_be_visible()
                expect(card.get_by_role("button", name="复制成稿", exact=True)).to_be_enabled()
                expect(card.get_by_role("button", name="查看成稿", exact=True)).to_be_enabled()
                assert actions(page) == [], actions(page)
                card.get_by_role("button", name="复制成稿", exact=True).click()
                expect(card.get_by_text("成稿已复制，可切回输入位置粘贴。", exact=True)).to_be_visible()
                assert [call["name"] for call in actions(page)] == ["result.copy"], actions(page)
                card.get_by_role("button", name="查看成稿", exact=True).click()
                expect(page.locator(".history-detail")).to_be_visible()
                assert [call["name"] for call in actions(page)] == ["result.copy"], actions(page)
                return {"explicitCopyOnly": True, "historyStillAccessible": True, "automaticRetry": False}

            for state, reason in (("skipped", "targetUnavailableAtStop"), ("failed", "nativeDispatchFailed"), ("unconfirmed", "partialInputUnconfirmed")):
                run_case(f"failure-{state}", _shared.fixture(state, reason), failure)

            error_data = _shared.fixture("unconfirmed", "inputDispatchedUnconfirmed")
            message = "本机输入回执未保存；不会再次自动输入，请核对目标后手动复制"
            error_data["snapshot"]["session"]["error"] = message
            def storage_error(page):
                expect(page.get_by_role("region", name="当前口述状态", exact=True).get_by_text(message, exact=True)).to_be_visible()
                assert actions(page) == [], actions(page)
                return {"realErrorRetained": True, "automaticActions": []}
            run_case("saved-result-error-visible", error_data, storage_error)

            recording = _shared.fixture("unconfirmed")
            recording["snapshot"]["session"].update(state="recording", workerReleased=False)
            def recording_controls(page):
                card = page.get_by_role("region", name="当前口述状态", exact=True)
                expect(card.get_by_role("button", name="结束口述", exact=True)).to_be_enabled()
                expect(card.get_by_role("button", name="取消本次口述", exact=True)).to_be_enabled()
                assert actions(page) == [], actions(page)
                return {"ongoingControlsRetained": True}
            run_case("recording-controls-retained", recording, recording_controls)
        finally:
            browser.close()
    receipt["status"] = "PASS" if tests and all(row["status"] == "PASS" for row in tests) and not errors and not blocked else "FAIL"
    receipt["endedAt"] = datetime.now(timezone.utc).isoformat()
    _shared.write_new(args.out / "receipt.json", receipt, json_value=True)
    print(json.dumps({"status": receipt["status"], "tests": len(tests), "receipt": str(args.out / "receipt.json")}, ensure_ascii=False))
    return 0 if receipt["status"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
