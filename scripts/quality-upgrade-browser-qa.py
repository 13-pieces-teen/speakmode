"""Production React correction and personal Mode UI with an isolated synthetic bridge.

All text is synthetic. No cloud, credentials, microphone, native Store, clipboard
or host insertion is exercised. This observes frontend behavior only.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import sys
from urllib.parse import urlparse
from playwright.sync_api import sync_playwright, expect

spec = importlib.util.spec_from_file_location('restore_qa', Path(__file__).with_name('restore-delivery-browser-qa.py'))
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)

EXTENSION = r"""
   if(name==='mode.save') {
    const draft=payload.draft;
    if(draft.id) {
     const current=fixture.snapshot.modes.find(mode=>mode.id===draft.id);
     if(!current||current.revision!==draft.expectedRevision)throw new Error('Synthetic stale Mode');
    }
    const saved={id:draft.id||'personal-synthetic',name:draft.name,baseMode:draft.baseMode,rules:draft.rules,revision:(draft.expectedRevision||0)+1,builtin:false};
    fixture.snapshot.modes=fixture.snapshot.modes.filter(mode=>mode.id!==saved.id).concat([saved]);
    return copy(saved);
   }
   if(name==='revision.correctTerm') {
    const d=fixture.detail,o=d.output,p=payload.patch;
    if(window.__speakmodeQA.failCorrection)throw new Error('合成保存失败，文字与词典均未保存。');
    if(payload.expectedCurrentRevisionId!==o.id)throw new Error('Synthetic stale correction');
    const block=o.blocks.find(block=>block.id===p.blockId),scalars=Array.from(block.text);
    if(scalars.slice(p.start,p.end).join('')!==p.expectedText)throw new Error('Synthetic stale selection');
    if(payload.remember&&payload.expectedDictionaryVersion!==fixture.snapshot.dictionary.version)throw new Error('Synthetic stale dictionary');
    const previous=copy(o),dictionary=fixture.snapshot.dictionary;
    let status='notRequested';
    if(payload.remember) {
     const existing=dictionary.entries.some(entry=>entry.term.normalize('NFKC').toLowerCase()===p.replacement.normalize('NFKC').toLowerCase());
     status=existing?'existing':'added';
     if(!existing){dictionary.version++;dictionary.entries.push({id:'synthetic-word',term:p.replacement,createdAt:'2026-10-08T04:00:00Z',updatedAt:'2026-10-08T04:00:00Z'});}
    }
    block.text=scalars.slice(0,p.start).join('')+p.replacement+scalars.slice(p.end).join('');
    o.id='synthetic-output-r'+(++revision);o.parentRevisionId=previous.id;o.kind='edit';d.canUndo=true;
    d.session.currentOutputId=o.id;d.session.undoTargetId=previous.id;
    return {detail:copy(d),dictionary:payload.remember?copy(dictionary):null,dictionaryStatus:status};
   }
"""


def select_word(page, area, start, end):
    """Physical browser pointer drag at observed textarea glyph positions."""
    area.scroll_into_view_if_needed()
    positions = area.evaluate('''(el,range)=>{
      const style=getComputedStyle(el),canvas=document.createElement('canvas'),ctx=canvas.getContext('2d');ctx.font=style.font;
      const box=el.getBoundingClientRect(),lineHeight=parseFloat(style.lineHeight)||parseFloat(style.fontSize)*1.2;
      return range.map(offset=>({x:box.left+parseFloat(style.paddingLeft)+ctx.measureText(el.value.slice(0,offset)).width+0.5,y:box.top+parseFloat(style.paddingTop)+lineHeight/2}));
    }''', [start, end])
    page.mouse.move(**positions[0])
    page.mouse.down()
    page.mouse.move(**positions[1], steps=8)
    page.mouse.up()
    expect(area).to_have_js_property('selectionStart', start)
    expect(area).to_have_js_property('selectionEnd', end)


def overflow(page):
    return page.evaluate('''()=>({viewport:innerWidth,width:document.documentElement.scrollWidth,offscreen:[...document.querySelectorAll('.history-term-correction button,.history-term-correction input,.mode-preference-options button,.mode-definition textarea')].filter(el=>{const b=el.getBoundingClientRect();return b.width>0&&(b.left < -1 || b.right>innerWidth+1);}).map(el=>({text:el.textContent,box:el.getBoundingClientRect().toJSON()}))})''')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--inspect-only', action='store_true')
    args=parser.parse_args()
    if urlparse(args.url).hostname!='127.0.0.1' or urlparse(args.url).scheme!='http' or not args.out.is_absolute():
        parser.error('Use an independent loopback URL and a new absolute evidence directory')
    args.out.mkdir(parents=True,exist_ok=False)
    rows,errors,external=[],[],[]
    receipt={'scope':__doc__,'cases':rows,'pageErrors':errors,'externalRequests':external}
    with qa.managed_preview(args.url,args.out,True) as server,sync_playwright() as p:
        receipt['server']=server
        browser=p.chromium.launch(channel='msedge',headless=True)
        receipt['browserVersion']=browser.version
        try:
            for name in ['correction-and-undo','correction-keyboard','correction-cancel','correction-error','memory-correction','expression-preferences']:
                context=browser.new_context(viewport={'width':1220,'height':900},locale='zh-CN')
                data=qa.fixture()
                data['detail']['output']['blocks'][0]['text']='🙂飞格玛和飞格玛。其他文字保持。'
                if name=='memory-correction':
                    data['detail']['session']['storagePolicy']='memoryOnly'
                bridge=qa.BRIDGE.replace("   if(name==='state.snapshot')",EXTENSION+"   if(name==='state.snapshot')")
                context.add_init_script(bridge.replace('__FIXTURE__',json.dumps(data,ensure_ascii=False)))
                def route(r):
                    if urlparse(r.request.url).hostname not in ('127.0.0.1',None):external.append(r.request.url);r.abort()
                    else:r.continue_()
                context.route('**/*',route)
                page=context.new_page()
                page.on('pageerror',lambda error:errors.append(str(error)))
                row={'name':name,'status':'RUNNING'}
                rows.append(row)
                try:
                    page.goto(args.url,wait_until='networkidle')
                    if name=='expression-preferences':
                        page.get_by_role('button',name='管理 Mode',exact=True).click()
                        page.get_by_role('button',name='新建 Mode',exact=True).click()
                        expect(page.get_by_role('group',name='表达偏好',exact=True)).to_be_visible()
                        qa.write_new(args.out/(name+'-dom.txt'),page.locator('body').inner_text())
                        page.screenshot(path=str(args.out/(name+'-initial.png')),full_page=True)
                        if not args.inspect_only:
                            page.get_by_role('textbox',name='名称',exact=True).fill('Synthetic 我的表达')
                            rules=page.get_by_role('textbox',name='整理要求',exact=True)
                            custom='只在有明确时间时列出日期，保留所有否定。'
                            rules.fill(custom)
                            chips=['保留英文术语','简洁自然','简体中文','少些客套']
                            for label in chips:
                                chip=page.get_by_role('button',name=label,exact=True)
                                expect(chip).to_have_attribute('aria-pressed','false');chip.click();expect(chip).to_have_attribute('aria-pressed','true')
                            expect(rules).to_have_value(custom+'\n保留英文名称、产品名与缩写的原始拼写，不翻译成中文。\n减少冗余措辞，使用自然简洁的表达，保留全部实质信息。\n中文部分使用简体中文，保留原有英文内容与专有名词。\n直接表达原意，不额外添加客套话、承诺或结论。')
                            page.get_by_role('button',name='少些客套',exact=True).click()
                            expect(page.get_by_role('button',name='少些客套',exact=True)).to_have_attribute('aria-pressed','false')
                            saved_rules=rules.input_value();assert saved_rules.startswith(custom+'\n') and '客套话' not in saved_rules
                            row['layouts']=[]
                            for width in [800,430]:
                                page.set_viewport_size({'width':width,'height':900});metrics=overflow(page);assert metrics['width']<=width+1 and not metrics['offscreen'],metrics;row['layouts'].append(metrics)
                            page.screenshot(path=str(args.out/(name+'-narrow.png')),full_page=True)
                            page.set_viewport_size({'width':1220,'height':900})
                            page.get_by_role('button',name='保存 Mode',exact=True).click()
                            expect(page.get_by_role('heading',name='Mode 管理',exact=True)).to_be_visible()
                            page.locator('.personal-mode-row').filter(has_text='Synthetic 我的表达').get_by_role('button',name='编辑',exact=True).click()
                            expect(page.get_by_role('textbox',name='整理要求',exact=True)).to_have_value(saved_rules)
                            for label in chips[:3]:expect(page.get_by_role('button',name=label,exact=True)).to_have_attribute('aria-pressed','true')
                            expect(page.get_by_role('button',name='少些客套',exact=True)).to_have_attribute('aria-pressed','false')
                            calls=[call for call in qa.command_ledger(page) if call['name']=='mode.save'];assert len(calls)==1 and calls[0]['payload']['draft']['rules']==saved_rules
                            row['savedRules']=saved_rules
                    else:
                        page.get_by_role('button',name='查看与调整',exact=True).click()
                        page.get_by_role('button',name='纠正词条',exact=True).click()
                        panel=page.get_by_role('region',name='纠正一个词',exact=True)
                        expect(panel).to_be_visible()
                        qa.write_new(args.out/(name+'-dom.txt'),page.locator('body').inner_text())
                        page.screenshot(path=str(args.out/(name+'-initial.png')),full_page=True)
                        if not args.inspect_only:
                            expect(page.get_by_role('checkbox',name='记住这个写法',exact=False)).not_to_be_checked()
                            expect(page.get_by_role('button',name='保存这一处',exact=True)).to_be_disabled()
                            area=page.get_by_role('textbox',name='划选当前文本中要纠正的词',exact=True)
                            if name=='correction-keyboard':
                                area.focus();area.press('Control+Home');area.press('ArrowRight')
                                area.press('Shift+ArrowRight');area.press('Shift+ArrowRight');area.press('Shift+ArrowRight')
                                expect(area).to_have_js_property('selectionStart',2);expect(area).to_have_js_property('selectionEnd',5)
                            else:select_word(page,area,2,5)
                            page.get_by_role('textbox',name='正确写法',exact=True).fill('Figma')
                            expect(page.locator('.history-correction-preview')).to_contain_text('🙂Figma和飞格玛。其他文字保持。')
                            page.get_by_role('checkbox',name='记住这个写法',exact=False).check()
                            expect(panel).to_contain_text('撤销本次文本修改不会删除词条')
                            if name=='memory-correction':expect(panel).to_contain_text('本次正文不保存')
                            if name=='correction-cancel':
                                page.get_by_role('button',name='取消纠正',exact=True).click();expect(panel).to_have_count(0)
                                assert not any(call['name']=='revision.correctTerm' for call in qa.command_ledger(page))
                            elif name=='correction-error':
                                page.evaluate('window.__speakmodeQA.failCorrection=true')
                                page.get_by_role('button',name='纠正并记住',exact=True).click()
                                expect(panel.get_by_role('alert')).to_have_text('合成保存失败，文字与词典均未保存。')
                                expect(page.get_by_role('textbox',name='正确写法',exact=True)).to_have_value('Figma')
                                expect(page.locator('.history-current')).to_contain_text('🙂飞格玛和飞格玛。其他文字保持。')
                            else:
                                page.set_viewport_size({'width':430,'height':900});metrics=overflow(page);assert metrics['width']<=431 and not metrics['offscreen'],metrics;row['layout']=metrics
                                page.screenshot(path=str(args.out/(name+'-narrow.png')),full_page=True)
                                page.set_viewport_size({'width':1220,'height':900})
                                page.get_by_role('button',name='纠正并记住',exact=True).click();expect(panel).to_have_count(0)
                                expect(page.locator('.history-current')).to_contain_text('🙂Figma和飞格玛。其他文字保持。')
                                calls=[call for call in qa.command_ledger(page) if call['name']=='revision.correctTerm'];assert len(calls)==1
                                patch=calls[0]['payload']['patch'];assert patch=={'blockId':'synthetic-block-1','start':1,'end':4,'expectedText':'飞格玛','replacement':'Figma'},patch
                                row['correctionPayload']=calls[0]['payload']
                                page.get_by_role('button',name='撤销上次修改',exact=True).click()
                                expect(page.locator('.history-current')).to_contain_text('🙂飞格玛和飞格玛。其他文字保持。')
                                page.get_by_role('button',name='纠正词条',exact=True).click()
                                expect(page.locator('.history-correction-remember')).to_contain_text('已有 1 / 200 词')
                    row['status']='INSPECTED' if args.inspect_only else 'PASS'
                    page.screenshot(path=str(args.out/(name+'-final.png')),full_page=True)
                    assert not any(call['name'].startswith(('credential.','session.')) or call['name'] in ('mode.preview','result.copy') for call in qa.command_ledger(page))
                except Exception as error:
                    row.update({'status':'FAIL','error':str(error)})
                    page.screenshot(path=str(args.out/(name+'-failed.png')),full_page=True)
                finally:context.close()
        finally:browser.close()
    receipt['status']='PASS' if all(row['status'] in ('PASS','INSPECTED') for row in rows) and not errors and not external else 'FAIL'
    qa.write_new(args.out/'receipt.json',receipt,json_value=True)
    print(json.dumps({'status':receipt['status'],'cases':rows,'pageErrors':errors},ensure_ascii=True))
    return 0 if receipt['status']=='PASS' else 1


if __name__=='__main__':sys.exit(main())
