"""Production React confirmation flow with synthetic IPC only; no native/provider input."""
import importlib.util
import hashlib
import json
import traceback
from pathlib import Path
from urllib.parse import urlparse
from playwright.sync_api import expect

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('candidate_qa', ROOT / 'scripts/candidate-review-browser-qa.py')
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)
base_fixture = qa.fixture
BEFORE = '🙂 I work on graph rock. I expect to graduate in twenty thirty.\nThree oh, I have not released it. Another graph rock is uncertain.'
AFTER = '🙂 I work on GraphRAG. I expect to graduate in 2033. I have not released it. Another graph rock is uncertain.'

def fixture():
    data = base_fixture()
    d = data['details'][0]
    d['source']['items'] = [{**d['source']['items'][0], 'text': BEFORE}]
    d['output']['blocks'] = [{**d['output']['blocks'][0], 'text': BEFORE}]
    return data

EXTRA = r"""
(() => {
 const q=window.__speakmodeQA,old=window.__TAURI_INTERNALS__.invoke;
 q.issues=[];
 window.__TAURI_INTERNALS__.invoke=async(command,args={})=>{
  const response=await old(command,args);
  if(command==='app_command'&&args.request.name==='history.rewrite'){
   const context=structuredClone(args.request.payload.confirmedContext);
   response.confirmedContext=context;
   if(q.candidate)q.candidate.confirmedContext=context;
  }
  return response;
 };
})();
"""

def open_editor(page):
    page.get_by_role('button',name='查看与调整',exact=True).click()
    page.get_by_role('button',name='换种整理',exact=True).click()
    page.get_by_role('combobox',name='本条成稿语言',exact=True).click()
    page.get_by_role('option',name='英文成稿',exact=True).click()
    page.locator('.confirmed-context-editor summary').click()

def confirm(page, old, new):
    area=page.get_by_role('textbox',name='选择需要确认的识别内容',exact=True)
    area.evaluate('(e,old)=>{const start=e.value.indexOf(old); e.focus();e.setSelectionRange(start,start+old.length);e.dispatchEvent(new PointerEvent("pointerup",{bubbles:true}));}',old)
    page.get_by_role('textbox',name='这处识别内容的正确表达',exact=True).fill(new)
    button=page.get_by_role('button',name='确认这一处',exact=True)
    expect(button).to_be_enabled();button.click()

def run(browser,args,rows,errors,external):
    def case(name,action,width=1280):
        if args.case and args.case!=name:return
        row={'name':name,'status':'RUNNING'};rows.append(row)
        context=browser.new_context(viewport={'width':width,'height':900},locale='zh-CN',reduced_motion='reduce')
        context.add_init_script(qa.ui.BRIDGE.replace('__FIXTURE__',json.dumps(fixture(),ensure_ascii=False))+qa.EXTRA.replace('__AFTER__',json.dumps(AFTER,ensure_ascii=False))+EXTRA)
        def route(r):
            if urlparse(r.request.url).hostname not in ('127.0.0.1',None):external.append(r.request.url);r.abort()
            else:r.continue_()
        context.route('**/*',route)
        page=context.new_page();page.on('pageerror',lambda e:errors.append({'case':name,'error':str(e)}))
        try:
            page.goto(args.url,wait_until='networkidle')
            expect(page.get_by_role('heading',name='最近一次口述',exact=True)).to_be_visible()
            row['observations']=action(page);row['measurement']=qa.ui.measurements(page)
            assert not row['measurement']['overflow'],row['measurement']['overflow']
            row['ledger']=page.evaluate('window.__speakmodeQA.ledger')
            assert not [c for c in row['ledger'] if c['name'].startswith(('dictionary.','delivery.')) and c['name']!='dictionary.list']
            row['status']='PASS'
        except Exception as error:row.update(status='FAIL',error=str(error),traceback=traceback.format_exc())
        finally:
            page.screenshot(path=str(args.out/(name+'.png')))
            context.close()

    def adoption(page):
        open_editor(page);confirm(page,'graph rock','GraphRAG');confirm(page,'twenty thirty.\nThree oh','2033')
        page.get_by_role('button',name='生成候选',exact=True).click()
        expect(page.get_by_role('heading',name='新候选',exact=True)).to_be_visible()
        payload=qa.calls(page,'history.rewrite')[-1]['payload'];c=payload['confirmedContext']
        assert c['corrections'][0]['start']==BEFORE.index('graph rock')
        assert c['corrections'][1]['expectedText']=='twenty thirty.\nThree oh'
        assert c['sourceRevisionId']==payload['sourceRevisionId'] and c['language']=='english'
        assert not qa.calls(page,'revision.adopt')
        page.get_by_role('button',name='保存这个版本',exact=True).click()
        expect(page.get_by_role('heading',name='新候选',exact=True)).to_have_count(0)
        saved=page.evaluate('window.__speakmodeQA.fixture.details[0]')
        assert saved['source']['items'][0]['text']==BEFORE and saved['output']['confirmedContext']==c
        page.get_by_role('button',name='换种整理',exact=True).click()
        expect(page.get_by_role('combobox',name='本条成稿语言',exact=True)).to_contain_text('英文成稿')
        expect(page.locator('.confirmed-context-editor summary')).to_contain_text('2 处')
        page.locator('.history-mode-panel').get_by_role('button',name='取消',exact=True).click()
        page.get_by_role('button',name='撤销上次修改',exact=True).click()
        expect(page.locator('.history-current')).to_contain_text('graph rock')
        assert page.evaluate('window.__speakmodeQA.fixture.details[0].output.confirmedContext') is None
        return {'unicodeOffsets':True,'crossFragment':True,'savedAndReused':True,'undoRestoresContext':True}
    case('01-confirm-adopt-reuse-undo',adoption)

    def overlaps(page):
        open_editor(page);confirm(page,'graph rock','GraphRAG')
        area=page.get_by_role('textbox',name='选择需要确认的识别内容',exact=True)
        area.evaluate('(e)=>{const i=e.value.indexOf("rock");e.focus();e.setSelectionRange(i,i+4);e.dispatchEvent(new PointerEvent("pointerup",{bubbles:true}));}')
        page.get_by_role('textbox',name='这处识别内容的正确表达',exact=True).fill('RAG')
        expect(page.get_by_role('button',name='确认这一处',exact=True)).to_be_disabled()
        page.get_by_role('button',name='移除确认：GraphRAG',exact=True).click()
        expect(page.get_by_role('button',name='确认这一处',exact=True)).to_be_enabled()
        return {'overlapBlocked':True,'removalAvailable':True}
    case('02-overlap-remove-narrow',overlaps,760)

    def flagged(page):
        page.evaluate('window.__speakmodeQA.issues=["confirmedTextChanged","tenseChanged","reviewRejected"]')
        open_editor(page);confirm(page,'graph rock','GraphRAG');page.get_by_role('button',name='生成候选',exact=True).click()
        expect(page.locator('.candidate-review-note')).to_contain_text('已确认内容未按原样保留')
        expect(page.locator('.candidate-review-note')).to_contain_text('全文复核仍有疑点')
        expect(page.locator('.candidate-review-note')).to_contain_text('现在或过去的状态可能有变化')
        expect(qa.editor(page)).to_have_value(AFTER)
        assert not qa.calls(page,'revision.adopt')
        return {'draftRetainedForEditing':True,'explicitSaveRequired':True}
    case('03-flagged-candidate-retained',flagged)

    def cancelled(page):
        open_editor(page);confirm(page,'graph rock','GraphRAG');page.evaluate('window.__speakmodeQA.delayCandidate=true')
        page.get_by_role('button',name='生成候选',exact=True).click()
        page.wait_for_function('window.__speakmodeQA.pendingCandidate.length===1')
        expect(page.get_by_role('combobox',name='本条成稿语言',exact=True)).to_be_disabled()
        page.locator('.history-mode-panel').get_by_role('button',name='取消',exact=True).click()
        page.evaluate('window.__speakmodeQA.pendingCandidate.splice(0).forEach(resolve=>resolve())')
        expect(page.get_by_role('heading',name='新候选',exact=True)).to_have_count(0)
        assert not qa.calls(page,'revision.adopt')
        return {'lateResultIgnored':True}
    case('04-cancel-late-confirmed-candidate',cancelled)

    def keyboard(page):
        open_editor(page)
        area=page.get_by_role('textbox',name='选择需要确认的识别内容',exact=True)
        area.focus();area.press('Control+Home')
        for _ in range(BEFORE.index('graph rock')):area.press('ArrowRight')
        for _ in 'graph rock':area.press('Shift+ArrowRight')
        page.get_by_role('textbox',name='这处识别内容的正确表达',exact=True).fill('GraphRAG')
        expect(page.get_by_role('button',name='确认这一处',exact=True)).to_be_enabled()
        page.get_by_role('button',name='确认这一处',exact=True).click()
        expect(page.locator('.confirmed-context-editor li')).to_contain_text('graph rock')
        return {'keyboardSelectionPastEmoji':True}
    case('05-keyboard-unicode-selection',keyboard)

if __name__=='__main__':
    qa.run=run
    code=qa.main()
    import sys
    receipt_path=Path(sys.argv[sys.argv.index('--out')+1])/'receipt.json'
    receipt=json.loads(receipt_path.read_text(encoding='utf-8'))
    receipt['scope']=__doc__
    receipt['sharedHarnessSha256']=receipt['scriptSha256']
    receipt['scriptSha256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    receipt_path.write_text(json.dumps(receipt,ensure_ascii=False,indent=2),encoding='utf-8')
    raise SystemExit(code)
