"""Production React + isolated synthetic IPC; no native app/profile or clipboard."""
import argparse
import importlib.util
import json
from pathlib import Path
import sys
from urllib.parse import urlparse
from playwright.sync_api import sync_playwright, expect

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('delivery_base', ROOT / 'scripts/restore-delivery-browser-qa.py')
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--serve', action='store_true')
    args = parser.parse_args()
    if urlparse(args.url).hostname != '127.0.0.1' or not args.out.is_absolute():
        parser.error('Use loopback and a new absolute evidence directory')
    args.out.mkdir(parents=True, exist_ok=False)
    rows, errors = [], []
    with base.managed_preview(args.url, args.out, args.serve), sync_playwright() as pw:
        browser = pw.chromium.launch(channel='msedge', headless=True)
        scenarios = [(state, fallback) for state in ('unconfirmed', 'skipped', 'failed', 'prepared') for fallback in (False, True)]
        scenarios += [('incomplete', False), ('copy-failure', True), ('history', True), ('history', False)]
        for index, (state, fallback) in enumerate(scenarios):
            name = f'{index+1:02}-{state}-' + ('source' if fallback else 'model')
            data = base.fixture('skipped' if state == 'copy-failure' else state, 'targetUnavailableAtStop')
            if fallback:
                output = data['detail']['output']
                output.update(rewriteFallback='meaningRisk', resolvedMode='light')
                output['blocks'] = [dict(id=f'fallback-{i}', text=item['text'], sourceRefs=[item['id']])
                                    for i, item in enumerate(data['detail']['source']['items'])]
                data['snapshot']['session']['rewriteFallback'] = 'meaningRisk'
            if state == 'incomplete':
                data['snapshot']['session'].update(state='incomplete', error='识别未完整，已有文字可复制。', canCopyRecovery=True, recoverableContentId='synthetic-session')
            bridge = base.BRIDGE.replace('__FIXTURE__', json.dumps(data, ensure_ascii=False))
            bridge = bridge.replace("resolvedMode:o.resolvedMode,currentRevisionId", "rewriteFallback:o.rewriteFallback,resolvedMode:o.resolvedMode,currentRevisionId")
            bridge = bridge.replace("if(name==='result.copy')return null;", "if(name==='recoverable.copy')return null; if(name==='result.copy')" + ("throw new Error('合成剪贴板暂不可用');" if state == 'copy-failure' else 'return null;'))
            context = browser.new_context(viewport={'width': 468 if state != 'history' else 1240, 'height': 430 if state != 'history' else 860}, reduced_motion='reduce')
            context.add_init_script(bridge)
            page = context.new_page()
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.route('**/*', lambda route: route.continue_() if urlparse(route.request.url).hostname == '127.0.0.1' else route.abort())
            try:
                page.goto(args.url + ('/?window=recorder' if state != 'history' else '/'), wait_until='networkidle')
                if state == 'history':
                    expect(page.locator('.latest-title-row .mode-badge')).to_have_text('识别稿' if fallback else '结构')
                    page.get_by_role('button', name='查看与调整', exact=True).click()
                    expect(page.locator('.history-tools .mode-badge')).to_have_text('识别稿' if fallback else '结构')
                else:
                    expect(page.get_by_role('button', name='打开主面板', exact=True)).to_have_count(0)
                    expect(page.get_by_role('button', name='查看成稿', exact=True)).to_have_count(0)
                    if fallback:
                        expect(page.locator('.recorder-delivery')).to_contain_text('已使用识别稿')
                    if state in ('skipped', 'failed', 'copy-failure', 'incomplete'):
                        label = '复制已识别文字' if state == 'incomplete' else '复制文本'
                        button = page.get_by_role('button', name=label, exact=True)
                        expect(button).to_be_enabled()
                        button.click()
                        if state == 'copy-failure':
                            expect(page.get_by_role('alert')).to_contain_text('合成剪贴板暂不可用')
                            expect(page.get_by_role('button', name='复制文本', exact=True)).to_be_enabled()
                        else:
                            expect(page.get_by_role('button', name='已复制', exact=True)).to_be_visible()
                        calls = [row for row in base.command_ledger(page) if row['name'] in ('result.copy', 'recoverable.copy', 'app.showMain', 'session.stop')]
                        assert len(calls) == 1 and calls[0]['name'] == ('recoverable.copy' if state == 'incomplete' else 'result.copy'), calls
                    else:
                        expect(page.get_by_role('button', name='复制文本', exact=True)).to_have_count(0)
                    geometry = page.locator('.recorder-assembly').evaluate('(el)=>{const r=el.getBoundingClientRect();return {left:r.left,right:r.right,top:r.top,bottom:r.bottom,width:innerWidth,height:innerHeight,scroll:document.documentElement.scrollWidth};}')
                    assert geometry['left'] >= 0 and geometry['right'] <= geometry['width'] + 1 and geometry['top'] >= 0 and geometry['bottom'] <= geometry['height'] + 1 and geometry['scroll'] <= geometry['width'] + 1, geometry
                    assert not any(row['name'] == 'app.showMain' for row in base.command_ledger(page))
                base.screenshot(page, args.out / f'{name}.png')
                rows.append({'name': name, 'status': 'PASS'})
            except Exception as error:
                rows.append({'name': name, 'status': 'FAIL', 'error': str(error)})
                base.screenshot(page, args.out / f'{name}-failure.png')
            finally:
                context.close()
        browser.close()
    receipt = {'status': 'PASS' if not errors and all(row['status'] == 'PASS' for row in rows) else 'FAIL', 'scope': __doc__, 'tests': rows, 'pageErrors': errors}
    base.write_new(args.out / 'receipt.json', receipt, json_value=True)
    print(json.dumps(receipt, ensure_ascii=False, indent=2))
    return 0 if receipt['status'] == 'PASS' else 1


if __name__ == '__main__':
    sys.exit(main())
