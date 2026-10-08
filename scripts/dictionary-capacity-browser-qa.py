"""Production dictionary UI against a synthetic bridge in an isolated headless browser.

The synthetic Store verifies frontend payloads and displays controlled responses;
it is not native Unicode casefold, storage, ASR, microphone or provider evidence.
No real user data or credentials are accessed; external requests are blocked.
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
   if(name==='dictionary.list')return copy(fixture.snapshot.dictionary);
   if(name==='dictionary.save'||name==='dictionary.import') {
    const current=fixture.snapshot.dictionary;
    if(window.__speakmodeQA.forceDictionaryConflict){
     window.__speakmodeQA.forceDictionaryConflict=false;
     current.version++;current.entries.push({id:'concurrent',term:'新词',createdAt:'2026-10-09',updatedAt:'2026-10-09'});
     throw {code:'revisionConflict',message:'合成词库版本冲突'};
    }
    if(payload.expectedVersion!==current.version)throw {code:'revisionConflict',message:'合成词库版本冲突'};
    const next=copy(current);
    if(name==='dictionary.import'){
     for(const term of payload.terms){
      // One explicitly frozen Unicode-equivalence example, not a casefold implementation.
      const duplicate=next.entries.some(entry=>entry.term===term||(entry.term==='Straße'&&term==='STRASSE'));
      if(!duplicate)next.entries.push({id:'import-'+next.entries.length,term,createdAt:'2026-10-09',updatedAt:'2026-10-09'});
     }
    } else {
     const draft=payload.draft,entry=next.entries.find(entry=>entry.id===draft.id);
     if(draft.id&&!entry)throw new Error('Synthetic missing dictionary entry');
     if(entry)entry.term=draft.term;
     else next.entries.push({id:'added-'+next.entries.length,term:draft.term,createdAt:'2026-10-09',updatedAt:'2026-10-09'});
    }
    const bytes=next.entries.length?new TextEncoder().encode('常用词汇：'+JSON.stringify(next.entries.map(entry=>entry.term))).length:0;
    if(bytes>4000||next.entries.length>200)throw new Error('合成词库超出容量限制，整批未保存。');
    next.version++;fixture.snapshot.dictionary=next;return copy(next);
   }
"""


def bytes_for(terms):
    return len(('常用词汇：'+json.dumps(terms, ensure_ascii=False, separators=(',', ':'))).encode('utf8')) if terms else 0


def layouts(page, out, name):
    rows=[]
    for width in [720,800,430]:
        page.set_viewport_size({'width':width,'height':900})
        metrics=page.evaluate('''()=>({viewport:innerWidth,width:document.documentElement.scrollWidth,offscreen:[...document.querySelectorAll('.dictionary-page button,.dictionary-page input,.dictionary-page textarea,.dictionary-capacity meter,.dictionary-capacity-label')].filter(el=>{const b=el.getBoundingClientRect();return b.width>0&&(b.left < -1 || b.right>innerWidth+1);}).map(el=>({text:el.textContent,box:el.getBoundingClientRect().toJSON()}))})''')
        assert metrics['width']<=width+1 and not metrics['offscreen'],metrics
        rows.append(metrics)
    page.screenshot(path=str(out/(name+'-430.png')))
    page.set_viewport_size({'width':1220,'height':900})
    return rows


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url',required=True)
    parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--inspect-only',action='store_true')
    args=parser.parse_args()
    if urlparse(args.url).hostname!='127.0.0.1' or urlparse(args.url).scheme!='http' or not args.out.is_absolute():
        parser.error('Use a dedicated loopback URL and a new absolute evidence directory')
    args.out.mkdir(parents=True,exist_ok=False)
    rows,errors,external=[],[],[]
    receipt={'scope':__doc__,'cases':rows,'pageErrors':errors,'externalRequests':external}
    with qa.managed_preview(args.url,args.out,True) as server,sync_playwright() as p:
        receipt['server']=server
        browser=p.chromium.launch(channel='msedge',headless=True)
        receipt['browserVersion']=browser.version
        try:
            for name in ['add-edit','bulk-equivalent','oversized-import','edit-boundary','stale-edit']:
                terms={'add-edit':['A','术语','😀','"\\'],'bulk-equivalent':['Straße'],'oversized-import':[],
                       'edit-boundary':['x'*78+f'{i:02}' for i in range(48)],'stale-edit':['A']}[name]
                data=qa.fixture()
                data['snapshot']['dictionary']['entries']=[{'id':str(i),'term':term,'createdAt':'2026-10-09','updatedAt':'2026-10-09'} for i,term in enumerate(terms)]
                bridge=qa.BRIDGE.replace("   if(name==='state.snapshot')",EXTENSION+"   if(name==='state.snapshot')")
                context=browser.new_context(viewport={'width':1220,'height':900},locale='zh-CN')
                context.add_init_script(bridge.replace('__FIXTURE__',json.dumps(data,ensure_ascii=False)))
                def route(r):
                    if urlparse(r.request.url).hostname not in ('127.0.0.1',None):external.append(r.request.url);r.abort()
                    else:r.continue_()
                context.route('**/*',route)
                page=context.new_page();page.on('pageerror',lambda error:errors.append(str(error)))
                row={'name':name,'status':'RUNNING'};rows.append(row)
                try:
                    page.goto(args.url,wait_until='networkidle')
                    page.get_by_role('button',name='词典',exact=True).click()
                    expect(page.get_by_role('heading',name='个人词典',exact=True)).to_be_visible()
                    expect(page.get_by_role('meter',name='词库容量')).to_have_attribute('value',str(bytes_for(terms)))
                    qa.write_new(args.out/(name+'-initial-dom.txt'),page.locator('.dictionary-page').inner_text())
                    page.screenshot(path=str(args.out/(name+'-initial.png')))
                    if not args.inspect_only:
                        if name=='add-edit':
                            page.get_by_role('button',name='添加词条',exact=True).click();page.get_by_role('textbox',name='词条',exact=True).fill('SpeakMode')
                            expect(page.locator('#dictionary-capacity-help')).to_contain_text('草稿容量上限 55 / 4000 字节')
                            page.get_by_role('button',name='保存词条',exact=True).click()
                            expect(page.get_by_role('meter',name='词库容量')).to_have_attribute('value','55')
                            page.get_by_role('button',name='编辑词条 SpeakMode',exact=True).click();page.get_by_role('textbox',name='词条',exact=True).fill('SM')
                            expect(page.locator('#dictionary-capacity-help')).to_contain_text('草稿容量上限 48 / 4000 字节')
                            row['layouts']=layouts(page,args.out,name);page.get_by_role('button',name='保存词条',exact=True).click()
                            expect(page.get_by_role('meter',name='词库容量')).to_have_attribute('value','48')
                            writes=[c for c in qa.command_ledger(page) if c['name']=='dictionary.save'];assert len(writes)==2 and writes[1]['payload']['draft']['id']=='added-4'
                        elif name in ('bulk-equivalent','oversized-import'):
                            page.get_by_role('button',name='批量添加',exact=True).click()
                            draft=['STRASSE','PolyU'] if name=='bulk-equivalent' else ['术'*70+'-'+str(i) for i in range(20)]
                            field=page.get_by_role('textbox',name='每行一个词条',exact=True);field.fill('\n'.join(draft))
                            expect(page.locator('#dictionary-capacity-help')).to_contain_text(f'草稿容量上限 {bytes_for(terms+draft)} / 4000 字节')
                            expect(page.locator('#dictionary-capacity-help')).to_contain_text('重复词尚未合并')
                            row['layouts']=layouts(page,args.out,name)
                            submit=page.locator('.dictionary-editor').get_by_role('button',name='添加词条',exact=True);expect(submit).to_be_enabled();submit.click()
                            if name=='bulk-equivalent':
                                expect(page.locator('.dictionary-notice')).to_contain_text('已添加 1 个词条，跳过 1 个重复词条')
                                expect(page.get_by_role('meter',name='词库容量')).to_have_attribute('value','34')
                            else:
                                expect(page.locator('.dictionary-error')).to_contain_text('合成词库超出容量限制，整批未保存。')
                                expect(field).to_have_value('\n'.join(draft));expect(page.get_by_role('meter',name='词库容量')).to_have_attribute('value','0')
                            writes=[c for c in qa.command_ledger(page) if c['name']=='dictionary.import'];assert len(writes)==1 and writes[0]['payload']['terms']==draft
                        elif name=='edit-boundary':
                            page.get_by_role('button',name='编辑词条 '+terms[-1],exact=True).click();field=page.get_by_role('textbox',name='词条',exact=True);field.fill('x'*78+'4é')
                            expect(page.locator('#dictionary-capacity-help')).to_contain_text('可能超出 1 字节');row['layouts']=layouts(page,args.out,name)
                            page.get_by_role('button',name='保存词条',exact=True).click();expect(page.locator('.dictionary-error')).to_contain_text('整批未保存')
                            expect(field).to_have_value('x'*78+'4é');expect(page.get_by_role('meter',name='词库容量')).to_have_attribute('value','4000')
                            field.fill('Short');expect(page.locator('#dictionary-capacity-help')).to_contain_text('至少剩余 75 字节');page.get_by_role('button',name='保存词条',exact=True).click()
                            expect(page.get_by_role('meter',name='词库容量')).to_have_attribute('value','3925')
                        else:
                            page.get_by_role('button',name='编辑词条 A',exact=True).click();field=page.get_by_role('textbox',name='词条',exact=True);field.fill('B')
                            page.evaluate('window.__speakmodeQA.forceDictionaryConflict=true');page.get_by_role('button',name='保存词条',exact=True).click()
                            expect(page.locator('.dictionary-error')).to_contain_text('草稿仍保留');expect(field).to_have_value('B')
                            expect(page.locator('#dictionary-capacity-help')).to_contain_text('草稿容量上限 29 / 4000 字节');row['layouts']=layouts(page,args.out,name)
                            page.get_by_role('button',name='核对后重试',exact=True).click();expect(page.locator('.dictionary-notice')).to_contain_text('词条已更新')
                            writes=[c for c in qa.command_ledger(page) if c['name']=='dictionary.save'];assert [c['payload']['expectedVersion'] for c in writes]==[1,2]
                        row['commands']=qa.command_ledger(page)
                        assert not any(c['name'].startswith(('credential.','session.')) or c['name'] in ('result.copy','mode.preview') for c in row['commands'])
                    row['status']='INSPECTED' if args.inspect_only else 'PASS'
                    page.screenshot(path=str(args.out/(name+'-final.png')))
                except Exception as error:
                    row.update({'status':'FAIL','error':str(error)});page.screenshot(path=str(args.out/(name+'-failed.png')))
                finally:context.close()
        finally:browser.close()
    receipt['status']='PASS' if all(row['status'] in ('PASS','INSPECTED') for row in rows) and not errors and not external else 'FAIL'
    qa.write_new(args.out/'receipt.json',receipt,json_value=True)
    print(json.dumps({'status':receipt['status'],'cases':[{'name':row['name'],'status':row['status'],'error':row.get('error')} for row in rows],'pageErrors':errors},ensure_ascii=True))
    return 0 if receipt['status']=='PASS' else 1


if __name__=='__main__':sys.exit(main())
