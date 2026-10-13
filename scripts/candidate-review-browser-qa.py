"""History candidate review with production React and isolated synthetic IPC.
No native app, microphone, provider, real credentials, history or clipboard.
"""
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
spec = importlib.util.spec_from_file_location('ui_base', ROOT / 'scripts/ui-experience-browser-qa.py')
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)

BEFORE = '项目预算是三千五百元，差旅另算。\n\n如果时间没确认，先不要发邀请。'
AFTER = '项目预算：五千元，差旅另算。\n\n如果时间没确认，先发邀请。\n\n沟通工具用 SpeakMode。'

def fixture():
    data = ui.fixture()
    data['details'] = data['details'][:1]
    d = data['details'][0]
    d['source']['items'] = [{**d['source']['items'][0], 'text': BEFORE}]
    d['output']['blocks'] = [{**d['output']['blocks'][0], 'text': BEFORE}]
    d['_title'] = '合成候选核对'
    data['detail'] = d
    return data

EXTRA = r"""
(() => {
 const q=window.__speakmodeQA,old=window.__TAURI_INTERNALS__.invoke,copy=x=>structuredClone(x);
 Object.assign(q,{after:__AFTER__,issues:['valuesChanged','meaningChanged'],candidate:null,undo:null,delayAdopt:false,pendingAdopt:[],delayCandidate:false,pendingCandidate:[],failAdopt:false,failList:false});
 window.__TAURI_INTERNALS__.invoke=async(command,args={})=>{
  if(command==='subscribe_state'){q.channel=args.channel;return old(command,args);}
  if(command!=='app_command')return old(command,args);
  const {name,payload}=args.request;
  if(name==='history.list'&&q.failList){q.failList=false;q.ledger.push({name,payload:copy(payload)});throw new Error('合成列表读取失败');}
  if(!['history.rewrite','history.cancelCandidate','revision.adopt','revision.undo'].includes(name))return old(command,args);
  q.ledger.push({name,payload:copy(payload)});const d=q.fixture.details.find(d=>d.session.id===payload.sessionId);
  if(name==='history.cancelCandidate'){q.candidate=null;return null;}
  if(name==='history.rewrite'){
   const candidate={...copy(d.output),id:payload.runId,baseRevisionId:payload.baseRevisionId,sourceRevisionId:payload.sourceRevisionId,creationReason:'history',blocks:[{id:'candidate-block',text:q.after,sourceRefs:d.source.items.map(i=>i.id)}],reviewIssues:copy(q.issues)};
   q.candidate=candidate;
   if(q.delayCandidate)return new Promise(resolve=>q.pendingCandidate.push(()=>resolve(copy(candidate))));
   return copy(candidate);
  }
  if(name==='revision.undo'){
   const undo=copy(q.undo);q.undo=null;d.output={...undo,id:'undone-output'};d.session.currentOutputId=d.output.id;d.canUndo=false;return copy(d);
  }
  if(q.failAdopt){q.failAdopt=false;throw new Error('合成本机保存失败，草稿仍保留');}
  if(q.delayAdopt)await new Promise(resolve=>q.pendingAdopt.push(resolve));
  if(payload.expectedCurrentRevisionId!==d.output.id||payload.candidateId!==q.candidate?.id)throw new Error('合成版本冲突');
  q.undo=copy(d.output);d.output={...copy(q.candidate),id:'saved-review',kind:'adopt',blocks:[{id:'reviewed-block',text:payload.reviewedText,sourceRefs:d.source.items.map(i=>i.id)}]};
  d.session.currentOutputId=d.output.id;d.canUndo=true;q.candidate=null;return copy(d);
 };
 q.advance=()=>{const d=q.fixture.details[0];d.output={...d.output,id:'newer-output',blocks:[{...d.output.blocks[0],text:'其他操作保存的新版本。'}]};d.session.currentOutputId=d.output.id;q.fixture.snapshot.sequence++;q.channel.onmessage({sequence:q.fixture.snapshot.sequence,kind:'history.changed',payload:null});};
})();
"""

def calls(page, name):
    return page.evaluate('(name)=>window.__speakmodeQA.ledger.filter(c=>c.name===name)', name)

def generate(page):
    page.get_by_role('button', name='查看与调整', exact=True).click()
    page.get_by_role('button', name='换种整理', exact=True).click()
    page.get_by_role('button', name='生成候选', exact=True).click()
    expect(page.get_by_role('heading', name='新候选', exact=True)).to_be_visible()
    return page.get_by_role('region', name='候选改动对照', exact=True)

def editor(page):
    page.get_by_role('button', name='编辑文本', exact=True).click()
    return page.get_by_role('textbox', name='编辑候选文本', exact=True)

def run(browser, args, rows, errors, external):
    def case(name, action, width=1440, height=900, motion='reduce'):
        if args.case and name != args.case: return
        row={'name':name,'status':'RUNNING'};rows.append(row)
        context=browser.new_context(viewport={'width':width,'height':height},locale='zh-CN',reduced_motion=motion)
        context.add_init_script(ui.BRIDGE.replace('__FIXTURE__',json.dumps(fixture(),ensure_ascii=False))+EXTRA.replace('__AFTER__',json.dumps(AFTER,ensure_ascii=False)))
        def route(r):
            if urlparse(r.request.url).hostname not in ('127.0.0.1',None):external.append(r.request.url);r.abort()
            else:r.continue_()
        context.route('**/*',route)
        page=context.new_page();page.on('pageerror',lambda e:errors.append({'case':name,'error':str(e)}))
        try:
            page.goto(args.url,wait_until='networkidle')
            expect(page.get_by_role('heading',name='最近一次口述',exact=True)).to_be_visible()
            row['observations']=action(page)
            row['measurement']=ui.measurements(page)
            assert not row['measurement']['overflow'],row['measurement']['overflow']
            row['ledger']=page.evaluate('window.__speakmodeQA.ledger')
            row['status']='PASS'
        except Exception as error:
            row.update(status='FAIL',error=str(error),traceback=traceback.format_exc())
        finally:
            page.screenshot(path=str(args.out/(name+'.png')))
            (args.out/(name+'.html')).write_text(page.content(),encoding='utf-8')
            context.close()

    def initial(page):
        region=generate(page)
        expect(region.locator('del').first).to_be_visible();expect(region.locator('ins').first).to_be_visible()
        expect(page.locator('.candidate-review-note')).to_contain_text('可保留原文或直接编辑后保存')
        expect(page.locator('.history-detail .inline-error')).to_have_count(0)
        expect(page.locator('.history-current')).to_contain_text(BEFORE)
        assert not calls(page,'revision.adopt')
        page.get_by_role('button',name='保存这个版本',exact=True).scroll_into_view_if_needed()
        return {'defaultDiffVisible':True,'noPrematureWrite':True}
    case('01-default-visible-review',initial)

    def choice(page):
        region=generate(page);button=region.locator('.candidate-choice-toggle').first
        button.click();expect(button).to_have_attribute('aria-pressed','false')
        editor(page)
        text=page.get_by_role('textbox',name='编辑候选文本').input_value()
        assert '三千五百元' in text and '先发邀请' in text and 'SpeakMode' in text,text
        page.get_by_role('button',name='改动对照',exact=True).click()
        button.click();expect(button).to_have_attribute('aria-pressed','true')
        page.get_by_role('button',name='保留全部原文',exact=True).click()
        expect(editor(page)).to_have_value(BEFORE)
        page.get_by_role('button',name='改动对照',exact=True).click()
        page.get_by_role('button',name='全部采用',exact=True).click()
        expect(editor(page)).to_have_value(AFTER)
        return {'localChoiceRestoresOnlyOneChange':True,'bulkReconstructionExact':True}
    case('02-selective-and-bulk-choices',choice)

    def save(page):
        generate(page);value='项目预算是三千五百元。🙂\n\n如果时间没确认，先不要发邀请。'
        editor(page).fill(value)
        page.get_by_role('button',name='保存这个版本',exact=True).click()
        expect(page.get_by_role('heading',name='新候选',exact=True)).to_have_count(0)
        expect(page.locator('.history-current')).to_contain_text(value)
        assert calls(page,'revision.adopt')[0]['payload']['reviewedText']==value
        page.get_by_role('button',name='撤销上次修改',exact=True).click()
        expect(page.locator('.history-current')).to_contain_text(BEFORE)
        assert not calls(page,'result.copy') and not calls(page,'app.showMain')
        return {'editedTextSavedExact':True,'undoRestoresBaseline':True,'noExternalPaste':True}
    case('03-edit-save-undo',save)

    def cancel(page):
        generate(page);editor(page).fill('未保存的修改。')
        page.get_by_role('button',name='取消候选',exact=True).click()
        expect(page.get_by_role('heading',name='新候选',exact=True)).to_have_count(0)
        expect(page.locator('.history-current')).to_contain_text(BEFORE)
        assert not calls(page,'revision.adopt') and len(calls(page,'history.cancelCandidate'))==1
        return {'candidateOnlyCancelled':True}
    case('04-cancel-preserves-current',cancel)

    def keyboard(page):
        region=generate(page);button=region.locator('.candidate-choice-toggle').first
        button.focus();button.press('Space');expect(button).to_have_attribute('aria-pressed','false')
        button.press('Enter');expect(button).to_have_attribute('aria-pressed','true')
        animation=button.locator('.candidate-choice-icon').evaluate('e=>getComputedStyle(e).animationName')
        assert animation=='none',animation
        return {'keyboardChoiceWorks':True,'reducedMotionStatic':True}
    case('05-keyboard-and-reduced-motion',keyboard,720,720)

    def navigation(page):
        generate(page);editor(page).fill('离开前核对过的修改。')
        page.get_by_role('navigation',name='主导航').get_by_role('button',name='首页',exact=True).click()
        dialog=page.get_by_role('alertdialog');expect(dialog).to_be_visible()
        dialog.get_by_role('button',name='继续编辑',exact=True).click()
        expect(page.get_by_role('textbox',name='编辑候选文本')).to_have_value('离开前核对过的修改。')
        page.get_by_role('navigation',name='主导航').get_by_role('button',name='首页',exact=True).click()
        page.get_by_role('alertdialog').get_by_role('button',name='保存并离开',exact=True).click()
        expect(page.get_by_role('heading',name='最近一次口述',exact=True)).to_be_visible()
        assert calls(page,'revision.adopt')[0]['payload']['reviewedText']=='离开前核对过的修改。'
        return {'unsavedEditGuard':True,'explicitSaveOnLeave':True}
    case('06-unsaved-navigation',navigation)

    def stale(page):
        generate(page);editor(page).fill('自己的草稿仍应保留。');page.evaluate('window.__speakmodeQA.advance()')
        expect(page.locator('.history-current')).to_contain_text('其他操作保存的新版本。')
        expect(page.get_by_role('button',name='保存这个版本',exact=True)).to_be_disabled()
        expect(page.get_by_role('textbox',name='编辑候选文本')).to_have_value('自己的草稿仍应保留。')
        expect(page.get_by_role('button',name='重新生成',exact=True)).to_be_enabled()
        assert not calls(page,'revision.adopt')
        return {'staleDraftEditable':True,'newerVersionProtected':True}
    case('07-stale-review',stale)

    def failed(page):
        generate(page);editor(page).fill('保存失败后继续修改。');page.evaluate('window.__speakmodeQA.failAdopt=true')
        page.get_by_role('button',name='保存这个版本',exact=True).click()
        expect(page.locator('.history-detail .inline-error')).to_contain_text('草稿仍保留')
        expect(page.get_by_role('textbox',name='编辑候选文本')).to_have_value('保存失败后继续修改。')
        page.get_by_role('textbox',name='编辑候选文本').fill('再次核对后保存。')
        page.get_by_role('button',name='保存这个版本',exact=True).click()
        expect(page.locator('.history-current')).to_contain_text('再次核对后保存。')
        return {'FailedSaveRetainsEditableDraft':True}
    case('08-save-failure-retry',failed)

    def pending(page):
        generate(page);page.evaluate('window.__speakmodeQA.delayAdopt=true')
        page.get_by_role('button',name='保存这个版本',exact=True).click()
        expect(page.get_by_role('button',name='保存这个版本',exact=True)).to_be_disabled()
        page.evaluate('window.__speakmodeQA.pendingAdopt.splice(0).forEach(resolve=>resolve())')
        expect(page.get_by_role('heading',name='新候选',exact=True)).to_have_count(0)
        assert len(calls(page,'revision.adopt'))==1
        return {'onlyOneSaveDuringPending':True}
    case('09-pending-save',pending)

    def long(page):
        page.evaluate("window.__speakmodeQA.after='复杂长文本候选。'.repeat(2000)")
        region=generate(page);expect(page.get_by_text('本次按整段对照',exact=False)).to_be_visible()
        assert region.inner_text().count('复杂长文本候选。')==2000
        page.get_by_role('button',name='保留全部原文',exact=True).click();expect(editor(page)).to_have_value(BEFORE)
        return {'boundedCoarseDiffExplained':True,'fullTextsPreserved':True}
    case('10-long-fallback',long,720,720)

    def narrow(page):
        region=generate(page);region.locator('.candidate-diff-change').first.click()
        expect(editor(page)).to_be_visible();page.get_by_role('button',name='改动对照',exact=True).click()
        page.get_by_role('button',name='保存这个版本',exact=True).scroll_into_view_if_needed()
        return {'narrowControlsAccessible':True}
    case('11-narrow-layout',narrow,430,760)

    def refresh_failure(page):
        generate(page);page.evaluate('window.__speakmodeQA.failList=true')
        page.get_by_role('button',name='保存这个版本',exact=True).click()
        expect(page.get_by_role('heading',name='新候选',exact=True)).to_have_count(0)
        expect(page.locator('.history-current')).to_contain_text(AFTER)
        expect(page.locator('.history-local-status')).to_contain_text('已保存你的选择')
        assert len(calls(page,'revision.adopt'))==1
        return {'CommittedSaveDoesNotReappearAsFailedCandidate':True}
    case('12-refresh-failure-after-commit',refresh_failure)

    def competing(page):
        generate(page)
        expect(page.get_by_role('button',name='手动修改',exact=True)).to_be_disabled()
        expect(page.get_by_role('button',name='纠正词条',exact=True)).to_be_disabled()
        expect(editor(page)).to_be_enabled()
        value=page.get_by_role('textbox',name='编辑候选文本').input_value()+'\n补充自己的文字。'
        page.get_by_role('textbox',name='编辑候选文本').fill(value)
        page.get_by_role('button',name='改动对照',exact=True).click()
        page.locator('.candidate-choice-toggle').first.click()
        assert '补充自己的文字。' in editor(page).input_value()
        return {'singleEditorOwner':True,'typedTextSurvivesLaterSelection':True}
    case('13-edit-owner-and-rebase',competing)

    def discarded(page):
        generate(page);editor(page).fill('放弃前的草稿。')
        page.get_by_role('navigation',name='主导航').get_by_role('button',name='首页',exact=True).click()
        page.get_by_role('alertdialog').get_by_role('button',name='放弃修改',exact=True).click()
        expect(page.get_by_role('heading',name='最近一次口述',exact=True)).to_be_visible()
        assert not calls(page,'revision.adopt') and len(calls(page,'history.cancelCandidate'))==1
        return {'discardCancelsOnce':True,'originalUntouched':True}
    case('14-discard-on-leave',discarded)

    def late(page):
        page.evaluate('window.__speakmodeQA.delayCandidate=true')
        page.get_by_role('button',name='查看与调整',exact=True).click()
        page.get_by_role('button',name='换种整理',exact=True).click()
        page.get_by_role('button',name='生成候选',exact=True).click()
        page.wait_for_function('window.__speakmodeQA.pendingCandidate.length===1')
        page.locator('.history-mode-panel').get_by_role('button',name='取消',exact=True).click()
        page.evaluate('window.__speakmodeQA.pendingCandidate.splice(0).forEach(resolve=>resolve())')
        expect(page.get_by_role('heading',name='新候选',exact=True)).to_have_count(0)
        expect(page.get_by_role('button',name='换种整理',exact=True)).to_be_enabled()
        assert not calls(page,'revision.adopt')
        return {'cancelledLateCandidateIgnored':True}
    case('15-cancel-late-generation',late)

    def same(page):
        page.evaluate('(text)=>{window.__speakmodeQA.after=text;window.__speakmodeQA.issues=[]}',BEFORE)
        region=generate(page);expect(page.locator('.candidate-change-count')).to_have_text('与生成前相同')
        expect(region.locator('del,ins,.candidate-choice-toggle')).to_have_count(0)
        expect(editor(page)).to_have_value(BEFORE)
        return {'noInventedDiff':True}
    case('16-identical-draft',same)

    def motion(page):
        region=generate(page);toggle=region.locator('.candidate-choice-toggle').first
        toggle.click();expect(toggle).to_have_attribute('aria-pressed','false')
        assert toggle.locator('.candidate-choice-icon').evaluate('e=>getComputedStyle(e).animationName')=='candidate-choice-in'
        return {'briefChoiceFeedback':True}
    case('17-choice-motion',motion,1024,760,'no-preference')

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url',required=True);parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--serve',action='store_true');parser.add_argument('--case')
    args=parser.parse_args()
    if urlparse(args.url).hostname!='127.0.0.1' or not args.out.is_absolute():parser.error('Local URL and absolute fresh output required')
    args.out.mkdir(parents=True,exist_ok=False)
    rows,errors,external=[],[],[]
    receipt={'startedAt':datetime.now(timezone.utc).isoformat(),'scope':__doc__,'tests':rows,'pageErrors':errors,'externalRequests':external}
    with ui.base.managed_preview(args.url,args.out,args.serve) as server,sync_playwright() as p:
        receipt['server']=server;browser=p.chromium.launch(channel='msedge',headless=True)
        receipt['browserVersion']=browser.version
        try:run(browser,args,rows,errors,external)
        finally:browser.close()
    receipt['counts']={'passed':sum(r['status']=='PASS' for r in rows),'failed':sum(r['status']=='FAIL' for r in rows)}
    receipt['status']='PASS' if rows and not receipt['counts']['failed'] and not errors and not external else 'FAIL'
    receipt['distSha256']={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((ROOT/'app/dist').rglob('*')) if p.is_file()}
    receipt['scriptSha256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    (args.out/'receipt.json').write_text(json.dumps(receipt,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps({k:receipt[k] for k in ['status','counts','pageErrors','externalRequests']},ensure_ascii=False,indent=2))
    return 0 if receipt['status']=='PASS' else 1

if __name__=='__main__':raise SystemExit(main())
