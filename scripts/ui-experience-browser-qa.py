"""Capture production UI with synthetic history/settings in isolated headless Edge.

Requires a fresh absolute evidence directory. Local preview only; no native app,
provider, microphone, real credentials, clipboard or user-history access.
"""
from __future__ import annotations

import argparse
import copy
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
spec = importlib.util.spec_from_file_location("restore_qa", ROOT / "scripts/restore-delivery-browser-qa.py")
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)


def fixture():
    data = base.fixture()
    paragraphs = [
        "下周的 Windows 首版测试分成三轮，先验证右 Alt 开始和结束录音，再观察连续口述与长时间停顿时的反馈。时间还没有确认，先不要向所有同学发邀请。",
        "第一轮安排 6 位同学，每人完成 8 次口述，包含中文、英文缩写和专有词。预算是 3,500 元，不是 5,000 元；模型和网络出现异常时，也要保留能恢复的内容。",
        "第二轮把同一句内容分别用轻整理和结构模式输出，重点检查事实、否定、条件和不确定性。用户可以核对识别稿、恢复选中的内容，但不应该被迫逐字检查所有结果。",
        "第三轮在记事本、网页输入框以及会议笔记里测试。说话时用户可能切到资料页面，结束时应确认交付状态；没有可靠输入位置时，成稿仍可在历史中找到并复制。",
        "先记录观察到的困难，再安排修改优先级。操作时长、重复点击和用户能否理解当前状态都要分别记录，不能把模拟测试当成真实模型质量的证明。",
        "如果麦克风连接中断，先明确提示失败原因，并保留可以恢复的结果。网络恢复后由用户主动重试；不要把上一条内容错误地插入到新的任务里。",
        "词典准备 SpeakMode、WebView2、Qwen 和项目名称。每条记录都保留原有条件，避免自动纠正改变数字或人名；修改后的结果需要能撤销。",
        "评审资料周五 18:30 前汇总，收集页面截图、操作录像和测试记录。当前所有案例都是原型验收所用的合成数据，不包含同学的真实个人信息。",
        "一轮测试结束后再安排下一轮，期间留出设备检查时间。若课堂时间发生变化，先确认会议邀请，再更新每一位参与者的安排。",
        "最终交付包含可运行程序、技术说明以及 HCI 的交互评估。保持操作简单，让用户知道什么时候正在录音、什么时候正在整理，以及结果在哪里。",
    ]
    titles = ["Windows 首版测试安排", "HCI 访谈后的三个发现", "发给同学的会前说明", "预算与行程确认", "接口联调记录", "下一轮用户测试", "专有词的识别检查", "保留条件与否定", "整理周五的待办", "模型质量观察", "新版本交付清单", "下周课堂展示安排", "开发环境的问题", "研究材料的补充", "会议时间尚未确定", "记录一次恢复流程"]
    details = []
    for i, title in enumerate(titles):
        d = copy.deepcopy(data["detail"])
        sid = f"ui-synthetic-session-{i + 1}"
        stamp = f"2026-10-{9 if i < 7 else 8:02d}T{6 - i % 6:02d}:26:00Z"
        blocks = paragraphs if i == 0 else [paragraphs[i % len(paragraphs)], paragraphs[(i + 2) % len(paragraphs)]]
        d["session"].update(id=sid, startedAt=stamp, endedAt=stamp, endLocalDate=stamp[:10], captureDurationMs=94000)
        d["source"].update(id=f"ui-source-{i}", sessionId=sid)
        d["source"]["items"] = [{"id": f"ui-source-{i}-{j}", "providerItemId": f"ui-{i}-{j}", "captureOrder": j, "text": text, "final": True, "startMs": None, "endMs": None} for j, text in enumerate(blocks)]
        d["output"].update(id=f"ui-output-{i}", sessionId=sid, sourceRevisionId=d["source"]["id"])
        d["output"]["blocks"] = [{"id": f"ui-block-{i}-{j}", "text": title + "：" + text if j == 0 else text, "sourceRefs": [f"ui-source-{i}-{j}"]} for j, text in enumerate(blocks)]
        d["session"].update(currentSourceId=d["source"]["id"], currentOutputId=d["output"]["id"])
        d["_title"] = title
        details.append(d)
    data["details"] = details
    data["detail"] = details[0]
    snap = data["snapshot"]
    snap["usage"] = {"date": "2026-10-09", "count": 28, "sourceCharCount": 12864, "captureDurationMs": 2074000}
    snap["devices"] = [{"id": "ui-microphone", "name": "USB 桌面麦克风（合成设备）", "isDefault": True, "available": True}]
    snap["provider"]["config"].update(provider="synthetic", region="beijing", asrModel="synthetic-asr", rewriteModel="synthetic-rewrite")
    snap["provider"]["modelCatalog"] = {"asr": [{"id": "synthetic-asr", "label": "测试语音模型", "implemented": True, "adapterVersion": "synthetic-browser-v1", "dictionarySupport": "vocabularyAndContext"}], "rewrite": [{"id": "synthetic-rewrite", "label": "测试整理模型", "implemented": True}]}
    snap["dictionary"]["entries"] = [{"id": f"ui-term-{i}", "term": term, "createdAt": base.NOW, "updatedAt": base.NOW} for i, term in enumerate(["SpeakMode", "WebView2", "PolyU"])]
    return data


BRIDGE = r"""
(() => {
 const fixture=__FIXTURE__,copy=value=>structuredClone(value),ledger=[],callbacks=new Map();let callbackId=0;
 window.isTauri=true;window.__speakmodeQA={ledger,fixture,failNextPreference:false,delayHistory:false,pendingHistory:[],delayPreference:false,pendingPreference:[],pendingRewrite:[],pendingCancel:[]};
 function summary(d){const o=d.output;return {sessionId:d.session.id,title:d._title,preview:o.blocks.map(b=>b.text).join(' '),endedAt:d.session.endedAt,endLocalDate:d.session.endLocalDate,modeId:o.modeSnapshot.modeId,modeName:o.modeSnapshot.name,resolvedMode:o.resolvedMode,currentRevisionId:o.id,sourceRevisionId:d.source.id,status:'ready',sourceCharCount:800,captureDurationMs:d.session.captureDurationMs};}
 window.__TAURI_INTERNALS__={
 transformCallback(fn){const id=++callbackId;callbacks.set(id,fn);return id;},unregisterCallback(id){callbacks.delete(id);},
 async invoke(command,args={}){
  if(command==='subscribe_state')return copy(fixture.snapshot);
  if(command==='control_main_window')return {maximized:false,minimized:false};
  if(command!=='app_command')throw new Error('Blocked synthetic IPC: '+command);
  const {name,payload}=args.request;ledger.push({name,payload:copy(payload)});
  if(name==='state.snapshot')return copy(fixture.snapshot);
  if(name==='usage.today'){const now=new Date(),date=`${now.getFullYear()}-${String(now.getMonth()+1).padStart(2,'0')}-${String(now.getDate()).padStart(2,'0')}`;return {...copy(fixture.snapshot.usage),date};}
  if(name==='usage.total'){const {date,...totals}=fixture.snapshot.usage;return copy(totals);}
  if(name==='audio.listDevices')return copy(fixture.snapshot.devices);
  if(name==='history.list'){if(window.__speakmodeQA.delayHistory)await new Promise(resolve=>window.__speakmodeQA.pendingHistory.push(resolve));const items=fixture.details.filter(d=>!payload.query||JSON.stringify(d).includes(payload.query)).map(summary);return {items,total:items.length,nextCursor:null};}
  if(name==='history.get'){const d=fixture.details.find(d=>d.session.id===payload.sessionId);if(!d)throw new Error('Unknown synthetic ID');return copy(d);}
  if(name==='result.copy'||name==='window.resizeRecorder'||name==='app.showMain')return null;
  if(name==='preferences.update'){
   if(window.__speakmodeQA.delayPreference)await new Promise(resolve=>window.__speakmodeQA.pendingPreference.push(resolve));
   if(window.__speakmodeQA.failNextPreference){window.__speakmodeQA.failNextPreference=false;throw new Error('模拟保存失败，请重试。');}
   if(payload.expectedVersion!==fixture.snapshot.preferences.version)throw new Error('Synthetic stale preference version');
   Object.assign(fixture.snapshot.preferences,payload.patch);fixture.snapshot.preferences.version++;fixture.snapshot.sequence++;return null;
  }
  if(name==='mode.setDefault'){fixture.snapshot.preferences.defaultModeId=payload.modeId;fixture.snapshot.preferences.version++;fixture.snapshot.sequence++;return null;}
  if(name==='dictionary.list')return copy(fixture.snapshot.dictionary);
  if(name==='dictionary.delete'){const d=fixture.snapshot.dictionary;if(payload.expectedVersion!==d.version)throw new Error('Synthetic stale dictionary');d.entries=d.entries.filter(e=>e.id!==payload.entryId);d.version++;return copy(d);}
  if(name==='history.rewrite'){await new Promise(resolve=>window.__speakmodeQA.pendingRewrite.push(resolve));const d=fixture.details.find(d=>d.session.id===payload.sessionId);return {...copy(d.output),id:payload.runId,baseRevisionId:payload.baseRevisionId,creationReason:'history'};}
  if(name==='history.cancelCandidate'){await new Promise(resolve=>window.__speakmodeQA.pendingCancel.push(resolve));return null;}
  throw new Error('Blocked synthetic app command: '+name);
 }
 };
})();
"""


def measurements(page):
    return page.evaluate("""() => {
      const main=document.querySelector('.app-main');
      const rect=(selector)=>{const e=document.querySelector(selector);if(!e)return null;const b=e.getBoundingClientRect();return {top:b.top,bottom:b.bottom,left:b.left,right:b.right,width:b.width,height:b.height,clientHeight:e.clientHeight,scrollHeight:e.scrollHeight,visible:b.height>0&&b.bottom>0&&b.top<innerHeight};};
      return {viewport:{width:innerWidth,height:innerHeight},documentWidth:document.documentElement.scrollWidth,main:rect('.app-main'),mainScrollTop:main?.scrollTop,usage:rect('.usage-overview'),latest:rect('.latest-card'),latestActions:rect('.latest-actions'),historyIndex:rect('.history-index'),historySearch:rect('.history-search'),historyGroups:rect('.history-record-groups'),historyDetail:rect('.history-detail'),historyTools:rect('.history-tools'),settingsFeedback:rect('.settings-feedback'),settingsInlineFeedback:rect('[data-setting-feedback]'),overflow:[...document.querySelectorAll('main *')].filter(e=>{const b=e.getBoundingClientRect();return b.width>0&&(b.right>innerWidth+2||b.left<-2)}).slice(0,20).map(e=>({tag:e.tagName,class:e.className,text:e.textContent?.slice(0,80)}))};
    }""")


def capture(page, out, tag):
    page.screenshot(path=str(out / f"{tag}.png"), full_page=False)
    base.write_new(out / f"{tag}.html", page.content())
    values = measurements(page)
    values["controls"] = page.locator("button,input,textarea,[role=combobox]").evaluate_all("nodes=>nodes.map(e=>({tag:e.tagName,name:e.getAttribute('aria-label'),text:e.textContent,disabled:e.disabled}))")
    base.write_new(out / f"{tag}.json", values, json_value=True)
    return values


def in_view(locator):
    return locator.evaluate("e=>{const b=e.getBoundingClientRect(),m=document.querySelector('.app-main').getBoundingClientRect();return b.width>0&&b.height>0&&b.top>=m.top-1&&b.bottom<=m.bottom+1&&b.left>=0&&b.right<=innerWidth+1}")


def acceptance(browser, args, rows, errors, external):
    def case(name, action, width=1440, height=900, data=None):
        if args.case and args.case!=name:return
        row={"name":name,"status":"RUNNING"};rows.append(row)
        context=browser.new_context(viewport={"width":width,"height":height},locale="zh-CN",reduced_motion="reduce")
        context.add_init_script(BRIDGE.replace("__FIXTURE__",json.dumps(data or fixture(),ensure_ascii=False)))
        def route(r):
            if urlparse(r.request.url).hostname not in ("127.0.0.1",None):external.append(r.request.url);r.abort()
            else:r.continue_()
        context.route("**/*",route)
        page=context.new_page();page.on("pageerror",lambda e:errors.append({"case":name,"error":str(e)}))
        try:
            page.goto(args.url,wait_until="networkidle")
            expect(page.get_by_role("heading",name="最近一次口述",exact=True)).to_be_visible()
            row["observations"]=action(page)
            row["final"]=capture(page,args.out,name)
            row["ledger"]=page.evaluate("window.__speakmodeQA.ledger")
            assert not row["final"]["overflow"],row["final"]["overflow"]
            row["status"]="PASS"
        except Exception as e:
            row.update(status="FAIL",error=str(e),traceback=traceback.format_exc())
            capture(page,args.out,name+"-failure")
        finally:context.close()

    def home(page):
        first=measurements(page)
        copy_button=page.get_by_role("button",name="复制结果",exact=True)
        assert in_view(copy_button),first
        expect(page.get_by_role("region",name="今日概览")).to_contain_text("12,864")
        more=page.get_by_role("button",name="展开全文",exact=True)
        expect(more).to_have_attribute("aria-expanded","false")
        more.click()
        less=page.get_by_role("button",name="收起全文",exact=True)
        expect(less).to_have_attribute("aria-expanded","true")
        expanded=measurements(page)
        assert expanded["latest"]["height"]>first["latest"]["height"]+100,(first,expanded)
        expect(page.locator(".latest-card")).to_contain_text("以及结果在哪里")
        less.click()
        folded=page.get_by_role("button",name="展开全文",exact=True)
        expect(folded).to_have_attribute("aria-expanded","false")
        expect(folded).to_be_focused();assert in_view(folded)
        assert in_view(copy_button),measurements(page)
        copy_button.click()
        expect(copy_button).to_contain_text("已复制")
        request=page.evaluate("window.__speakmodeQA.ledger.filter(x=>x.name==='result.copy')")
        assert len(request)==1 and request[0]["payload"]["outputRevisionId"]=="ui-output-0",request
        assert len(page.evaluate("window.__speakmodeQA.fixture.details[0].output.blocks"))==10
        return {"collapsed":first,"expanded":expanded,"copyRequest":request[0],"scope":"Full-result revision requested; native clipboard is not exercised."}
    case("home-preview-and-copy",home)

    short=fixture()
    short["details"][0]["output"]["blocks"]=[{"id":"ui-short-block","text":"会议时间还没有确认，先不要发邀请。","sourceRefs":["ui-source-0-0"]}]
    def short_home(page):
        expect(page.locator(".latest-card")).to_contain_text("会议时间还没有确认，先不要发邀请。")
        expect(page.get_by_role("button",name="展开全文",exact=True)).to_have_count(0)
        expect(page.locator(".latest-preview-viewport")).to_have_attribute("data-overflowing","false")
        return {"shortTextDoesNotShowExpand":True}
    case("home-short-result",short_home,data=short)

    def nav(page):
        nav=page.get_by_role("navigation",name="主导航")
        buttons=nav.get_by_role("button")
        assert buttons.count()==5
        boxes=[b.bounding_box() for b in buttons.all()]
        assert max(b["y"] for b in boxes)-min(b["y"] for b in boxes)<2,boxes
        assert max(b["width"] for b in boxes)-min(b["width"] for b in boxes)<2,boxes
        nav.get_by_role("button",name="历史记录",exact=True).click()
        expect(page.get_by_role("heading",name="历史记录",exact=True)).to_be_visible()
        return {"navigationBoxes":boxes}
    case("narrow-navigation-five-columns",nav,720,640)

    def history(page):
        page.get_by_role("button",name="查看与调整",exact=True).click()
        expect(page.locator(".history-detail")).to_be_visible()
        page.locator(".app-main").evaluate("e=>e.scrollTo(0,0)")
        scrolling=page.locator(".history-list-scroll")
        first=measurements(page)
        assert first["historyIndex"]["bottom"]<=first["main"]["bottom"],first
        before=page.locator(".history-detail").bounding_box()
        scrolling.hover();page.mouse.wheel(0,620)
        page.wait_for_function("document.querySelector('.history-list-scroll').scrollTop>100")
        after=page.locator(".history-detail").bounding_box()
        assert abs(after["y"]-before["y"])<2,(before,after)
        assert in_view(page.get_by_role("searchbox",name="搜索口述记录"))
        page.locator(".history-current").hover();page.mouse.wheel(0,440)
        page.wait_for_timeout(100)
        assert in_view(page.get_by_role("searchbox",name="搜索口述记录"))
        return {"detailBefore":before,"detailAfterListScroll":after,"listScrollTop":scrolling.evaluate("e=>e.scrollTop")}
    case("history-independent-list-scroll",history)

    def empty_loading(page):
        page.get_by_role("navigation",name="主导航").get_by_role("button",name="历史记录",exact=True).click()
        search=page.get_by_role("searchbox",name="搜索口述记录")
        search.fill("不存在的合成记录")
        expect(page.get_by_role("heading",name="没有找到这段口述",exact=True)).to_be_visible()
        page.evaluate("window.__speakmodeQA.delayHistory=true")
        search.fill("下一轮用户测试")
        expect(page.get_by_role("heading",name="正在读取记录",exact=True)).to_be_visible()
        expect(page.locator(".history-list-scroll")).to_have_attribute("aria-busy","true")
        expect(page.get_by_role("heading",name="没有找到这段口述",exact=True)).to_have_count(0)
        page.evaluate("window.__speakmodeQA.delayHistory=false;window.__speakmodeQA.pendingHistory.splice(0).forEach(f=>f())")
        expect(page.locator(".history-list-scroll")).to_have_attribute("aria-busy","false")
        expect(page.locator(".history-record-list button")).to_have_count(1)
        return {"loadingReplacesStaleEmpty":True}
    case("history-empty-to-loading",empty_loading)

    def panels(page):
        page.get_by_role("button",name="查看与调整",exact=True).click()
        expect(page.locator(".history-detail")).to_be_visible()
        mode=page.get_by_role("button",name="换种整理",exact=True)
        mode.click();heading=page.get_by_role("heading",name="选择整理方式",exact=True)
        expect(heading).to_be_focused();assert in_view(heading)
        page.locator(".history-mode-panel").get_by_role("button",name="取消",exact=True).click()
        expect(mode).to_be_focused();assert in_view(mode)
        correction=page.get_by_role("button",name="纠正词条",exact=True)
        correction.click();heading=page.get_by_role("heading",name="纠正一个词",exact=True)
        expect(heading).to_be_focused();assert in_view(heading)
        page.get_by_role("button",name="取消纠正",exact=True).click()
        expect(correction).to_be_focused()
        delete=page.get_by_role("button",name="删除记录",exact=True)
        delete.click();heading=page.get_by_role("heading",name="删除这条口述？",exact=True)
        expect(heading).to_be_focused();assert in_view(heading)
        page.get_by_role("button",name="保留",exact=True).click()
        expect(delete).to_be_focused()
        return {"panelHeadingFocusAndReturn":True}
    case("history-panel-focus",panels,1024,768)

    def delayed_cancel(page):
        page.get_by_role("button",name="查看与调整",exact=True).click()
        mode=page.get_by_role("button",name="换种整理",exact=True)
        mode.click()
        page.get_by_role("button",name="生成候选",exact=True).click()
        page.wait_for_function("window.__speakmodeQA.pendingRewrite.length===1")
        page.locator(".history-mode-panel").get_by_role("button",name="取消",exact=True).click()
        page.wait_for_function("window.__speakmodeQA.pendingCancel.length===1")
        expect(mode).to_be_enabled()
        # Reopen through real controls, regardless of whether the previous panel
        # closes immediately or awaits the old cancellation acknowledgement.
        if mode.get_attribute("aria-expanded")=="true":mode.click()
        mode.click()
        heading=page.get_by_role("heading",name="选择整理方式",exact=True)
        expect(heading).to_be_focused()
        before=heading.bounding_box()
        page.evaluate("window.__speakmodeQA.pendingCancel.splice(0).forEach(f=>f());window.__speakmodeQA.pendingRewrite.splice(0).forEach(f=>f())")
        page.wait_for_timeout(120)
        expect(heading).to_be_visible();expect(heading).to_be_focused();assert in_view(heading)
        expect(mode).to_have_attribute("aria-expanded","true")
        expect(page.get_by_role("heading",name="新候选",exact=True)).to_have_count(0)
        return {"reopenedPanelBeforeAck":before,"afterAck":heading.bounding_box(),"oldRewriteDidNotBecomeCandidate":True}
    case("history-delayed-cancel-keeps-new-panel",delayed_cancel)

    def settings(page):
        page.get_by_role("navigation",name="主导航").get_by_role("button",name="设置",exact=True).click()
        language=page.get_by_role("combobox",name="识别语言",exact=True)
        row=language.locator("xpath=ancestor::div[contains(concat(' ', normalize-space(@class), ' '), ' setting-row ')][1]")
        page.evaluate("window.__speakmodeQA.delayPreference=true")
        language.click();page.get_by_role("option",name="中文",exact=True).click()
        expect(row).to_contain_text("正在保存")
        assert in_view(row.get_by_role("status")),row.inner_text()
        expect(language).to_be_disabled()
        page.evaluate("window.__speakmodeQA.delayPreference=false;window.__speakmodeQA.pendingPreference.splice(0).forEach(f=>f())")
        expect(language).to_contain_text("中文")
        expect(row).to_contain_text("已保存")
        assert in_view(row.get_by_role("status")),row.inner_text()
        page.evaluate("window.__speakmodeQA.failNextPreference=true")
        language.click();page.get_by_role("option",name="English",exact=True).click()
        expect(row.get_by_role("alert")).to_contain_text("模拟保存失败")
        assert in_view(row.get_by_role("alert"))
        expect(language).to_contain_text("中文")
        return {"rowText":row.inner_text(),"savedLanguage":page.evaluate("window.__speakmodeQA.fixture.snapshot.preferences.language")}
    case("settings-local-feedback",settings,1024,768)

    many=fixture()
    many["snapshot"]["modes"] += [{"id":f"ui-personal-{i}","name":f"研究访谈整理：保留条件与否定 {i:02d}","baseMode":"structure","rules":"按访谈问题组织内容，保留原有条件，不推测未表达的观点。","revision":1,"builtin":False} for i in range(10)]
    def compact_modes(page):
        page.get_by_role("navigation",name="主导航").get_by_role("button",name="设置",exact=True).click()
        chooser=page.get_by_role("combobox",name="新口述默认 Mode",exact=True)
        chooser.click()
        options=page.get_by_role("option")
        expect(options).to_have_count(13)
        labels=options.all_inner_texts()
        assert len([name for name in labels if "研究访谈整理" in name])==10,labels
        options_data=options.evaluate_all("nodes=>nodes.map(e=>({text:e.textContent,width:e.clientWidth,scrollWidth:e.scrollWidth}))")
        assert all(item["scrollWidth"]<=item["width"]+1 for item in options_data),options_data
        page.screenshot(path=str(args.out/"settings-many-personal-modes-menu.png"))
        target=page.get_by_role("option",name="研究访谈整理：保留条件与否定 09",exact=True)
        target.click()
        expect(chooser).to_contain_text("研究访谈整理：保留条件与否定 09")
        assert page.evaluate("window.__speakmodeQA.fixture.snapshot.preferences.defaultModeId")=="ui-personal-9"
        return {"selectedMode":chooser.inner_text(),"modeCount":13,"menuOptions":options_data}
    case("settings-many-personal-modes",compact_modes,720,640,many)

    def dictionary(page):
        page.get_by_role("navigation",name="主导航").get_by_role("button",name="词典",exact=True).click()
        edit=page.get_by_role("button",name="编辑词条 SpeakMode",exact=True)
        edit.click();expect(page.get_by_role("textbox",name="词条",exact=True)).to_be_focused()
        page.get_by_role("button",name="取消",exact=True).click();expect(edit).to_be_focused()
        remove=page.get_by_role("button",name="删除词条 SpeakMode",exact=True)
        remove.click();page.get_by_role("button",name="保留",exact=True).click();expect(remove).to_be_focused()
        remove.click();page.get_by_role("button",name="删除词条",exact=True).click()
        expect(page.get_by_role("button",name="编辑词条 SpeakMode",exact=True)).to_have_count(0)
        assert page.evaluate("document.activeElement!==document.body"),"Deleted row left focus on body"
        focus_after_delete=page.evaluate("({tag:document.activeElement.tagName,name:document.activeElement.getAttribute('aria-label'),text:document.activeElement.textContent})")
        search=page.get_by_role("textbox",name="搜索词条",exact=True)
        search.fill("WebView")
        page.get_by_role("button",name="清除搜索",exact=True).click()
        expect(search).to_be_focused();expect(search).to_have_value("")
        return {"focusAfterDelete":focus_after_delete,"clearSearchRefocus":True}
    case("dictionary-focus-return",dictionary,720,640)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--serve", action="store_true")
    parser.add_argument("--baseline", action="store_true", help="Capture baseline without asserting proposed improvements")
    parser.add_argument("--case", help="Run one named acceptance case, without the viewport capture sweep")
    args = parser.parse_args()
    if urlparse(args.url).scheme != "http" or urlparse(args.url).hostname != "127.0.0.1" or not args.out.is_absolute():
        parser.error("Requires local http://127.0.0.1 URL and absolute fresh --out")
    args.out.mkdir(parents=True, exist_ok=False)
    rows, errors, external = [], [], []
    receipt = {"status":"RUNNING","scope":"Production React with isolated synthetic bridge; no native, microphone, provider, credential or host input acceptance.","startedAt":datetime.now(timezone.utc).isoformat(),"baseline":args.baseline,"tests":rows,"pageErrors":errors,"blockedExternalRequests":external}
    receipt["distSha256"] = {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((ROOT / "app/dist").rglob("*")) if p.is_file()}
    receipt["scriptSha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    with base.managed_preview(args.url,args.out,args.serve) as server, sync_playwright() as p:
        receipt["server"] = server
        browser = p.chromium.launch(channel="msedge",headless=True)
        receipt["browserVersion"] = browser.version
        try:
            for width,height in ([] if args.case else [(1440,900),(1024,768),(720,640)] if args.baseline else [(1440,900),(1024,768),(720,640),(430,720)]):
                context = browser.new_context(viewport={"width":width,"height":height},locale="zh-CN")
                context.add_init_script(BRIDGE.replace("__FIXTURE__",json.dumps(fixture(),ensure_ascii=False)))
                def route(r):
                    if urlparse(r.request.url).hostname not in ("127.0.0.1",None):external.append(r.request.url);r.abort()
                    else:r.continue_()
                context.route("**/*",route)
                page=context.new_page();page.on("pageerror",lambda e:errors.append(str(e)))
                tag=f"{width}x{height}"
                row={"name":tag,"status":"RUNNING","observations":{}};rows.append(row)
                try:
                    page.goto(args.url,wait_until="networkidle")
                    expect(page.get_by_role("heading",name="最近一次口述",exact=True)).to_be_visible()
                    row["observations"]["home"] = capture(page,args.out,f"{tag}-home")
                    page.get_by_role("button",name="查看与调整",exact=True).click()
                    expect(page.locator(".history-detail")).to_be_visible()
                    row["observations"]["history"] = capture(page,args.out,f"{tag}-history")
                    page.locator(".history-current").hover();page.mouse.wheel(0,900);page.wait_for_timeout(300)
                    row["observations"]["historyScrolled"] = capture(page,args.out,f"{tag}-history-scrolled")
                    page.get_by_role("navigation",name="主导航").get_by_role("button",name="设置",exact=True).click()
                    expect(page.get_by_role("combobox",name="识别语言",exact=True)).to_be_visible()
                    row["observations"]["settings"] = capture(page,args.out,f"{tag}-settings")
                    page.get_by_role("combobox",name="识别语言",exact=True).click()
                    page.get_by_role("option",name="中文",exact=True).click()
                    expect(page.get_by_role("combobox",name="识别语言",exact=True)).to_contain_text("中文")
                    row["observations"]["settingsSaved"] = capture(page,args.out,f"{tag}-settings-saved")
                    row["ledger"] = page.evaluate("window.__speakmodeQA.ledger")
                    row["status"]="PASS"
                except Exception as e:
                    row.update(status="FAIL",error=str(e),traceback=traceback.format_exc())
                    page.screenshot(path=str(args.out/f"{tag}-failure.png"))
                finally:context.close()
            if not args.baseline:
                acceptance(browser,args,rows,errors,external)
        finally:browser.close()
    receipt["counts"]={"registered":len(rows),"passed":sum(r["status"]=="PASS" for r in rows),"failed":sum(r["status"]=="FAIL" for r in rows)}
    receipt["status"]="PASS" if rows and not errors and not external and receipt["counts"]["failed"]==0 else "FAIL"
    receipt["completedAt"]=datetime.now(timezone.utc).isoformat()
    base.write_new(args.out/"receipt.json",receipt,json_value=True)
    print(json.dumps({k:receipt[k] for k in ["status","counts","pageErrors","blockedExternalRequests"]},ensure_ascii=False,indent=2))
    return 0 if receipt["status"]=="PASS" else 1


if __name__=="__main__":sys.exit(main())
