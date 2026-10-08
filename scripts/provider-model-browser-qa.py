"""Headless production React settings checks using an isolated synthetic bridge.

No native Store, credentials, cloud requests, microphone or host input. Owns a
loopback preview through the existing QA helper and refuses occupied ports.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import sys
from urllib.parse import urlparse
from playwright.sync_api import sync_playwright, expect

spec = importlib.util.spec_from_file_location("restore_qa", Path(__file__).with_name("restore-delivery-browser-qa.py"))
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)

CATALOG = {
    "asr": [
        {"id": "qwen3-asr-flash-realtime", "label": "Qwen3 实时（当前默认）", "implemented": True, "adapterVersion": "qwen-realtime-v1"},
        {"id": "qwen3-asr-flash-realtime-2026-02-10", "label": "Qwen3 实时 · 2026-02-10（待评测）", "implemented": True, "adapterVersion": "qwen-realtime-v1"},
        {"id": "qwen-audio-3.1-asr-flash-streaming", "label": "Qwen Audio 3.1 流式（热词候选）", "implemented": True, "adapterVersion": "qwen-task-streaming-v1", "dictionarySupport": "vocabularyAndContext"},
    ],
    "rewrite": [
        {"id": "qwen-flash", "label": "Qwen Flash（当前默认）", "implemented": True},
        {"id": "qwen-plus", "label": "Qwen Plus（待评测）", "implemented": True},
    ],
}
CONFIGURE = """
   if(name==='provider.configure') {
    if(payload.expectedVersion!==fixture.snapshot.provider.config.version)throw new Error('Synthetic stale settings');
    fixture.snapshot.provider.config={...copy(payload.config),version:payload.expectedVersion+1};
    return copy(fixture.snapshot.provider.config);
   }
"""

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    parsed = urlparse(args.url)
    if parsed.scheme != 'http' or parsed.hostname != '127.0.0.1' or parsed.username or parsed.password:
        parser.error('Use an independent http://127.0.0.1 preview URL')
    if not args.out.is_absolute():
        parser.error('Use a new absolute evidence directory')
    args.out.mkdir(parents=True, exist_ok=False)
    rows, errors, external = [], [], []
    receipt = {'scope': __doc__, 'cases': rows, 'pageErrors': errors, 'externalRequests': external}
    with qa.managed_preview(args.url, args.out, True) as server, sync_playwright() as p:
        receipt['server'] = server
        browser = p.chromium.launch(channel='msedge', headless=True)
        receipt['browserVersion'] = browser.version
        try:
            for name in ['compatible-save', 'task-streaming-save', 'unknown-existing', 'empty-existing', 'missing-catalog', 'browser-preview']:
                context = browser.new_context(viewport={'width': 1220, 'height': 900}, locale='zh-CN')
                data = qa.fixture()
                provider = data['snapshot']['provider']
                provider.update({'modelCatalog': CATALOG, 'configured': True, 'credentialConfigured': False})
                provider['config'].update({'provider': 'aliyun', 'region': 'beijing', 'asrModel': 'qwen3-asr-flash-realtime', 'rewriteModel': 'qwen-flash', 'adapterVersion': 'qwen-realtime-v1'})
                if name == 'unknown-existing':
                    provider['config']['asrModel'] = 'unverified-model'
                if name == 'empty-existing':
                    provider['config']['asrModel'] = ''
                    provider['config']['rewriteModel'] = ''
                if name == 'missing-catalog':
                    del provider['modelCatalog']
                if name != 'browser-preview':
                    bridge = qa.BRIDGE.replace("   if(name==='state.snapshot')", CONFIGURE + "   if(name==='state.snapshot')")
                    context.add_init_script(bridge.replace('__FIXTURE__', json.dumps(data, ensure_ascii=False)))
                def route(r):
                    if urlparse(r.request.url).hostname not in ('127.0.0.1', None):
                        external.append(r.request.url)
                        r.abort()
                    else:
                        r.continue_()
                context.route('**/*', route)
                page = context.new_page()
                page.on('pageerror', lambda error: errors.append(str(error)))
                row = {'name': name, 'status': 'RUNNING'}
                rows.append(row)
                try:
                    page.goto(args.url, wait_until='networkidle')
                    page.get_by_role('button', name='设置', exact=True).click()
                    section = page.get_by_role('region', name='云端服务（BYOK）', exact=True)
                    # HTML sections are named by aria-labelledby, exposing region.
                    expect(section).to_be_visible()
                    asr = page.get_by_role('combobox', name='语音识别模型', exact=True)
                    save = page.get_by_role('button', name='保存服务配置', exact=True)
                    expect(save).to_be_disabled()
                    if name in ('missing-catalog', 'browser-preview'):
                        expect(asr).to_be_disabled()
                    elif name == 'empty-existing':
                        expect(asr).to_contain_text('尚未确认兼容')
                    else:
                        asr.click()
                        expect(page.get_by_role('option', name='Qwen Audio 3.1 流式（热词候选）', exact=True)).to_be_enabled()
                        selected = 'Qwen Audio 3.1 流式（热词候选）' if name == 'task-streaming-save' else 'Qwen3 实时 · 2026-02-10（待评测）'
                        page.get_by_role('option', name=selected, exact=True).click()
                        if name == 'task-streaming-save':
                            expect(page.locator('#provider-asr-capability')).to_contain_text('支持词典热词与词汇上下文')
                        page.get_by_role('combobox', name='整理模型', exact=True).click()
                        page.get_by_role('option', name='Qwen Plus（待评测）', exact=True).click()
                        expect(save).to_be_enabled()
                        save.click()
                        expect(page.get_by_text('服务配置已保存。尚未验证网络、账号权限和模型可用性。', exact=True)).to_be_visible()
                        calls = [r for r in qa.command_ledger(page) if r['name'] == 'provider.configure']
                        assert len(calls) == 1
                        payload = calls[0]['payload']
                        assert payload['expectedVersion'] == 1
                        assert payload['config']['adapterVersion'] == ('qwen-task-streaming-v1' if name == 'task-streaming-save' else 'qwen-realtime-v1')
                        assert payload['config']['asrModel'] == ('qwen-audio-3.1-asr-flash-streaming' if name == 'task-streaming-save' else 'qwen3-asr-flash-realtime-2026-02-10')
                        assert payload['config']['rewriteModel'] == 'qwen-plus'
                        assert not any(r['name'].startswith('credential.') for r in qa.command_ledger(page))
                        row['configurePayload'] = payload
                    row['status'] = 'PASS'
                    page.screenshot(path=str(args.out / (name + '.png')), full_page=True)
                except Exception as error:
                    row.update({'status': 'FAIL', 'error': str(error)})
                    page.screenshot(path=str(args.out / (name + '-failed.png')), full_page=True)
                finally:
                    context.close()
        finally:
            browser.close()
    receipt['status'] = 'PASS' if all(r['status'] == 'PASS' for r in rows) and not errors and not external else 'FAIL'
    qa.write_new(args.out / 'receipt.json', receipt, json_value=True)
    print(json.dumps({'status': receipt['status'], 'cases': [{k:v for k,v in r.items() if k != 'configurePayload'} for r in rows], 'pageErrors': errors}, ensure_ascii=False))
    return 0 if receipt['status'] == 'PASS' else 1

if __name__ == '__main__':
    sys.exit(main())
