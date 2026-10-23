"""Audit real browser font rendering and usage display with isolated synthetic IPC.

Local production preview only. Does not read the native profile, credentials,
microphone, clipboard or host inputs. CDP font evidence identifies the actual
font per rendered glyph; computed font-family alone is insufficient.
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
from playwright.sync_api import sync_playwright, expect

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("ui_qa", ROOT / "scripts/ui-experience-browser-qa.py")
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)
base = ui.base
TITLE = "SpeakMode 语音整理测试：收藏、筛选与撤销，预算暂未确认。"
USAGE_BRIDGE = r"""
(() => {
 const qa=window.__speakmodeQA,original=window.__TAURI_INTERNALS__.invoke;
 qa.usageReads=[];qa.pendingUsage=[];qa.usageMode=qa.fixture.usageMode||'ready';
 qa.totalUsage=qa.fixture.totalUsage||{count:97,sourceCharCount:56432,captureDurationMs:5824000};
 window.__TAURI_INTERNALS__.invoke=async(command,args={})=>{
  const name=args.request?.name;
  if(command!=='app_command'||!['usage.today','usage.total'].includes(name))return original(command,args);
  const value=structuredClone(name==='usage.today'?qa.fixture.snapshot.usage:qa.totalUsage);
  qa.usageReads.push({name,value,at:new Date().toISOString()});
  qa.ledger.push({name,payload:structuredClone(args.request.payload)});
  if(qa.usageMode==='fail')throw new Error('Synthetic usage read failure');
  if(qa.usageMode==='hold')return new Promise((resolve,reject)=>qa.pendingUsage.push({name,value,resolve,reject}));
  return value;
 };
})();
"""


def fixture():
    data = ui.fixture()
    for detail in data["details"][:3]:
        detail["_title"] = TITLE
        detail["output"]["blocks"] = [{"id": detail["output"]["id"] + "-block", "text": TITLE, "sourceRefs": [detail["source"]["items"][0]["id"]]}]
    return data


def actual_fonts(page, selectors):
    page.evaluate("document.fonts.ready")
    cdp = page.context.new_cdp_session(page)
    cdp.send("DOM.enable")
    cdp.send("CSS.enable")
    root = cdp.send("DOM.getDocument", {"depth": -1})["root"]["nodeId"]
    results = []
    for selector in selectors:
        node = cdp.send("DOM.querySelector", {"nodeId": root, "selector": selector})["nodeId"]
        if not node:
            raise AssertionError("Missing font audit node: " + selector)
        result = {"selector": selector, "text": page.locator(selector).first.inner_text(), "fonts": cdp.send("CSS.getPlatformFontsForNode", {"nodeId": node})["fonts"]}
        result["computed"] = page.locator(selector).first.evaluate("e=>{const s=getComputedStyle(e);return {family:s.fontFamily,size:s.fontSize,weight:s.fontWeight,lineHeight:s.lineHeight}}")
        results.append(result)
    # Preserve the production screenshot before this observational DOM probe.
    # Inline wrappers inherit every relevant metric to identify each actual glyph.
    for index, selector in enumerate(selectors):
        characters = page.locator(selector).first.evaluate("""(e,prefix)=>{const text=e.textContent;e.replaceChildren(...Array.from(text).map((char,i)=>{const span=document.createElement('span');span.textContent=char;span.id=prefix+i;span.style.cssText='display:inline;font:inherit;line-height:inherit;color:inherit;letter-spacing:inherit';return span}));return Array.from(text)}""", f"font-audit-{index}-")
        glyphs = []
        for i, char in enumerate(characters):
            if '\u3400' <= char <= '\u9fff':
                node = cdp.send("DOM.querySelector", {"nodeId": root, "selector": f"#font-audit-{index}-{i}"})["nodeId"]
                glyphs.append({"character": char, "fonts": cdp.send("CSS.getPlatformFontsForNode", {"nodeId": node})["fonts"]})
        results[index]["chineseGlyphFonts"] = glyphs
    cdp.detach()
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--serve", action="store_true")
    parser.add_argument("--baseline", action="store_true", help="Capture actual old fonts without asserting the proposed fix")
    parser.add_argument("--case", help="Only run a named case")
    args = parser.parse_args()
    if urlparse(args.url).scheme != "http" or urlparse(args.url).hostname != "127.0.0.1" or not args.out.is_absolute():
        parser.error("Requires local http://127.0.0.1 URL and absolute fresh --out")
    args.out.mkdir(parents=True, exist_ok=False)
    rows, errors, external = [], [], []
    receipt = {"status": "RUNNING", "scope": "Synthetic bridge browser UI only; actual per-glyph platform font evidence. Native database persistence is validated separately.", "startedAt": datetime.now(timezone.utc).isoformat(), "baseline": args.baseline, "tests": rows, "pageErrors": errors, "blockedExternalRequests": external}
    receipt["distSha256"] = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((ROOT / "app/dist").rglob("*")) if p.is_file()}
    with base.managed_preview(args.url, args.out, args.serve) as server, sync_playwright() as p:
        receipt["server"] = server
        browser = p.chromium.launch(channel="msedge", headless=True)
        receipt["browserVersion"] = browser.version
        try:
            def case(name, action, width=1440, height=900, data=None, instant="2026-10-09T06:00:00+00:00"):
                if args.case and args.case != name:
                    return
                row = {"name": name, "status": "RUNNING"}; rows.append(row)
                context = browser.new_context(viewport={"width": width, "height": height}, locale="zh-CN", timezone_id="Asia/Shanghai", reduced_motion="reduce")
                context.add_init_script(ui.BRIDGE.replace("__FIXTURE__", json.dumps(data or fixture(), ensure_ascii=False)))
                context.add_init_script(USAGE_BRIDGE)
                def route(r):
                    if urlparse(r.request.url).hostname not in ("127.0.0.1", None):
                        external.append(r.request.url); r.abort()
                    else:
                        r.continue_()
                context.route("**/*", route)
                page = context.new_page(); page.on("pageerror", lambda e: errors.append({"case": name, "error": str(e)}))
                try:
                    page.clock.install(time=datetime.fromisoformat(instant))
                    page.goto(args.url, wait_until="networkidle")
                    expect(page.get_by_role("heading", name="最近一次口述", exact=True)).to_be_visible()
                    row["observations"] = action(page)
                    row["layout"] = ui.capture(page, args.out, name + "-final")
                    assert not row["layout"]["overflow"], row["layout"]["overflow"]
                    row["ledger"] = page.evaluate("window.__speakmodeQA.ledger")
                    row["status"] = "PASS"
                except Exception as e:
                    row.update(status="FAIL", error=str(e), traceback=traceback.format_exc())
                    ui.capture(page, args.out, name + "-failure")
                finally:
                    context.close()

            def typography(page):
                page.screenshot(path=str(args.out / "typography-original.png"), full_page=True)
                results = actual_fonts(page, [".home-hero h1", ".latest-title-row h3", ".recent-copy strong"])
                base.write_new(args.out / "actual-fonts.json", results, json_value=True)
                if not args.baseline:
                    for item in results:
                        assert item["chineseGlyphFonts"], item
                        for glyph in item["chineseGlyphFonts"]:
                            assert len(glyph["fonts"]) == 1 and "YaHei" in glyph["fonts"][0]["familyName"], glyph
                            assert not glyph["fonts"][0]["isCustomFont"], glyph
                    assert results[1]["computed"]["weight"] == results[2]["computed"]["weight"], results
                return {"actualFonts": results}
            case("actual-chinese-title-fonts", typography)
            if not args.baseline:
                def usage_metrics(page):
                    return page.locator(".usage-metric dd").all_inner_texts()

                def await_today(page, count="28", characters="12,864"):
                    section=page.get_by_role("region",name="今日概览",exact=True)
                    expect(section).to_have_attribute("aria-busy","false")
                    expect(section.locator(".usage-metric").nth(0)).to_contain_text(count)
                    expect(section.locator(".usage-metric").nth(1)).to_contain_text(characters)
                    return section

                def viewport(page):
                    section=await_today(page)
                    expect(section).to_contain_text("2026 年 10 月 09 日")
                    expect(section).to_contain_text("口述字数")
                    expect(section.get_by_role("button",name="今日",exact=True)).to_have_attribute("aria-pressed","true")
                    section.get_by_role("button",name="累计",exact=True).click()
                    total=page.get_by_role("region",name="累计概览",exact=True)
                    expect(total).to_contain_text("56,432")
                    expect(total.get_by_role("button",name="累计",exact=True)).to_have_attribute("aria-pressed","true")
                    return {"metrics":usage_metrics(page),"totalDateLabel":total.locator(".usage-period p").inner_text()}
                for width,height in [(1440,900),(1024,768),(720,640),(430,720)]:
                    case(f"usage-layout-{width}x{height}",viewport,width,height)

                def reload_read(page):
                    await_today(page)
                    before=usage_metrics(page)
                    page.reload(wait_until="networkidle")
                    await_today(page)
                    after=usage_metrics(page)
                    assert before==after,(before,after)
                    reads=page.evaluate("window.__speakmodeQA.usageReads")
                    assert any(r["name"]=="usage.today" for r in reads),reads
                    return {"before":before,"after":after,"reloadReads":reads,"boundary":"Reload reconstructs UI from synthetic persisted counts; SQLite durability tested separately."}
                case("usage-reload-rehydrates",reload_read)

                empty_today=fixture()
                empty_today["snapshot"]["usage"]={"date":"2026-10-09","count":0,"sourceCharCount":0,"captureDurationMs":0}
                empty_today["totalUsage"]={"count":7,"sourceCharCount":1427,"captureDurationMs":212000}
                def zero_day(page):
                    section=await_today(page,"0","0")
                    expect(section.locator(".usage-read-status")).to_have_text("今日暂无口述")
                    assert page.locator(".recent-item").count()==2
                    section.get_by_role("button",name="累计",exact=True).click()
                    section=page.get_by_role("region",name="累计概览",exact=True)
                    expect(section.locator(".usage-metric").nth(0)).to_contain_text("7")
                    expect(section.locator(".usage-metric").nth(1)).to_contain_text("1,427")
                    return {"todayWasZero":True,"historyRemainsVisible":True,"total":usage_metrics(page)}
                case("empty-today-retains-total",zero_day,data=empty_today)

                held=fixture();held["usageMode"]="hold"
                def pending(page):
                    section=page.get_by_role("region",name="今日概览",exact=True)
                    expect(section).to_have_attribute("aria-busy","true")
                    expect(section.locator(".usage-read-status")).to_have_text("正在读取…")
                    assert all("—" in v and "0" not in v for v in usage_metrics(page)),usage_metrics(page)
                    page.screenshot(path=str(args.out/"usage-initial-loading.png"))
                    page.evaluate("window.__speakmodeQA.usageMode='ready';window.__speakmodeQA.pendingUsage.splice(0).forEach(p=>p.resolve(p.value))")
                    await_today(page)
                    return {"pendingWasPlaceholder":True,"resolved":usage_metrics(page)}
                case("usage-loading-not-zero",pending,data=held)

                failed=fixture();failed["usageMode"]="fail"
                def failed_retry(page):
                    section=page.get_by_role("region",name="今日概览",exact=True)
                    expect(section.locator(".usage-read-status")).to_contain_text("读取失败")
                    assert all("—" in v for v in usage_metrics(page)),usage_metrics(page)
                    page.screenshot(path=str(args.out/"usage-initial-failure.png"))
                    page.evaluate("window.__speakmodeQA.usageMode='ready'")
                    section.get_by_role("button",name="重新读取",exact=True).click()
                    await_today(page)
                    return {"failureDidNotShowZero":True,"retry":usage_metrics(page)}
                case("usage-failure-retry",failed_retry,data=failed)

                def stale_failure(page):
                    section=await_today(page)
                    before=usage_metrics(page)
                    page.evaluate("window.__speakmodeQA.usageMode='fail';window.dispatchEvent(new Event('focus'))")
                    expect(section.locator(".usage-read-status")).to_contain_text("更新失败 · 旧值")
                    assert usage_metrics(page)==before
                    page.evaluate("window.__speakmodeQA.usageMode='ready'")
                    section.get_by_role("button",name="重新读取",exact=True).click()
                    await_today(page)
                    expect(section).not_to_contain_text("更新失败")
                    return {"previousValuesRetained":before}
                case("usage-stale-refresh-retains-values",stale_failure)

                def range_race(page):
                    await_today(page)
                    page.evaluate("window.__speakmodeQA.usageMode='hold'")
                    page.get_by_role("button",name="累计",exact=True).click()
                    page.wait_for_function("window.__speakmodeQA.pendingUsage.some(p=>p.name==='usage.total')")
                    page.get_by_role("button",name="今日",exact=True).click()
                    page.wait_for_function("window.__speakmodeQA.pendingUsage.some(p=>p.name==='usage.today')")
                    page.evaluate("const q=window.__speakmodeQA.pendingUsage;const i=q.findIndex(p=>p.name==='usage.today');const p=q.splice(i,1)[0];p.resolve(p.value)")
                    await_today(page)
                    page.evaluate("window.__speakmodeQA.pendingUsage.splice(0).forEach(p=>p.resolve(p.value))")
                    await_today(page)
                    expect(page.get_by_role("region",name="累计概览",exact=True)).to_have_count(0)
                    return {"lateTotalDidNotOverwriteToday":True,"metrics":usage_metrics(page)}
                case("usage-range-out-of-order",range_race)

                def midnight(page):
                    await_today(page)
                    before=page.evaluate("window.__speakmodeQA.usageReads.length")
                    page.evaluate("window.__speakmodeQA.fixture.snapshot.usage={date:'2026-10-10',count:0,sourceCharCount:0,captureDurationMs:0}")
                    page.clock.run_for(31000)
                    section=await_today(page,"0","0")
                    expect(section).to_contain_text("2026 年 10 月 10 日")
                    after=page.evaluate("window.__speakmodeQA.usageReads.length")
                    assert after>before,(before,after)
                    page.clock.run_for(60000)
                    assert page.evaluate("window.__speakmodeQA.usageReads.length")==after,"Unexpected same-day polling"
                    return {"readsBeforeMidnight":before,"readsAfterMidnight":after,"sameDayNoPolling":True,"metrics":usage_metrics(page)}
                case("usage-midnight-timer",midnight,instant="2026-10-09T15:59:50+00:00")

                def focus_refresh(page):
                    await_today(page)
                    page.evaluate("window.__speakmodeQA.fixture.snapshot.usage={date:'2026-10-10',count:3,sourceCharCount:234,captureDurationMs:31000}")
                    page.clock.set_system_time(datetime.fromisoformat("2026-10-10T06:00:00+00:00"))
                    page.evaluate("window.dispatchEvent(new Event('focus'))")
                    section=await_today(page,"3","234")
                    expect(section).to_contain_text("2026 年 10 月 10 日")
                    page.evaluate("window.__speakmodeQA.fixture.snapshot.usage.count=4;document.dispatchEvent(new Event('visibilitychange'))")
                    await_today(page,"4","234")
                    return {"focusAndVisibleReload":True,"metrics":usage_metrics(page)}
                case("usage-focus-after-day-change",focus_refresh)

                def midnight_new_response(page):
                    section=page.get_by_role("region",name="今日概览",exact=True)
                    expect(section).to_have_attribute("aria-busy","true")
                    page.clock.set_system_time(datetime.fromisoformat("2026-10-10T00:00:00+00:00"))
                    page.evaluate("const qa=window.__speakmodeQA;qa.fixture.snapshot.usage={date:'2026-10-10',count:2,sourceCharCount:108,captureDurationMs:45000};qa.pendingUsage.splice(0).forEach(p=>p.resolve(structuredClone(qa.fixture.snapshot.usage)))")
                    section=await_today(page,"2","108")
                    expect(section).to_contain_text("2026 年 10 月 10 日")
                    expect(section).not_to_contain_text("2026 年 10 月 09 日")
                    return {"newDayResponseUsesResponseDate":True,"metrics":usage_metrics(page)}
                case("usage-midnight-response-new-day",midnight_new_response,data=held,instant="2026-10-09T15:59:50+00:00")

                def midnight_old_response(page):
                    section=page.get_by_role("region",name="今日概览",exact=True)
                    expect(section).to_have_attribute("aria-busy","true")
                    page.clock.set_system_time(datetime.fromisoformat("2026-10-10T00:00:00+00:00"))
                    page.evaluate("window.__speakmodeQA.pendingUsage.splice(0).forEach(p=>p.resolve(p.value))")
                    expect(section.locator(".usage-read-status")).to_contain_text("读取失败")
                    expect(section).to_contain_text("2026 年 10 月 10 日")
                    assert all("—" in v for v in usage_metrics(page)),usage_metrics(page)
                    page.evaluate("const qa=window.__speakmodeQA;qa.usageMode='ready';qa.fixture.snapshot.usage={date:'2026-10-10',count:0,sourceCharCount:0,captureDurationMs:0}")
                    section.get_by_role("button",name="重新读取",exact=True).click()
                    await_today(page,"0","0")
                    return {"oldDayResponseRejected":True,"retry":usage_metrics(page)}
                case("usage-midnight-response-old-day",midnight_old_response,data=held,instant="2026-10-09T15:59:50+00:00")
        finally:
            browser.close()
    receipt["counts"] = {"registered": len(rows), "passed": sum(r["status"] == "PASS" for r in rows), "failed": sum(r["status"] == "FAIL" for r in rows)}
    receipt["status"] = "PASS" if rows and not errors and not external and receipt["counts"]["failed"] == 0 else "FAIL"
    receipt["completedAt"] = datetime.now(timezone.utc).isoformat()
    base.write_new(args.out / "receipt.json", receipt, json_value=True)
    print(json.dumps({k: receipt[k] for k in ["status", "counts", "pageErrors", "blockedExternalRequests"]}, ensure_ascii=False, indent=2))
    return 0 if receipt["status"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
