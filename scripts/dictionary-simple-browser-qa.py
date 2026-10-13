"""Simple dictionary production UI in isolated headless Edge with synthetic IPC.

Only frontend layout, focus and requests are evaluated. No real dictionary,
native storage, microphone, cloud, clipboard or personal profile is accessed.
The fixed store is deliberately not an implementation of native Unicode folding.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
from urllib.parse import urlparse
from playwright.sync_api import sync_playwright, expect

spec = importlib.util.spec_from_file_location('dictionary_capacity', Path(__file__).with_name('dictionary-capacity-browser-qa.py'))
capacity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capacity)
qa = capacity.qa

LIST_EXTENSION = r"""
   if(name==='dictionary.list') {
    if(window.__speakmodeQA.deferDictionaryList)await new Promise(resolve=>{window.__speakmodeQA.releaseDictionaryList=resolve;});
    if(window.__speakmodeQA.failDictionaryList)throw new Error('合成词库读取失败，请重新读取。');
    return copy(fixture.syntheticDictionary);
   }
   if(name==='dictionary.delete') {
    const current=fixture.snapshot.dictionary;
    if(window.__speakmodeQA.forceDeleteConflict){
     window.__speakmodeQA.forceDeleteConflict=false;current.entries=current.entries.filter(entry=>entry.id!==payload.entryId);current.version++;fixture.syntheticDictionary=current;
     throw {code:'revisionConflict',message:'合成删除版本冲突'};
    }
    if(payload.expectedVersion!==current.version)throw {code:'revisionConflict',message:'合成词库版本冲突'};
    const next=copy(current);next.entries=next.entries.filter(entry=>entry.id!==payload.entryId);next.version++;
    fixture.snapshot.dictionary=next;fixture.syntheticDictionary=next;return copy(next);
   }
"""


def make_fixture(terms, state='ready'):
    data=qa.fixture()
    dictionary={'version':1,'entries':[{'id':str(i),'term':term,'createdAt':'2026-10-09','updatedAt':'2026-10-09'} for i,term in enumerate(terms)]}
    data['syntheticDictionary']=dictionary
    data['snapshot']['dictionary']=dictionary if state=='ready' else None
    return data


def assert_layout(page, width, *, grid=False):
    page.set_viewport_size({'width':width,'height':900})
    result=page.evaluate('''()=>{
      const shown=el=>{const r=el.getBoundingClientRect();return r.width>0&&r.height>0&&getComputedStyle(el).visibility!=='hidden';};
      const selector='.dictionary-page button,.dictionary-page input,.dictionary-page li,[role="dialog"] input,[role="dialog"] textarea,[role="dialog"] button,[role="dialog"] summary,[role="alertdialog"] button';
      const offscreen=[...document.querySelectorAll(selector)].filter(shown).map(el=>({label:el.getAttribute('aria-label')||el.textContent,box:el.getBoundingClientRect().toJSON()})).filter(item=>item.box.left < -1 || item.box.right>innerWidth+1);
      const list=document.querySelector('.dictionary-list'),scope=document.querySelector('.dictionary-page');
      return {viewport:innerWidth,documentWidth:document.documentElement.scrollWidth,offscreen,contentWidth:scope?.getBoundingClientRect().width,columns:list?getComputedStyle(list).gridTemplateColumns.split(' ').length:0};
    }''')
    assert result['documentWidth']<=width+1 and not result['offscreen'],result
    if grid:
        expected=3 if result['contentWidth']>=960 else 2 if result['contentWidth']>=580 else 1
        assert result['columns']==expected,result
    return result


def commands_are_scoped(page):
    ledger=qa.command_ledger(page)
    assert not any(item['name'].startswith(('credential.','session.')) or item['name'] in ('result.copy','mode.preview') for item in ledger),ledger
    return ledger


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url',required=True)
    parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--inspect-only',action='store_true')
    args=parser.parse_args()
    parsed=urlparse(args.url)
    if parsed.scheme!='http' or parsed.hostname!='127.0.0.1' or not args.out.is_absolute():
        parser.error('Use a dedicated loopback URL and a fresh absolute evidence directory')
    args.out.mkdir(parents=True,exist_ok=False)
    results,errors,external=[],[],[]
    receipt={'scope':__doc__,'cases':results,'pageErrors':errors,'externalRequests':external,
             'sourceSha256':{name:hashlib.sha256((qa.ROOT/name).read_bytes()).hexdigest() for name in ['app/src/pages/DictionaryPage.tsx','app/src/pages/dictionary.css','app/src/components/NavigationGuard.tsx','app/src/styles.css']},
             'distSha256':{str(path.relative_to(qa.ROOT)):hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted((qa.ROOT/'app/dist').rglob('*')) if path.is_file()}}
    cases=['layout-search','add-edit-delete','delete-conflict','dirty-dialog','save-and-switch','keyboard-focus','bulk-failure','loading','read-failure','empty']
    with qa.managed_preview(args.url,args.out,True) as server,sync_playwright() as playwright:
        receipt['server']=server
        browser=playwright.chromium.launch(channel='msedge',headless=True)
        receipt['browserVersion']=browser.version
        try:
            for name in cases:
                terms=['SpeakMode','PolyU','人机交互','长词'*40,'VeryLongUnbrokenProjectIdentifier'*2,'C++'] if name=='layout-search' else [] if name in ['empty','loading','read-failure'] else ['SpeakMode','PolyU']
                state=name if name in ['loading','read-failure'] else 'ready'
                data=make_fixture(terms,state)
                extension=capacity.EXTENSION.replace("   if(name==='dictionary.list')return copy(fixture.snapshot.dictionary);",LIST_EXTENSION)
                bridge=qa.BRIDGE.replace("   if(name==='state.snapshot')",extension+"   if(name==='state.snapshot')")
                context=browser.new_context(viewport={'width':1440,'height':900},locale='zh-CN')
                flags='\nwindow.__speakmodeQA.deferDictionaryList=true;' if state=='loading' else '\nwindow.__speakmodeQA.failDictionaryList=true;' if state=='read-failure' else ''
                context.add_init_script(bridge.replace('__FIXTURE__',json.dumps(data,ensure_ascii=False))+flags)
                def route(request_route):
                    if urlparse(request_route.request.url).hostname not in ('127.0.0.1',None):external.append(request_route.request.url);request_route.abort()
                    else:request_route.continue_()
                context.route('**/*',route)
                page=context.new_page();page.on('pageerror',lambda error:errors.append(str(error)))
                row={'name':name,'status':'RUNNING'};results.append(row)
                try:
                    page.goto(args.url,wait_until='networkidle')
                    page.get_by_role('button',name='词典',exact=True).click()
                    expect(page.get_by_role('heading',name='个人词典',exact=True)).to_be_visible()
                    qa.write_new(args.out/(name+'-initial-dom.txt'),page.locator('.dictionary-page').inner_text())
                    page.screenshot(path=str(args.out/(name+'-initial.png')))
                    if not args.inspect_only:
                        if name=='layout-search':
                            expect(page.locator('.dictionary-list > li')).to_have_count(len(terms))
                            row['layouts']=[]
                            for width in [1560,1220,800,430]:
                                row['layouts'].append(assert_layout(page,width,grid=True));page.screenshot(path=str(args.out/f'grid-{width}.png'))
                            search=page.get_by_role('textbox',name='搜索词条',exact=True)
                            search.fill('ＳＰＥＡＫＭＯＤＥ');expect(page.locator('.dictionary-list > li')).to_have_count(1)
                            search.fill('does-not-exist');expect(page.get_by_text('没有找到这个词条',exact=True)).to_be_visible()
                            page.locator('.dictionary-search').get_by_role('button',name='清除搜索',exact=True).click();expect(search).to_be_focused();expect(page.locator('.dictionary-list > li')).to_have_count(len(terms))
                        elif name=='add-edit-delete':
                            page.get_by_role('button',name='新词',exact=True).click()
                            dialog=page.get_by_role('dialog',name='添加到词典',exact=True)
                            field=dialog.get_by_role('textbox',name='词条',exact=True);expect(field).to_be_focused();field.fill('NewTerm');field.press('Enter')
                            expect(page.get_by_role('dialog')).to_have_count(0);expect(page.get_by_role('button',name='新词',exact=True)).to_be_focused()
                            edit=page.get_by_role('button',name='编辑词条 NewTerm',exact=True);expect(edit).to_be_visible();edit.click()
                            field=page.get_by_role('textbox',name='词条',exact=True);field.fill('RenamedTerm');page.get_by_role('button',name='保存词条',exact=True).click()
                            expect(page.get_by_role('button',name='编辑词条 RenamedTerm',exact=True)).to_be_focused()
                            menu=page.get_by_role('button',name='词条操作 RenamedTerm',exact=True);menu.click();page.get_by_role('menuitem',name='删除词条',exact=True).click()
                            confirmation=page.get_by_role('alertdialog');expect(confirmation).to_contain_text('RenamedTerm');confirmation.get_by_role('button',name='保留',exact=True).click();expect(menu).to_be_focused()
                            menu.click();page.get_by_role('menuitem',name='删除词条',exact=True).click();page.get_by_role('alertdialog').get_by_role('button',name='删除词条',exact=True).click()
                            expect(page.get_by_role('button',name='编辑词条 RenamedTerm',exact=True)).to_have_count(0)
                            writes=[entry for entry in qa.command_ledger(page) if entry['name'].startswith('dictionary.') and entry['name']!='dictionary.list']
                            assert [entry['name'] for entry in writes]==['dictionary.save','dictionary.save','dictionary.delete'],writes
                            assert [entry['payload']['expectedVersion'] for entry in writes]==[1,2,3],writes
                        elif name=='delete-conflict':
                            page.get_by_role('button',name='词条操作 SpeakMode',exact=True).click();page.get_by_role('menuitem',name='删除词条',exact=True).click()
                            page.evaluate('window.__speakmodeQA.forceDeleteConflict=true')
                            page.get_by_role('alertdialog').get_by_role('button',name='删除词条',exact=True).click()
                            expect(page.get_by_role('alertdialog')).to_have_count(0);expect(page.get_by_role('button',name='编辑词条 PolyU',exact=True)).to_be_focused()
                            expect(page.get_by_text('此词条已不在当前词库中。',exact=True)).to_be_visible()
                            writes=[entry for entry in qa.command_ledger(page) if entry['name']=='dictionary.delete'];assert len(writes)==1 and writes[0]['payload']['expectedVersion']==1
                        elif name=='dirty-dialog':
                            page.get_by_role('button',name='新词',exact=True).click();field=page.get_by_role('textbox',name='词条',exact=True);field.fill('UnsavedTerm')
                            page.get_by_role('dialog').get_by_role('button',name='取消',exact=True).click();guard=page.get_by_role('alertdialog',name='保存这次修改？',exact=True)
                            expect(guard).to_be_visible();guard.get_by_role('button',name='继续编辑',exact=True).click();expect(field).to_have_value('UnsavedTerm')
                            field.press('Escape');expect(guard).to_be_visible();guard.get_by_role('button',name='放弃修改',exact=True).click()
                            expect(page.get_by_role('dialog')).to_have_count(0);expect(page.get_by_role('button',name='新词',exact=True)).to_be_focused()
                            assert not any(entry['name']=='dictionary.save' for entry in qa.command_ledger(page))
                        elif name=='save-and-switch':
                            page.get_by_role('button',name='新词',exact=True).click();field=page.get_by_role('textbox',name='词条',exact=True);field.fill('FirstNewTerm')
                            page.get_by_role('button',name='批量添加',exact=True).click();guard=page.get_by_role('alertdialog',name='保存这次修改？',exact=True)
                            expect(guard).to_be_visible();guard.get_by_role('button',name='保存并离开',exact=True).click()
                            bulk=page.get_by_role('textbox',name='每行一个词条',exact=True);expect(bulk).to_be_visible();bulk.fill('SecondNewTerm')
                            page.get_by_role('dialog').get_by_role('button',name='添加词条',exact=True).click();expect(page.get_by_role('dialog')).to_have_count(0)
                            expect(page.get_by_role('button',name='编辑词条 FirstNewTerm',exact=True)).to_be_visible();expect(page.get_by_role('button',name='编辑词条 SecondNewTerm',exact=True)).to_be_visible()
                            writes=[entry for entry in qa.command_ledger(page) if entry['name'] in ('dictionary.save','dictionary.import')]
                            assert [entry['name'] for entry in writes]==['dictionary.save','dictionary.import'],writes
                            assert [entry['payload']['expectedVersion'] for entry in writes]==[1,2],writes
                        elif name=='keyboard-focus':
                            trigger=page.get_by_role('button',name='新词',exact=True);trigger.click();field=page.get_by_role('textbox',name='词条',exact=True);expect(field).to_be_focused()
                            page.evaluate("async()=>await Promise.all(document.getAnimations().filter(a=>a.effect?.getTiming().iterations!==Infinity).map(a=>a.finished.catch(()=>{})))")
                            row['overlay']=page.evaluate("""()=>{const dialog=document.querySelector('[role=dialog]'),overlay=dialog.closest('.rt-BaseDialogOverlay');return {dialogOpacity:getComputedStyle(dialog).opacity,dialogBackground:getComputedStyle(dialog).backgroundColor,overlayBackground:getComputedStyle(overlay).backgroundColor,shade:getComputedStyle(overlay,'::before').backgroundColor};}""")
                            assert row['overlay']['dialogOpacity']=='1' and row['overlay']['overlayBackground']=='rgba(0, 0, 0, 0)',row['overlay']
                            page.screenshot(path=str(args.out/'new-word-1440.png'),animations='disabled')
                            for key in ['Tab']*12+['Shift+Tab']*12:
                                page.keyboard.press(key)
                                assert page.evaluate("Boolean(document.activeElement?.closest('[role=dialog]'))"),page.evaluate('document.activeElement?.outerHTML')
                            row['layouts']=[assert_layout(page,width) for width in [1440,800,430]]
                            page.screenshot(path=str(args.out/'new-word-430.png'),animations='disabled')
                            page.keyboard.press('Escape');expect(page.get_by_role('dialog')).to_have_count(0);expect(trigger).to_be_focused()
                        elif name=='bulk-failure':
                            page.get_by_role('button',name='新词',exact=True).click();page.get_by_role('button',name='批量添加',exact=True).click()
                            draft='\n'.join('术'*70+'-'+str(i) for i in range(20));field=page.get_by_role('textbox',name='每行一个词条',exact=True);field.fill(draft)
                            submit=page.get_by_role('dialog').get_by_role('button',name='添加词条',exact=True);expect(submit).to_be_enabled();submit.click()
                            expect(page.get_by_role('dialog').locator('.dictionary-error')).to_contain_text('整批未保存');expect(field).to_have_value(draft)
                            row['layouts']=[assert_layout(page,width) for width in [1440,800,430]]
                            writes=[entry for entry in qa.command_ledger(page) if entry['name']=='dictionary.import'];assert len(writes)==1 and writes[0]['payload']['terms']==draft.splitlines()
                        elif name=='loading':
                            expect(page.get_by_text('正在读取词库…',exact=True)).to_be_visible();expect(page.get_by_role('button',name='新词',exact=True)).to_be_disabled()
                            expect(page.get_by_text('从一个常用词开始',exact=True)).to_have_count(0)
                            page.evaluate('window.__speakmodeQA.deferDictionaryList=false;window.__speakmodeQA.releaseDictionaryList()')
                            expect(page.get_by_text('从一个常用词开始',exact=True)).to_be_visible();expect(page.get_by_role('button',name='新词',exact=True)).to_be_enabled()
                        elif name=='read-failure':
                            expect(page.get_by_text('合成词库读取失败，请重新读取。',exact=True)).to_be_visible();expect(page.get_by_role('button',name='新词',exact=True)).to_be_disabled()
                            expect(page.get_by_text('从一个常用词开始',exact=True)).to_have_count(0)
                            page.evaluate('window.__speakmodeQA.failDictionaryList=false');page.get_by_role('button',name='重新读取',exact=True).click()
                            expect(page.get_by_role('button',name='新词',exact=True)).to_be_enabled();expect(page.get_by_text('从一个常用词开始',exact=True)).to_be_visible()
                        else:
                            expect(page.get_by_text('从一个常用词开始',exact=True)).to_be_visible();expect(page.locator('.dictionary-list > li')).to_have_count(0)
                            expect(page.get_by_role('button',name='新词',exact=True)).to_be_enabled()
                        row['commands']=commands_are_scoped(page)
                    row['status']='INSPECTED' if args.inspect_only else 'PASS'
                    page.screenshot(path=str(args.out/(name+'-final.png')))
                except Exception as error:
                    row.update({'status':'FAIL','error':str(error)});page.screenshot(path=str(args.out/(name+'-failed.png')))
                finally:context.close()
        finally:browser.close()
    receipt['status']='PASS' if all(row['status'] in ('PASS','INSPECTED') for row in results) and not errors and not external else 'FAIL'
    qa.write_new(args.out/'receipt.json',receipt,json_value=True)
    print(json.dumps({'status':receipt['status'],'cases':[{'name':row['name'],'status':row['status'],'error':row.get('error')} for row in results],'pageErrors':errors},ensure_ascii=True))
    return 0 if receipt['status']=='PASS' else 1


if __name__=='__main__':sys.exit(main())
