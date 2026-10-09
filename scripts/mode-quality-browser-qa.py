"""Mode trial and candidate-review acceptance with production React and synthetic IPC.

No provider, native process, microphone, real credentials/history or clipboard is
used. Model outputs below are deterministic UI fixtures, not quality evidence.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import traceback
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("mode_ui_base", ROOT / "scripts/ui-experience-browser-qa.py")
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)


def fixture():
    data = ui.fixture()
    data["details"] = [data["details"][0]]
    detail = data["details"][0]
    sources = ["预算五千，不对，三千五百元。", "可能周五开会。", "刚才的预算只算设备，差旅另算。", "如果会议没确认，先不要买票。"]
    detail["source"]["items"] = [
        {"id": f"qa-original-item-{i}", "providerItemId": f"qa-provider-{i}", "captureOrder": i,
         "text": text, "final": True, "startMs": i * 1000, "endMs": (i + 1) * 1000}
        for i, text in enumerate(sources)
    ]
    detail["output"]["blocks"] = [
        {"id": "qa-baseline-1", "text": "预算是三千五百元。可能周五开会。", "sourceRefs": ["qa-original-item-0", "qa-original-item-1"]},
        {"id": "qa-baseline-2", "text": "预算只算设备，差旅另算。如果会议没确认，先不要买票。", "sourceRefs": ["qa-original-item-2", "qa-original-item-3"]},
    ]
    detail["_title"] = "合成 Mode 试用记录"
    data["detail"] = detail
    return data


# Wrap only the new endpoints; all ordinary navigation remains the existing
# synthetic bridge. Exactly one ledger entry is recorded per IPC invocation.
EXTRA_BRIDGE = r"""
(() => {
 const q=window.__speakmodeQA,copy=value=>structuredClone(value),previous=window.__TAURI_INTERNALS__.invoke;
 Object.assign(q,{pendingPreview:[],delayPreview:false,failNextPreview:false,resultLabel:'本轮',sameCandidate:false,longCandidate:false});
 function result(payload) {
  const d=payload.source?q.fixture.details.find(d=>d.session.id===payload.source.sessionId):null;
  if(payload.source&&(!d||d.source.id!==payload.source.sourceRevisionId))throw new Error('合成来源版本已变化');
  if(payload.source&&Object.hasOwn(payload,'sample'))throw new Error('History source must not be flattened');
  const items=d?d.source.items:payload.sample.split(/\r?\n/).filter(t=>t.trim()).map((text,i)=>({id:'synthetic-'+i,text}));
  const metadata={runId:payload.runId,sourceKind:d?'history':'synthetic',sourceItemCount:items.length,sourceRevisionId:d?d.source.id:payload.runId,promptVersion:'synthetic-ui-fixture-v1'};
  const output=mode=>({resolvedMode:mode,blocks:[{id:'trial-'+mode,text:q.resultLabel+' · '+(mode==='light'?'预算是三千五百元。可能周五开会，预算只算设备，差旅另算。如果会议没确认，先不要买票。':'预算：三千五百元，只算设备，差旅另算。\n会议：可能周五开会。如果会议没确认，先不要买票。'),sourceRefs:items.map(i=>i.id)}]});
  if(payload.compareBuiltins)return {...metadata,comparison:{light:output('light'),structure:output('structure')}};
  return {...metadata,...output(payload.draft.baseMode)};
 }
 q.resolvePreviews=()=>q.pendingPreview.splice(0).forEach(item=>item.resolve(copy(item.value)));
 window.__TAURI_INTERNALS__.invoke=async(command,args={})=>{
  if(command!=='app_command')return previous(command,args);
  const {name,payload}=args.request;
  if(!['mode.preview','mode.cancelPreview','history.rewrite','history.cancelCandidate','revision.edit'].includes(name))return previous(command,args);
  q.ledger.push({name,payload:copy(payload)});
  if(name==='mode.cancelPreview'||name==='history.cancelCandidate')return null;
  if(name==='mode.preview'){
   if(q.failNextPreview){q.failNextPreview=false;throw new Error('合成模型请求失败；请重试。');}
   const value=result(payload);
   if(q.delayPreview)return new Promise(resolve=>q.pendingPreview.push({resolve,value}));
   return copy(value);
  }
  const d=q.fixture.details.find(d=>d.session.id===payload.sessionId);
  if(!d)throw new Error('Unknown synthetic session');
  if(name==='history.rewrite'){
   const blocks=q.sameCandidate?copy(d.output.blocks):[{id:'qa-candidate-1',text:q.longCandidate?'长文本候选。'.repeat(1200):'设备预算：三千五百元；差旅另算。\n会议安排：可能周五开会。会议没确认前，先不要买票。',sourceRefs:d.source.items.map(i=>i.id)}];
   return {...copy(d.output),id:payload.runId,baseRevisionId:payload.baseRevisionId,creationReason:'history',blocks};
  }
  if(payload.expectedCurrentRevisionId!==d.output.id)throw new Error('合成旧版本冲突');
  d.output={...copy(d.output),id:'qa-manual-edit',parentRevisionId:d.output.id,blocks:copy(payload.blocks),kind:'edit'};
  d.session.currentOutputId=d.output.id;d.canUndo=true;
  return copy(d);
 };
})();
"""


def ledger(page, name=None):
    calls = page.evaluate("window.__speakmodeQA.ledger")
    return [call for call in calls if name is None or call["name"] == name]


def capture(page, out, tag):
    page.screenshot(path=str(out / f"{tag}.png"), full_page=False)
    ui.base.write_new(out / f"{tag}.html", page.content())
    values = ui.measurements(page)
    # Radix RadioCards creates invisible native radio inputs for form bubbling.
    # These absolute inputs inherit the card dimensions and translate left;
    # excluding their exact inaccessible/noninteractive/zero-opacity shape must
    # not exempt the visible radio cards or any other overflowing element.
    overflow = page.evaluate("""() => {
      const visible=[],ignored=[];
      for(const e of document.querySelectorAll('main *')){
        const b=e.getBoundingClientRect();
        if(!(b.width>0&&(b.right>innerWidth+2||b.left<-2)))continue;
        const s=getComputedStyle(e),entry={tag:e.tagName,class:e.className,text:e.textContent?.slice(0,80),left:b.left,right:b.right};
        const nativeRadio=e instanceof HTMLInputElement&&e.type==='radio'&&e.getAttribute('aria-hidden')==='true'&&e.tabIndex===-1&&s.position==='absolute'&&Number(s.opacity)===0&&s.pointerEvents==='none';
        if(nativeRadio)ignored.push({...entry,reason:'Radix native form radio: aria-hidden, tabindex -1, absolute, zero opacity, no pointer events'});
        else visible.push(entry);
      }
      return {visible,ignored};
    }""")
    values["rawOverflow"] = values["overflow"]
    values["overflow"] = overflow["visible"]
    values["ignoredNativeFormInputs"] = overflow["ignored"]
    values["controls"] = page.locator("button,input,textarea,[role=combobox]").evaluate_all("nodes=>nodes.map(e=>({tag:e.tagName,name:e.getAttribute('aria-label'),text:e.textContent,disabled:e.disabled}))")
    ui.base.write_new(out / f"{tag}.json", values, json_value=True)
    return values


def open_editor(page):
    page.get_by_role("button", name="管理 Mode", exact=True).click()
    page.get_by_role("button", name="新建 Mode", exact=True).click()
    expect(page.get_by_role("heading", name="新建 Mode", exact=True)).to_be_visible()


def compare(page):
    page.get_by_role("button", name="对比轻整理与结构", exact=True).click()
    expect(page.get_by_role("group", name="同一输入的内置模式对比", exact=True)).to_be_visible()


def generate_candidate(page):
    page.get_by_role("button", name="查看与调整", exact=True).click()
    page.get_by_role("button", name="换种整理", exact=True).click()
    page.get_by_role("button", name="生成候选", exact=True).click()
    expect(page.get_by_role("heading", name="新候选", exact=True)).to_be_visible()
    page.get_by_role("button", name="对比生成前文本", exact=True).click()
    region = page.get_by_role("region", name="候选改动对照", exact=True)
    expect(region).to_be_visible()
    return region


def acceptance(browser, args, rows, errors, external):
    def case(name, action, width=1440, height=900):
        if args.case and args.case != name:
            return
        row = {"name": name, "status": "RUNNING"}
        rows.append(row)
        context = browser.new_context(viewport={"width": width, "height": height}, locale="zh-CN", reduced_motion="reduce")
        context.add_init_script(ui.BRIDGE.replace("__FIXTURE__", json.dumps(fixture(), ensure_ascii=False)) + EXTRA_BRIDGE)

        def route(request):
            if urlparse(request.request.url).hostname not in ("127.0.0.1", None):
                external.append(request.request.url)
                request.abort()
            else:
                request.continue_()

        context.route("**/*", route)
        page = context.new_page()
        page.on("pageerror", lambda error: errors.append({"case": name, "error": str(error)}))
        try:
            page.goto(args.url, wait_until="networkidle")
            expect(page.get_by_role("heading", name="最近一次口述", exact=True)).to_be_visible()
            row["observations"] = action(page)
            row["final"] = capture(page, args.out, name)
            row["ledger"] = ledger(page)
            assert not row["final"]["overflow"], row["final"]["overflow"]
            assert not [call for call in row["ledger"] if call["name"] in ("mode.save", "mode.setDefault", "revision.adopt", "result.copy")]
            row["status"] = "PASS"
        except Exception as error:
            row.update(status="FAIL", error=str(error), traceback=traceback.format_exc())
            capture(page, args.out, name + "-failure")
        finally:
            context.close()

    def responsive(page):
        open_editor(page)
        expect(page.get_by_role("textbox", name="名称", exact=True)).to_have_value("")
        compare(page)
        calls = ledger(page, "mode.preview")
        assert len(calls) == 1 and calls[0]["payload"]["compareBuiltins"] is True
        assert "draft" not in calls[0]["payload"]
        expect(page.locator(".mode-tryout")).to_contain_text("两次云端请求")
        expect(page.get_by_role("group", name="同一输入的内置模式对比")).to_contain_text("可能周五")
        panes = page.locator(".mode-comparison-pane").evaluate_all("nodes=>nodes.map(e=>{const b=e.getBoundingClientRect();return {left:b.left,top:b.top,width:b.width}})")
        assert len(panes) == 2
        assert all(pane["width"] >= 160 for pane in panes), panes
        if page.viewport_size["width"] == 430:
            assert panes[1]["top"] > panes[0]["top"] + 20, panes
        return {"unnamedDraftCanCompare": True, "panes": panes, "comparisonRequest": calls[0], "modelOutputsAreSynthetic": True}

    for width, height in [(1440, 900), (1024, 768), (720, 640), (430, 720)]:
        case(f"comparison-{width}x{height}", responsive, width, height)

    def history_source(page):
        open_editor(page)
        page.get_by_role("combobox", name="选择试用文本", exact=True).click()
        page.get_by_role("option").last.click()
        sample = page.get_by_role("textbox", name="输入样例", exact=True)
        expect(sample).to_have_attribute("readonly", "")
        expect(page.locator("#mode-sample-origin")).to_contain_text("4 个原始片段")
        compare(page)
        payload = ledger(page, "mode.preview")[-1]["payload"]
        assert payload["source"] == {"sessionId": "ui-synthetic-session-1", "sourceRevisionId": "ui-source-0"}
        assert "sample" not in payload
        expect(page.locator(".mode-preview-origin")).to_contain_text("历史识别稿 · 4 个输入片段")
        page.get_by_role("button", name="转为自行输入", exact=True).click()
        expect(sample).not_to_have_attribute("readonly", "")
        expect(page.locator(".mode-preview")).to_have_attribute("data-stale", "true")
        sample.fill("自行输入的合成测试。\n如果没确认，先不要发邀请。")
        page.get_by_role("textbox", name="名称", exact=True).fill("合成项目整理")
        page.get_by_role("button", name="试用我的 Mode", exact=True).click()
        expect(page.locator(".mode-preview-origin")).to_contain_text("文本样例 · 2 个输入片段")
        calls = ledger(page, "mode.preview")
        assert len(calls) == 2
        assert "source" not in calls[-1]["payload"] and "sample" in calls[-1]["payload"]
        assert "compareBuiltins" not in calls[-1]["payload"] and "draft" in calls[-1]["payload"]
        return {"sourcePreserved": payload["source"], "explicitConversion": True, "ordinaryTrialSingleIpc": calls[-1]}

    case("history-source-and-free-text", history_source)

    def source_changed_during_trial(page, removed=False):
        open_editor(page)
        page.get_by_role("combobox", name="选择试用文本", exact=True).click()
        page.get_by_role("option").last.click()
        page.evaluate("window.__speakmodeQA.resultLabel='已完成的旧来源预览'")
        compare(page)
        page.evaluate("Object.assign(window.__speakmodeQA,{delayPreview:true,resultLabel:'迟到的旧来源输出'})")
        page.get_by_role("button", name="对比轻整理与结构", exact=True).click()
        page.wait_for_function("window.__speakmodeQA.pendingPreview.length===1")
        pending_id = ledger(page, "mode.preview")[-1]["payload"]["runId"]
        # Simulate a change made by another native window, then let the real
        # provider refresh and history.get path deliver it to the mounted editor.
        # No React state/internal component hook is bypassed.
        page.evaluate("""removed => {
          const q=window.__speakmodeQA;
          if(removed)q.fixture.details=[];
          else {
            const d=q.fixture.details[0];
            d.source.id='qa-replaced-source';d.session.currentSourceId=d.source.id;
            d.output.sourceRevisionId=d.source.id;
            d.source.items=d.source.items.map((item,index)=>({...item,id:'qa-replaced-item-'+index,text:index===0?'替换后的来源：预算三千五百元。':item.text}));
          }
          document.dispatchEvent(new Event('visibilitychange'));
        }""", removed)
        page.wait_for_function("runId=>window.__speakmodeQA.ledger.some(call=>call.name==='mode.cancelPreview'&&call.payload.runId===runId)", arg=pending_id)
        expect(page.locator(".mode-preview")).to_have_attribute("data-stale", "true")
        expect(page.locator(".mode-preview")).to_contain_text("输入已变化，请重新对比")
        assert len(ledger(page, "mode.preview")) == 2, "Source refresh unexpectedly started a model trial"
        if removed:
            expect(page.locator("#mode-sample-origin")).to_contain_text("所选历史记录已不可用")
            expect(page.get_by_role("textbox", name="输入样例", exact=True)).to_have_value("")
        else:
            expect(page.get_by_role("textbox", name="输入样例", exact=True)).to_contain_text("替换后的来源")
        page.evaluate("window.__speakmodeQA.resolvePreviews()")
        page.wait_for_timeout(100)
        expect(page.locator(".mode-preview")).to_contain_text("已完成的旧来源预览")
        expect(page.locator(".mode-preview")).not_to_contain_text("迟到的旧来源输出")
        page.evaluate("Object.assign(window.__speakmodeQA,{delayPreview:false,resultLabel:'已替换来源的新预览'})")
        page.get_by_role("button", name="对比轻整理与结构", exact=True).click()
        if removed:
            expect(page.locator("#mode-sample-error")).to_contain_text("这条记录已不可用")
            assert len(ledger(page, "mode.preview")) == 2
        else:
            expect(page.locator(".mode-preview")).to_contain_text("已替换来源的新预览")
            expect(page.locator(".mode-preview")).to_have_attribute("data-stale", "false")
            assert ledger(page, "mode.preview")[-1]["payload"]["source"]["sourceRevisionId"] == "qa-replaced-source"
        return {"removed": removed, "sourceRefreshCancelledRun": pending_id, "lateOutputIgnored": True,
                "sourceRefreshDidNotAutoRequest": True, "explicitRetryBlockedIfRemoved": removed}

    case("history-source-replaced-during-trial", source_changed_during_trial)
    case("history-source-removed-during-trial", lambda page: source_changed_during_trial(page, removed=True))

    def stale_and_cancel(page):
        open_editor(page)
        page.evaluate("Object.assign(window.__speakmodeQA,{delayPreview:true,resultLabel:'旧请求'})")
        page.get_by_role("button", name="对比轻整理与结构", exact=True).click()
        page.wait_for_function("window.__speakmodeQA.pendingPreview.length===1")
        first_id = ledger(page, "mode.preview")[0]["payload"]["runId"]
        page.get_by_role("textbox", name="输入样例", exact=True).fill("新的合成输入，如果没确认，不要发邀请。")
        page.wait_for_function("window.__speakmodeQA.ledger.some(c=>c.name==='mode.cancelPreview')")
        assert ledger(page, "mode.cancelPreview")[0]["payload"]["runId"] == first_id
        page.evaluate("Object.assign(window.__speakmodeQA,{delayPreview:false,resultLabel:'新请求'})")
        compare(page)
        expect(page.locator(".mode-preview")).to_contain_text("新请求")
        page.evaluate("window.__speakmodeQA.resolvePreviews()")
        page.wait_for_timeout(100)
        expect(page.locator(".mode-preview")).not_to_contain_text("旧请求")
        page.evaluate("Object.assign(window.__speakmodeQA,{delayPreview:true,resultLabel:'已取消请求'})")
        page.get_by_role("button", name="对比轻整理与结构", exact=True).click()
        page.wait_for_function("window.__speakmodeQA.pendingPreview.length===1")
        page.get_by_role("button", name="取消试用", exact=True).click()
        expect(page.locator(".mode-preview")).to_contain_text("已取消本次试用")
        page.evaluate("window.__speakmodeQA.resolvePreviews()")
        page.wait_for_timeout(100)
        expect(page.locator(".mode-preview")).to_contain_text("新请求")
        expect(page.locator(".mode-preview")).not_to_contain_text("已取消请求")
        return {"editCancelsRunId": first_id, "oldResponseIgnored": True, "cancelledResponseIgnored": True}

    case("trial-edit-cancel-late-response", stale_and_cancel)

    def trial_failure(page):
        open_editor(page)
        compare(page)
        previous_text = page.get_by_role("group", name="同一输入的内置模式对比").inner_text()
        page.evaluate("window.__speakmodeQA.failNextPreview=true")
        page.get_by_role("button", name="对比轻整理与结构", exact=True).click()
        expect(page.get_by_role("alert")).to_contain_text("合成模型请求失败")
        expect(page.get_by_role("button", name="对比轻整理与结构", exact=True)).to_be_enabled()
        assert page.get_by_role("group", name="同一输入的内置模式对比").inner_text() == previous_text
        return {"previousPreviewRetained": True, "retryEnabled": True}

    case("trial-failure-retains-result", trial_failure)

    def leave_pending(page):
        open_editor(page)
        page.evaluate("window.__speakmodeQA.delayPreview=true")
        page.get_by_role("button", name="对比轻整理与结构", exact=True).click()
        page.wait_for_function("window.__speakmodeQA.pendingPreview.length===1")
        run_id = ledger(page, "mode.preview")[0]["payload"]["runId"]
        page.get_by_role("button", name="返回 Mode 管理", exact=True).click()
        expect(page.get_by_role("heading", name="Mode 管理", exact=True)).to_be_visible()
        page.evaluate("window.__speakmodeQA.resolvePreviews()")
        page.wait_for_timeout(100)
        assert ledger(page, "mode.cancelPreview")[-1]["payload"]["runId"] == run_id
        expect(page.get_by_role("group", name="同一输入的内置模式对比")).to_have_count(0)
        return {"unmountCancelsCorrectRun": run_id, "noLateEditorReopen": True}

    case("trial-unmount-cancels", leave_pending)

    def candidate_changes(page):
        region = generate_candidate(page)
        expect(region.locator("mark.history-source-diff").first).to_be_visible()
        expect(region.locator("mark.history-current-diff").first).to_be_visible()
        before = region.locator(".history-sentence-line").first.inner_text()
        page.get_by_role("button", name="手动修改", exact=True).click()
        page.get_by_role("textbox", name="编辑当前文本", exact=True).fill("这是候选生成后的手动改稿，不应替换对照基线。")
        page.get_by_role("button", name="保存修改", exact=True).click()
        expect(page.get_by_role("button", name="采用这个版本", exact=True)).to_be_disabled()
        expect(page.get_by_role("alert")).to_contain_text("当前版本已改变")
        assert region.locator(".history-sentence-line").first.inner_text() == before
        expect(page.locator(".history-current")).to_contain_text("候选生成后的手动改稿")
        return {"changedCharactersVisible": True, "frozenBaselineAfterManualEdit": True, "staleAdoptionDisabled": True}

    case("candidate-diff-frozen-baseline", candidate_changes)

    def same_candidate(page):
        page.evaluate("window.__speakmodeQA.sameCandidate=true")
        region = generate_candidate(page)
        expect(region).to_contain_text("两份文本相同")
        expect(region.locator("mark")).to_have_count(0)
        return {"identicalOutputDoesNotPretendDifference": True}

    case("candidate-identical-output", same_candidate, 430, 720)

    def long_candidate(page):
        page.evaluate("window.__speakmodeQA.longCandidate=true")
        region = generate_candidate(page)
        expect(region).to_contain_text("整体标色")
        expect(region).to_contain_text("不表示每个字都被改动")
        return {"boundedDiffFallbackExplained": True}

    case("candidate-long-diff-fallback", long_candidate)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--serve", action="store_true")
    parser.add_argument("--case", help="Only run one named acceptance case")
    args = parser.parse_args()
    if urlparse(args.url).scheme != "http" or urlparse(args.url).hostname != "127.0.0.1" or not args.out.is_absolute():
        parser.error("Requires local http://127.0.0.1 URL and absolute fresh --out")
    args.out.mkdir(parents=True, exist_ok=False)
    rows, errors, external = [], [], []
    receipt = {"status": "RUNNING", "scope": __doc__, "startedAt": datetime.now(timezone.utc).isoformat(),
               "tests": rows, "pageErrors": errors, "blockedExternalRequests": external}
    receipt["distSha256"] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                             for path in sorted((ROOT / "app/dist").rglob("*")) if path.is_file()}
    receipt["scriptSha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    with ui.base.managed_preview(args.url, args.out, args.serve) as server, sync_playwright() as playwright:
        receipt["server"] = server
        browser = playwright.chromium.launch(channel="msedge", headless=True)
        receipt["browserVersion"] = browser.version
        try:
            acceptance(browser, args, rows, errors, external)
        finally:
            browser.close()
    receipt["counts"] = {"registered": len(rows), "passed": sum(row["status"] == "PASS" for row in rows),
                         "failed": sum(row["status"] == "FAIL" for row in rows)}
    receipt["status"] = "PASS" if rows and not errors and not external and not receipt["counts"]["failed"] else "FAIL"
    receipt["completedAt"] = datetime.now(timezone.utc).isoformat()
    ui.base.write_new(args.out / "receipt.json", receipt, json_value=True)
    print(json.dumps({key: receipt[key] for key in ["status", "counts", "pageErrors", "blockedExternalRequests"]}, ensure_ascii=False, indent=2))
    return 0 if receipt["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
