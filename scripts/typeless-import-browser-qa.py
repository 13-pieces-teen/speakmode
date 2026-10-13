"""Production React Typeless import checks with an isolated synthetic IPC bridge.

Checks only UI requests, controlled responses, busy/failure recovery and layout.
Never reads Typeless, a real dictionary/profile, credentials, audio or clipboard.
The synthetic import is deliberately not proof of native discovery, parsing,
Unicode deduplication or transactional storage. External requests are blocked.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
from urllib.parse import urlparse
from playwright.sync_api import sync_playwright, expect

spec = importlib.util.spec_from_file_location('simple_dictionary', Path(__file__).with_name('dictionary-simple-browser-qa.py'))
simple = importlib.util.module_from_spec(spec)
spec.loader.exec_module(simple)
qa = simple.qa

IMPORT_EXTENSION = r"""
   if(name==='dictionary.importTypeless') {
    if(Object.keys(payload).sort().join(',')!=='expectedVersion,requestId')throw new Error('Unexpected import payload');
    if(window.__speakmodeQA.deferImport)await new Promise(resolve=>{window.__speakmodeQA.releaseImport=resolve;});
    const current=fixture.snapshot.dictionary;
    if(window.__speakmodeQA.forceImportConflict){
     window.__speakmodeQA.forceImportConflict=false;
     current.version++;current.entries.push({id:'concurrent',term:'ConcurrentTerm',createdAt:'2026-10-11',updatedAt:'2026-10-11'});
     fixture.syntheticDictionary=current;
     throw {code:'revisionConflict',message:'合成词库版本冲突'};
    }
    if(payload.expectedVersion!==current.version)throw {code:'revisionConflict',message:'合成词库版本冲突'};
    if(window.__speakmodeQA.importError)throw new Error(window.__speakmodeQA.importError);
    if(window.__speakmodeQA.malformedImport)return {version:2,entries:'invalid'};
    const next=copy(current);
    for(const term of fixture.typelessTerms){
     // Exact frozen fixture equality only; native tests own Unicode folding.
     if(!next.entries.some(entry=>entry.term===term))next.entries.push({id:'typeless-'+next.entries.length,term,createdAt:'2026-10-11',updatedAt:'2026-10-11'});
    }
    if(next.entries.length!==current.entries.length)next.version++;
    fixture.snapshot.dictionary=next;fixture.syntheticDictionary=next;return copy(next);
   }
"""


def bridge_for(data, flags):
    extension = simple.capacity.EXTENSION.replace("   if(name==='dictionary.list')return copy(fixture.snapshot.dictionary);", simple.LIST_EXTENSION)
    bridge = qa.BRIDGE.replace("   if(name==='state.snapshot')", IMPORT_EXTENSION + extension + "   if(name==='state.snapshot')")
    return bridge.replace('__FIXTURE__', json.dumps(data, ensure_ascii=False)) + '\nObject.assign(window.__speakmodeQA,' + json.dumps(flags, ensure_ascii=False) + ');'


def import_calls(page):
    return [call for call in qa.command_ledger(page) if call['name'] == 'dictionary.importTypeless']


def visible_terms(page):
    return page.locator('.dictionary-word-text').all_text_contents()


def assert_import_count(page, count):
    assert len(import_calls(page)) == count, import_calls(page)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    parsed = urlparse(args.url)
    if parsed.scheme != 'http' or parsed.hostname != '127.0.0.1' or not args.out.is_absolute():
        parser.error('Use a dedicated loopback URL and a fresh absolute evidence directory')
    args.out.mkdir(parents=True, exist_ok=False)
    rows, errors, external = [], [], []
    receipt = {'scope': __doc__, 'cases': rows, 'pageErrors': errors, 'externalRequests': external,
               'sourceSha256': {name: hashlib.sha256((qa.ROOT / name).read_bytes()).hexdigest() for name in [
                   'app/src/pages/DictionaryPage.tsx', 'app/src/pages/dictionary.css', 'app/src/components/NavigationGuard.tsx',
                   'scripts/typeless-import-browser-qa.py']},
               'distSha256': {str(path.relative_to(qa.ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                              for path in sorted((qa.ROOT / 'app/dist').rglob('*')) if path.is_file()}}
    failure_messages = {
        'missing-source': '未找到可导入的 Typeless 词典，请先打开 Typeless 词典后重试。',
        'source-read-failure': 'Typeless 词典暂时无法读取，请关闭占用后重试。',
        'capacity-failure': '导入后词库超出容量限制，整批未保存。请先清理不常用的词条。',
        'empty-source': 'Typeless 中没有可导入的词汇。',
    }
    cases = ['import-added-repeat', 'pending-doubleclick-navigation', *failure_messages,
             'malformed-response', 'revision-conflict', 'conflict-refresh-failure',
             'keyboard-layout-720', 'keyboard-layout-430', 'desktop-unavailable']
    with qa.managed_preview(args.url, args.out, True) as server, sync_playwright() as playwright:
        receipt['server'] = server
        browser = playwright.chromium.launch(channel='msedge', headless=True)
        receipt['browserVersion'] = browser.version
        try:
            for name in cases:
                terms = ['SpeakMode', 'PolyU']
                if name.startswith('keyboard-layout'):
                    terms += ['LongUnbrokenProjectIdentifier' * 2, '中文专业词' * 12]
                data = simple.make_fixture(terms)
                data['typelessTerms'] = ['SpeakMode', 'Typeless', 'NewTerm', 'Typeless']
                flags = {}
                if name in failure_messages:
                    flags['importError'] = failure_messages[name]
                elif name == 'pending-doubleclick-navigation':
                    flags['deferImport'] = True
                elif name in ['revision-conflict', 'conflict-refresh-failure']:
                    flags['forceImportConflict'] = True
                elif name == 'malformed-response':
                    flags['malformedImport'] = True
                width = int(name.rsplit('-', 1)[1]) if name.startswith('keyboard-layout') else 1440
                context = browser.new_context(viewport={'width': width, 'height': 900}, locale='zh-CN')
                if name != 'desktop-unavailable':
                    context.add_init_script(bridge_for(data, flags))

                def route(request_route):
                    if urlparse(request_route.request.url).hostname not in ('127.0.0.1', None):
                        external.append(request_route.request.url)
                        request_route.abort()
                    else:
                        request_route.continue_()

                context.route('**/*', route)
                page = context.new_page()
                page.on('pageerror', lambda error: errors.append(str(error)))
                row = {'name': name, 'status': 'RUNNING'}
                rows.append(row)
                try:
                    page.goto(args.url, wait_until='networkidle')
                    page.get_by_role('button', name='词典', exact=True).click()
                    expect(page.get_by_role('heading', name='个人词典', exact=True)).to_be_visible()
                    button = page.get_by_role('button', name='导入 Typeless', exact=True)
                    expect(button).to_be_visible()
                    if name == 'desktop-unavailable':
                        expect(button).to_be_disabled()
                        expect(page.get_by_role('button', name='新词', exact=True)).to_be_disabled()
                        expect(page.get_by_text('在桌面应用中添加和管理词汇。', exact=True)).to_be_visible()
                        assert page.evaluate('typeof window.__TAURI_INTERNALS__') == 'undefined'
                        row['commands'] = []
                        row['layouts'] = [simple.assert_layout(page, size) for size in [720, 430]]
                    else:
                        expect(button).to_be_enabled()
                        expect(page.locator('.dictionary-list > li')).to_have_count(len(terms))
                        if name == 'pending-doubleclick-navigation':
                            # Send a genuine two-click pointer sequence; the DOM and ref gates must
                            # suppress the second call while the controlled response is pending.
                            box = button.bounding_box()
                            page.mouse.click(box['x'] + box['width']/2, box['y'] + box['height']/2, click_count=2)
                            busy_button = page.get_by_role('button', name='正在导入 Typeless 词汇', exact=True)
                            expect(busy_button).to_be_disabled()
                            expect(busy_button).to_have_attribute('aria-busy', 'true')
                            expect(page.get_by_role('button', name='新词', exact=True)).to_be_disabled()
                            expect(page.get_by_role('button', name='编辑词条 SpeakMode', exact=True)).to_be_disabled()
                            expect(page.locator('.dictionary-library')).to_have_attribute('aria-busy', 'true')
                            assert_import_count(page, 1)
                            page.get_by_role('button', name='首页', exact=True).click()
                            guard = page.get_by_role('alertdialog', name='保存这次修改？', exact=True)
                            expect(guard).to_contain_text('正在处理这次操作，请等完成后再离开。')
                            expect(guard.get_by_role('button', name='保存并离开', exact=True)).to_be_disabled()
                            expect(guard.get_by_role('button', name='放弃修改', exact=True)).to_be_disabled()
                            guard.get_by_role('button', name='继续编辑', exact=True).click()
                            expect(page.get_by_role('heading', name='个人词典', exact=True)).to_be_visible()
                            page.screenshot(path=str(args.out / 'pending-disabled.png'))
                            page.evaluate('window.__speakmodeQA.deferImport=false;window.__speakmodeQA.releaseImport()')
                            expect(button).to_be_enabled()
                            expect(page.locator('.dictionary-notice')).to_contain_text('已从 Typeless 导入 2 个新词')
                            assert_import_count(page, 1)
                        elif name.startswith('keyboard-layout'):
                            row['layouts'] = [simple.assert_layout(page, width, grid=True)]
                            # Reach and activate the new action with keyboard navigation alone.
                            page.get_by_role('button', name='词典', exact=True).focus()
                            reached = False
                            for _ in range(18):
                                page.keyboard.press('Tab')
                                if button.evaluate('(el)=>el===document.activeElement'):
                                    reached = True
                                    break
                            assert reached, page.evaluate('document.activeElement.outerHTML')
                            expect(button).to_be_focused()
                            page.screenshot(path=str(args.out / f'{name}-focus.png'))
                            page.keyboard.press('Enter')
                            expect(page.locator('.dictionary-notice')).to_contain_text('已从 Typeless 导入 2 个新词')
                            expect(button).to_be_enabled()
                            row['layouts'].append(simple.assert_layout(page, width, grid=True))
                            assert_import_count(page, 1)
                        elif name in failure_messages:
                            button.click()
                            expect(page.locator('.dictionary-error')).to_have_text(failure_messages[name])
                            expect(button).to_be_enabled()
                            assert visible_terms(page) == terms
                            assert_import_count(page, 1)
                            page.screenshot(path=str(args.out / (name + '-error.png')))
                            # A corrected source must be retryable without losing existing terms.
                            page.evaluate('window.__speakmodeQA.importError=null')
                            button.click()
                            expect(page.locator('.dictionary-notice')).to_contain_text('已从 Typeless 导入 2 个新词')
                            assert [call['payload']['expectedVersion'] for call in import_calls(page)] == [1, 1]
                        elif name == 'malformed-response':
                            button.click()
                            expect(page.locator('.dictionary-error')).to_contain_text('导入结果未能确认')
                            expect(button).to_be_enabled()
                            assert visible_terms(page) == terms
                            assert_import_count(page, 1)
                        elif name in ['revision-conflict', 'conflict-refresh-failure']:
                            if name == 'conflict-refresh-failure':
                                page.evaluate('window.__speakmodeQA.failDictionaryList=true')
                            button.click()
                            assert_import_count(page, 1)
                            if name == 'conflict-refresh-failure':
                                expect(page.get_by_text('词库已变化，请重新读取后再导入。', exact=True)).to_be_visible()
                                assert visible_terms(page) == terms
                                page.evaluate('window.__speakmodeQA.failDictionaryList=false')
                                page.get_by_role('button', name='重新读取', exact=True).click()
                            else:
                                expect(page.locator('.dictionary-error')).to_have_text('词库已更新，请再次点击导入。已有词汇会自动跳过。')
                            expect(page.get_by_role('button', name='编辑词条 ConcurrentTerm', exact=True)).to_be_visible()
                            assert_import_count(page, 1)  # No automatic retry after a stale revision.
                            page.screenshot(path=str(args.out / (name + '-refreshed.png')))
                            button.click()
                            expect(page.locator('.dictionary-notice')).to_contain_text('已从 Typeless 导入 2 个新词')
                            assert [call['payload']['expectedVersion'] for call in import_calls(page)] == [1, 2]
                        else:
                            button.click()
                            expect(page.locator('.dictionary-notice')).to_have_text('已从 Typeless 导入 2 个新词，重复词已跳过。下次录音生效。')
                            expect(page.locator('.dictionary-list > li')).to_have_count(4)
                            assert visible_terms(page) == terms + ['Typeless', 'NewTerm']
                            button.click()
                            expect(page.locator('.dictionary-notice')).to_have_text('Typeless 中的词汇已在词典里，无需重复导入。')
                            expect(page.locator('.dictionary-list > li')).to_have_count(4)
                            assert [call['payload']['expectedVersion'] for call in import_calls(page)] == [1, 2]
                        row['commands'] = simple.commands_are_scoped(page)
                        for call in import_calls(page):
                            assert sorted(call['payload']) == ['expectedVersion', 'requestId'], call
                            assert isinstance(call['payload']['requestId'], str) and call['payload']['requestId']
                    qa.write_new(args.out / (name + '-dom.txt'), page.locator('.dictionary-page').inner_text())
                    page.screenshot(path=str(args.out / (name + '-final.png')))
                    row['status'] = 'PASS'
                except Exception as error:
                    row.update({'status': 'FAIL', 'error': str(error)})
                    page.screenshot(path=str(args.out / (name + '-failed.png')))
                finally:
                    context.close()
        finally:
            browser.close()
    receipt['status'] = 'PASS' if all(row['status'] == 'PASS' for row in rows) and not errors and not external else 'FAIL'
    qa.write_new(args.out / 'receipt.json', receipt, json_value=True)
    print(json.dumps({'status': receipt['status'], 'cases': [{'name': row['name'], 'status': row['status'], 'error': row.get('error')} for row in rows],
                      'pageErrors': errors, 'externalRequests': external}, ensure_ascii=True))
    return 0 if receipt['status'] == 'PASS' else 1


if __name__ == '__main__':
    sys.exit(main())
