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

DEFAULT_REWRITE = 'qwen3.8-flash'
DEFAULT_LABEL = 'Qwen 3.8 Flash（默认）'
SAVED_NOTICE = '服务配置已保存。尚未验证网络、账号权限和模型可用性。'
STALE_NOTICE = '已保存配置发生变化。当前编辑内容仍保留；请重新载入后再修改，避免覆盖其他更改。'
CATALOG = {
    "asr": [
        {"id": "qwen3-asr-flash-realtime", "label": "Qwen3 实时（当前默认）", "implemented": True, "adapterVersion": "qwen-realtime-v1"},
        {"id": "qwen3-asr-flash-realtime-2026-02-10", "label": "Qwen3 实时 · 2026-02-10（待评测）", "implemented": True, "adapterVersion": "qwen-realtime-v1"},
        {"id": "qwen-audio-3.1-asr-flash-streaming", "label": "Qwen Audio 3.1 流式（热词候选）", "implemented": True, "adapterVersion": "qwen-task-streaming-v1", "dictionarySupport": "vocabularyAndContext"},
    ],
    "rewrite": [
        {"id": DEFAULT_REWRITE, "label": DEFAULT_LABEL, "implemented": True},
        {"id": "qwen-flash", "label": "Qwen Flash", "implemented": True},
        {"id": "qwen-plus", "label": "Qwen Plus", "implemented": True},
        {"id": "qwen3.7-plus", "label": "Qwen 3.7 Plus", "implemented": True},
        {"id": "qwen3.7-plus-2026-05-26", "label": "Qwen 3.7 Plus · 2026-05-26", "implemented": True},
    ],
}
PROVIDER_CATALOG = [
    {'id': 'aliyun', 'label': '阿里云百炼', **CATALOG},
    {'id': 'google', 'label': 'Google（录音后转写）', 'asr': [
        {'id': 'synthetic-google-transcribe', 'label': 'Google 测试转写', 'implemented': True, 'adapterVersion': 'synthetic-google-v1', 'productAudioLimitSeconds': 420}], 'rewrite': []},
    *[{'id': provider_id, 'label': label, 'asr': [], 'rewrite': [
        {'id': model, 'label': model_label, 'implemented': True}]} for provider_id, label, model, model_label in [
        ('deepseek', 'DeepSeek', 'deepseek-flash', 'DeepSeek Flash'),
        ('kimi', 'Kimi', 'kimi-k2.6', 'Kimi K2.6'),
        ('zhipu', '智谱', 'glm-4.7', 'GLM 4.7'),
        ('openai', 'OpenAI', 'gpt-5.4-mini', 'GPT 5.4 Mini'),
    ]],
    {'id': 'custom', 'label': '自定义兼容服务', 'asr': [], 'rewrite': []},
]
CONFIGURE = """
   const providerAccount=(config,stage)=>{
    const id=stage==='asr'?config.provider:config.rewriteProvider;
    return id==='aliyun'?id+':'+config.region+':'+(config.workspace||'default'):id==='custom'?id+':'+config.rewriteEndpoint:id;
   };
   const updateProviderStatus=()=>{
    const provider=fixture.snapshot.provider,config=provider.config;
    provider.asrCredentialConfigured=fixture.savedAccounts.includes(providerAccount(config,'asr'));
    provider.rewriteCredentialConfigured=fixture.savedAccounts.includes(providerAccount(config,'rewrite'));
    provider.credentialConfigured=provider.asrCredentialConfigured&&provider.rewriteCredentialConfigured;
    provider.configured=provider.credentialConfigured;
   };
   if(name==='usage.today')return copy(fixture.snapshot.usage);
   if(name==='provider.configure') {
    if(fixture.providerSaveFailures>0){fixture.providerSaveFailures--;throw new Error('Synthetic provider save rejected');}
    if(payload.expectedVersion!==fixture.snapshot.provider.config.version)throw new Error('Synthetic stale settings');
    fixture.snapshot.provider.config={...copy(payload.config),version:payload.expectedVersion+1};
    updateProviderStatus();
    return copy(fixture.snapshot.provider.config);
   }
   if(name==='credential.set'||name==='credential.delete') {
    if(name==='credential.set'&&fixture.credentialFailures>0){fixture.credentialFailures--;throw new Error('Synthetic rejected credential '+payload.secret);}
    const config=fixture.snapshot.provider.config;
    if(payload.expectedProviderVersion!==config.version)throw new Error('Synthetic stale credential settings');
    if(!['asr','rewrite'].includes(payload.stage))throw new Error('Missing synthetic credential stage');
    const id=payload.stage==='asr'?config.provider:config.rewriteProvider;
    if(payload.providerAccountRef!==id)throw new Error('Synthetic mismatched credential provider');
    const scope=providerAccount(config,payload.stage);
    fixture.savedAccounts=fixture.savedAccounts.filter(account=>account!==scope);
    if(name==='credential.set')fixture.savedAccounts.push(scope);
    updateProviderStatus();
    return {configured:fixture.snapshot.provider.configured};
   }
"""
PROVIDER_CONTROLS = """
 const providerChannels=[];
 window.__speakmodeProviderQA={
  saved:()=>copy(fixture.snapshot.provider),
  updateSavedConfig:patch=>{
   const config=fixture.snapshot.provider.config;
   fixture.snapshot.provider.config={...config,...copy(patch),version:config.version+1};
   fixture.snapshot.sequence++;
   const event={kind:'snapshot',sequence:fixture.snapshot.sequence,payload:copy(fixture.snapshot)};
   for(const channel of providerChannels)channel.onmessage(copy(event));
  }
 };
"""


def synthetic_bridge(data):
    bridge = qa.BRIDGE.replace("   if(name==='state.snapshot')", CONFIGURE + "   if(name==='state.snapshot')")
    bridge = bridge.replace(' window.__TAURI_INTERNALS__ = {', PROVIDER_CONTROLS + ' window.__TAURI_INTERNALS__ = {')
    bridge = bridge.replace("if(command==='subscribe_state')return copy(fixture.snapshot);",
                            "if(command==='subscribe_state'){providerChannels.push(args.channel);return copy(fixture.snapshot);}")
    return bridge.replace('__FIXTURE__', json.dumps(data, ensure_ascii=False))


def configure_calls(page):
    return [call for call in qa.command_ledger(page) if call['name'] == 'provider.configure']


def saved_provider(page):
    return page.evaluate('window.__speakmodeProviderQA.saved()')


def choose_rewrite(page, label):
    page.get_by_role('combobox', name='整理模型', exact=True).click()
    page.get_by_role('option', name=label, exact=True).click()


def choose_provider(page, stage, label):
    page.get_by_role('combobox', name='语音识别供应商' if stage == 'asr' else '文字整理供应商', exact=True).click()
    page.get_by_role('option', name=label, exact=True).click()


def assert_isolated(page, credentials=False):
    # Even an accidental unexpected command rejected by the bridge must fail QA.
    allowed = {'state.snapshot', 'audio.listDevices', 'history.list', 'history.get', 'usage.today', 'provider.configure'}
    if credentials:
        allowed.update(('credential.set', 'credential.delete'))
    unexpected = [call['name'] for call in qa.command_ledger(page) if call['name'] not in allowed]
    assert not unexpected, f'Unexpected commands: {unexpected}'
    for field in page.locator('input[type=password]').all():
        expect(field).to_have_value('')


def assert_rewrite_payload(payload, original, model, version):
    assert payload['expectedVersion'] == version
    # Full equality also protects provider account, region, workspace, ASR and adapter.
    assert payload['config'] == {**original, 'version': version, 'rewriteModel': model}


def return_to_settings(page, label):
    page.get_by_role('button', name='首页', exact=True).click()
    expect(page.get_by_role('region', name='云端服务（BYOK）', exact=True)).to_have_count(0)
    page.get_by_role('button', name='设置', exact=True).click()
    expect(page.get_by_role('combobox', name='整理模型', exact=True)).to_have_text(label)
    expect(page.get_by_role('button', name='保存服务配置', exact=True)).to_be_disabled()


def assert_menu_bounds(page):
    menu = page.get_by_role('listbox')
    expect(menu).to_be_visible()
    # Measure the settled Radix popper, not a smaller/translucent animation frame.
    menu.evaluate("""async element => {
        const opening = document.getAnimations().filter(animation =>
            animation.playState === 'running' && animation.effect?.target instanceof Element
            && animation.effect.target.contains(element));
        await Promise.all(opening.map(animation => animation.finished.catch(() => {})));
    }""")
    bounds = menu.bounding_box()
    viewport = page.viewport_size
    assert bounds and bounds['width'] > 0 and bounds['height'] > 0
    assert bounds['x'] >= -1 and bounds['x'] + bounds['width'] <= viewport['width'] + 1, bounds
    assert bounds['y'] >= -1 and bounds['y'] + bounds['height'] <= viewport['height'] + 1, bounds
    assert page.evaluate('document.documentElement.scrollWidth <= document.documentElement.clientWidth'), 'Horizontal page overflow'
    return bounds

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
            cases = ['compatible-save', 'task-streaming-save', 'unknown-existing', 'empty-existing', 'missing-catalog', 'browser-preview',
                     'new-default-visible', 'existing-profile-retained', 'unknown-rewrite-existing',
                     'rewrite-roundtrip-beijing', 'rewrite-roundtrip-singapore', 'rewrite-save-failure',
                     'rewrite-stale-version', 'rewrite-keyboard-720', 'multi-rewrite-providers', 'multi-google-asr',
                     'multi-custom-guard', 'multi-credentials', 'multi-scope-draft', 'multi-provider-stale',
                     'multi-provider-keyboard-720', 'multi-onboarding-compact']
            for name in cases:
                context = browser.new_context(viewport={'width': 720 if name.endswith('720') else 1220, 'height': 900}, locale='zh-CN')
                data = qa.fixture()
                provider = data['snapshot']['provider']
                provider.update({'modelCatalog': CATALOG, 'providerCatalog': PROVIDER_CATALOG, 'configured': True,
                                 'credentialConfigured': True, 'asrCredentialConfigured': True, 'rewriteCredentialConfigured': True})
                provider['config'].update({'provider': 'aliyun', 'rewriteProvider': 'aliyun', 'rewriteEndpoint': None,
                                          'region': 'beijing', 'asrModel': 'qwen3-asr-flash-realtime', 'rewriteModel': DEFAULT_REWRITE, 'adapterVersion': 'qwen-realtime-v1'})
                if name in ('compatible-save', 'task-streaming-save', 'unknown-existing', 'existing-profile-retained'):
                    provider['config']['rewriteModel'] = 'qwen-flash'
                if name == 'rewrite-roundtrip-singapore':
                    provider['config'].update({'region': 'singapore', 'workspace': 'synthetic-workspace'})
                if name == 'unknown-existing':
                    provider['config']['asrModel'] = 'unverified-model'
                if name == 'unknown-rewrite-existing':
                    provider['config']['rewriteModel'] = 'unverified-rewrite-model'
                if name == 'empty-existing':
                    provider['config']['asrModel'] = ''
                    provider['config']['rewriteModel'] = ''
                if name == 'missing-catalog':
                    del provider['modelCatalog']
                    del provider['providerCatalog']
                if name == 'rewrite-save-failure':
                    data['providerSaveFailures'] = 1
                if name == 'multi-google-asr':
                    provider['config']['region'] = None
                if name == 'multi-credentials':
                    data['credentialFailures'] = 1
                if name == 'multi-onboarding-compact':
                    provider.update(configured=False, credentialConfigured=False, asrCredentialConfigured=False, rewriteCredentialConfigured=False)
                    data['snapshot']['onboarding'] = {'active': True, 'shortcutPresses': 0, 'progress': {
                        'version': 1, 'autoShow': True, 'step': 'setup', 'completedLessons': [], 'completedAt': None}}
                data['savedAccounts'] = ['aliyun:singapore:synthetic-workspace' if name == 'rewrite-roundtrip-singapore' else 'aliyun:beijing:default']
                original = dict(provider['config'])
                if name != 'browser-preview':
                    context.add_init_script(synthetic_bridge(data))
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
                    if name != 'multi-onboarding-compact':
                        page.get_by_role('button', name='设置', exact=True).click()
                    section = page.get_by_role('region', name='云端服务（BYOK）', exact=True)
                    # HTML sections are named by aria-labelledby, exposing region.
                    expect(section).to_be_visible()
                    asr = page.get_by_role('combobox', name='语音识别模型', exact=True)
                    rewrite = page.get_by_role('combobox', name='整理模型', exact=True)
                    model_hint = page.locator('#provider-rewrite-model-id')
                    save = page.get_by_role('button', name='保存服务配置', exact=True)
                    expect(save).to_be_disabled()
                    expect(section.locator('form').first.get_by_role('combobox').first).to_have_attribute('aria-label', '语音识别供应商')
                    if name in ('missing-catalog', 'browser-preview'):
                        expect(asr).to_be_disabled()
                        expect(rewrite).to_be_disabled()
                    elif name == 'empty-existing':
                        expect(asr).to_contain_text('尚未确认兼容')
                        expect(rewrite).to_contain_text('尚未确认兼容')
                    elif name in ('compatible-save', 'task-streaming-save', 'unknown-existing'):
                        asr.click()
                        expect(page.get_by_role('option', name='Qwen Audio 3.1 流式（热词候选）', exact=True)).to_be_enabled()
                        selected = 'Qwen Audio 3.1 流式（热词候选）' if name == 'task-streaming-save' else 'Qwen3 实时 · 2026-02-10（待评测）'
                        page.get_by_role('option', name=selected, exact=True).click()
                        if name == 'task-streaming-save':
                            expect(page.locator('#provider-asr-capability')).to_contain_text('支持词典热词与词汇上下文')
                        choose_rewrite(page, 'Qwen Plus')
                        expect(save).to_be_enabled()
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                        calls = configure_calls(page)
                        assert len(calls) == 1
                        payload = calls[0]['payload']
                        assert payload['expectedVersion'] == 1
                        assert payload['config']['adapterVersion'] == ('qwen-task-streaming-v1' if name == 'task-streaming-save' else 'qwen-realtime-v1')
                        assert payload['config']['asrModel'] == ('qwen-audio-3.1-asr-flash-streaming' if name == 'task-streaming-save' else 'qwen3-asr-flash-realtime-2026-02-10')
                        assert payload['config']['rewriteModel'] == 'qwen-plus'
                        row['configurePayload'] = payload
                    elif name == 'multi-rewrite-providers':
                        payloads = []
                        for version, provider_id in enumerate(('deepseek', 'kimi', 'zhipu', 'openai', 'aliyun'), 1):
                            definition = next(item for item in PROVIDER_CATALOG if item['id'] == provider_id)
                            choose_provider(page, 'rewrite', definition['label'])
                            expect(rewrite).to_have_text(definition['rewrite'][0]['label'])
                            rewrite.click()
                            expect(page.get_by_role('option')).to_have_text([model['label'] for model in definition['rewrite']])
                            page.keyboard.press('Escape')
                            save.click()
                            expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                            payload = configure_calls(page)[-1]['payload']
                            assert payload['expectedVersion'] == version
                            assert payload['config'] == {**original, 'version': version,
                                'rewriteProvider': definition['id'], 'rewriteModel': definition['rewrite'][0]['id']}
                            assert saved_provider(page)['asrCredentialConfigured'] is True
                            assert saved_provider(page)['rewriteCredentialConfigured'] is (definition['id'] == 'aliyun')
                            payloads.append(payload)
                        row['configurePayloads'] = payloads
                    elif name == 'multi-google-asr':
                        choose_provider(page, 'asr', 'Google（录音后转写）')
                        choose_provider(page, 'rewrite', 'DeepSeek')
                        expect(section.get_by_text('Google 在结束录音后上传音频并转写，录音期间不显示实时识别字幕。单次录音最长 7 分钟。', exact=True)).to_be_visible()
                        expect(page.locator('#provider-region')).to_have_count(0)
                        expect(save).to_be_enabled()
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                        payload = configure_calls(page)[-1]['payload']
                        assert payload['config'] == {**original, 'provider': 'google', 'asrModel': 'synthetic-google-transcribe',
                            'adapterVersion': 'synthetic-google-v1', 'rewriteProvider': 'deepseek', 'rewriteModel': 'deepseek-flash'}
                        assert saved_provider(page)['asrCredentialConfigured'] is False
                        assert saved_provider(page)['rewriteCredentialConfigured'] is False
                        assert saved_provider(page)['configured'] is False
                        expect(section.locator('input[type=password]')).to_have_count(2)
                        row['configurePayload'] = payload
                    elif name == 'multi-custom-guard':
                        choose_provider(page, 'rewrite', '自定义兼容服务')
                        custom_model = page.locator('#provider-rewrite-model')
                        endpoint = page.locator('#provider-rewrite-endpoint')
                        expect(save).to_be_disabled()
                        custom_model.fill('custom.test-model')
                        for invalid in ['', 'http://api.example.com/v1', 'https://name:secret@example.com/v1',
                                        'https://api.example.com/v1?api_key=secret', 'https://api.example.com/v1#secret']:
                            endpoint.fill(invalid)
                            expect(save).to_be_disabled()
                        for valid in ['https://api.example.com/v1', 'http://127.0.0.1:8765/v1']:
                            endpoint.fill(valid)
                            expect(save).to_be_enabled()
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                        payload = configure_calls(page)[-1]['payload']
                        assert payload['config'] == {**original, 'rewriteProvider': 'custom',
                            'rewriteModel': 'custom.test-model', 'rewriteEndpoint': 'http://127.0.0.1:8765/v1'}
                        expect(section.locator('input[type=password]')).to_have_count(2)
                        page.locator('#provider-rewrite-api-key').fill('synthetic-custom-draft')
                        endpoint.fill('https://another.example.com/v1')
                        expect(page.locator('#provider-rewrite-api-key')).to_have_value('')
                        expect(page.locator('#provider-rewrite-api-key')).to_be_disabled()
                        page.get_by_role('button', name='重新载入', exact=True).click()
                        expect(endpoint).to_have_value('http://127.0.0.1:8765/v1')
                        row['configurePayload'] = payload
                    elif name == 'multi-credentials':
                        page.locator('#provider-api-key').fill('synthetic-old-account-draft')
                        choose_provider(page, 'rewrite', 'DeepSeek')
                        expect(page.locator('#provider-api-key')).to_have_value('')
                        expect(page.locator('#provider-api-key')).to_be_disabled()
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                        asr_form = section.locator('form.settings-credential-form').nth(0)
                        rewrite_form = section.locator('form.settings-credential-form').nth(1)
                        expect(asr_form.get_by_text('API Key：已保存', exact=True)).to_be_visible()
                        expect(rewrite_form.get_by_text('API Key：未保存', exact=True)).to_be_visible()
                        expect(rewrite_form.get_by_role('button', name='删除已保存的 Key', exact=True)).to_be_disabled()
                        page.locator('#provider-rewrite-api-key').fill('synthetic-deepseek-key')
                        rewrite_form.get_by_role('button', name='保存 API Key', exact=True).click()
                        expect(section.get_by_role('alert')).to_have_text('未能确认 API Key 已保存，请重试。')
                        expect(section.get_by_role('alert')).not_to_contain_text('synthetic-deepseek-key')
                        assert saved_provider(page)['rewriteCredentialConfigured'] is False
                        rewrite_form.get_by_role('button', name='保存 API Key', exact=True).click()
                        expect(rewrite_form.get_by_text('API Key：已保存', exact=True)).to_be_visible()
                        expect(page.locator('#provider-rewrite-api-key')).to_have_value('')
                        assert saved_provider(page)['configured'] is True
                        page.locator('#provider-api-key').fill('synthetic-aliyun-key')
                        asr_form.get_by_role('button', name='保存 API Key', exact=True).click()
                        expect(page.locator('#provider-api-key')).to_have_value('')
                        rewrite_form.get_by_role('button', name='删除已保存的 Key', exact=True).click()
                        expect(rewrite_form.get_by_text('API Key：未保存', exact=True)).to_be_visible()
                        expect(asr_form.get_by_text('API Key：已保存', exact=True)).to_be_visible()
                        assert saved_provider(page)['configured'] is False
                        calls = [call for call in qa.command_ledger(page) if call['name'].startswith('credential.')]
                        assert [(call['name'], call['payload']['stage'], call['payload']['providerAccountRef']) for call in calls] == [
                            ('credential.set', 'rewrite', 'deepseek'), ('credential.set', 'rewrite', 'deepseek'),
                            ('credential.set', 'asr', 'aliyun'), ('credential.delete', 'rewrite', 'deepseek')]
                        assert all(call['payload']['expectedProviderVersion'] == 2 for call in calls)
                        row['credentialRequests'] = [{key: value for key, value in call['payload'].items() if key != 'secret'} for call in calls]
                    elif name == 'multi-scope-draft':
                        page.locator('#provider-api-key').fill('synthetic-region-draft')
                        page.get_by_role('combobox', name='阿里云账号地域', exact=True).click()
                        page.get_by_role('option', name='国际（新加坡）', exact=True).click()
                        expect(page.locator('#provider-api-key')).to_have_value('')
                        expect(page.locator('#provider-api-key')).to_be_disabled()
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                        expect(section.locator('form.settings-credential-form')).to_have_count(1)
                        expect(section.get_by_text('API Key：未保存', exact=True)).to_be_visible()
                        page.locator('#provider-api-key').fill('synthetic-workspace-draft')
                        page.locator('#provider-workspace').fill('another-workspace')
                        expect(page.locator('#provider-api-key')).to_have_value('')
                        page.get_by_role('button', name='重新载入', exact=True).click()
                        expect(page.locator('#provider-workspace')).to_have_value('')
                        page.locator('#provider-api-key').fill('synthetic-external-change-draft')
                        page.evaluate("window.__speakmodeProviderQA.updateSavedConfig({region:'beijing'})")
                        expect(page.locator('#provider-api-key')).to_have_value('')
                        assert not [call for call in qa.command_ledger(page) if call['name'].startswith('credential.')]
                    elif name == 'multi-provider-stale':
                        choose_provider(page, 'rewrite', 'Kimi')
                        page.evaluate("window.__speakmodeProviderQA.updateSavedConfig({rewriteProvider:'deepseek',rewriteModel:'deepseek-flash'})")
                        expect(page.get_by_text(STALE_NOTICE, exact=True)).to_be_visible()
                        expect(page.get_by_role('combobox', name='文字整理供应商', exact=True)).to_have_text('Kimi')
                        expect(save).to_be_disabled()
                        expect(page.locator('#provider-api-key')).to_be_disabled()
                        assert not configure_calls(page)
                        page.get_by_role('button', name='重新载入', exact=True).click()
                        expect(page.get_by_role('combobox', name='文字整理供应商', exact=True)).to_have_text('DeepSeek')
                        choose_provider(page, 'rewrite', 'Kimi')
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                        payload = configure_calls(page)[-1]['payload']
                        assert payload['expectedVersion'] == 2
                        assert payload['config'] == {**original, 'version': 2, 'rewriteProvider': 'kimi', 'rewriteModel': 'kimi-k2.6'}
                        row['configurePayload'] = payload
                    elif name == 'multi-provider-keyboard-720':
                        selector = page.get_by_role('combobox', name='文字整理供应商', exact=True)
                        selector.focus()
                        page.keyboard.press('Space')
                        expect(page.get_by_role('listbox')).to_be_visible()
                        page.keyboard.press('End')
                        expect(page.get_by_role('option', name='自定义兼容服务', exact=True)).to_be_focused()
                        row['menuBounds'] = assert_menu_bounds(page)
                        page.keyboard.press('Home')
                        expect(page.get_by_role('option', name='阿里云百炼', exact=True)).to_be_focused()
                        page.keyboard.press('ArrowDown')
                        expect(page.get_by_role('option', name='DeepSeek', exact=True)).to_be_focused()
                        page.keyboard.press('Enter')
                        expect(selector).to_have_text('DeepSeek')
                        expect(selector).to_be_focused()
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                    elif name == 'multi-onboarding-compact':
                        expect(page.get_by_role('combobox', name='语音识别供应商', exact=True)).to_be_visible()
                        expect(page.get_by_role('combobox', name='文字整理供应商', exact=True)).to_be_visible()
                        expect(page.locator('#provider-asr-model')).to_have_count(0)
                        expect(page.locator('#provider-rewrite-model')).to_have_count(0)
                        expect(page.locator('#provider-workspace')).to_have_count(0)
                        choose_provider(page, 'rewrite', '自定义兼容服务')
                        expect(page.locator('#provider-rewrite-endpoint')).to_be_visible()
                        expect(page.locator('#provider-rewrite-model')).to_be_visible()
                        page.get_by_role('button', name='重新载入', exact=True).click()
                        expect(page.locator('#provider-rewrite-model')).to_have_count(0)
                    elif name in ('new-default-visible', 'existing-profile-retained'):
                        label = DEFAULT_LABEL if name == 'new-default-visible' else 'Qwen Flash'
                        expect(rewrite).to_have_text(label)
                        expect(section.get_by_text('已保存的整理模型：' + label, exact=True)).to_be_visible()
                        expect(model_hint).to_have_text('型号：' + original['rewriteModel'])
                        rewrite.click()
                        expect(page.get_by_role('option')).to_have_text([model['label'] for model in CATALOG['rewrite']])
                        for model in CATALOG['rewrite']:
                            expect(page.get_by_role('option', name=model['label'], exact=True)).to_be_enabled()
                        row['menuBounds'] = assert_menu_bounds(page)
                        page.screenshot(path=str(args.out / (name + '-menu.png')), full_page=True)
                        page.keyboard.press('Escape')
                        return_to_settings(page, label)
                        assert not configure_calls(page), 'Viewing settings must not migrate an explicit saved profile'
                        assert saved_provider(page)['config'] == original
                    elif name == 'unknown-rewrite-existing':
                        expect(rewrite).to_contain_text('尚未确认兼容')
                        expect(section.get_by_text('已保存的整理模型：unverified-rewrite-model', exact=True)).to_be_visible()
                        rewrite.click()
                        expect(page.get_by_role('option', name='当前配置（尚未确认兼容）', exact=True)).to_be_disabled()
                        page.get_by_role('option', name=DEFAULT_LABEL, exact=True).click()
                        expect(save).to_be_enabled()
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                        calls = configure_calls(page)
                        assert len(calls) == 1
                        assert_rewrite_payload(calls[0]['payload'], original, DEFAULT_REWRITE, 1)
                    elif name.startswith('rewrite-roundtrip-'):
                        expect(rewrite).to_have_text(DEFAULT_LABEL)
                        payloads = []
                        for model, label, version, previous_label in [
                            ('qwen-flash', 'Qwen Flash', 1, DEFAULT_LABEL),
                            (DEFAULT_REWRITE, DEFAULT_LABEL, 2, 'Qwen Flash'),
                        ]:
                            choose_rewrite(page, label)
                            expect(model_hint).to_have_text('型号：' + model + ' · 尚未保存')
                            expect(section.get_by_text('已保存的整理模型：' + previous_label, exact=True)).to_be_visible()
                            expect(save).to_be_enabled()
                            save.click()
                            expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                            expect(model_hint).to_have_text('型号：' + model)
                            calls = configure_calls(page)
                            assert len(calls) == version
                            payload = calls[-1]['payload']
                            assert_rewrite_payload(payload, original, model, version)
                            payloads.append(payload)
                            saved = saved_provider(page)
                            assert saved['config'] == {**original, 'version': version + 1, 'rewriteModel': model}
                            assert saved['credentialConfigured'] is True
                            expect(section.get_by_text('API Key：已保存', exact=True)).to_be_visible()
                            return_to_settings(page, label)
                            assert len(configure_calls(page)) == version
                        row['configurePayloads'] = payloads
                        row['savedProvider'] = saved_provider(page)
                    elif name == 'rewrite-save-failure':
                        choose_rewrite(page, 'Qwen Flash')
                        save.click()
                        expect(section.get_by_role('alert')).to_have_text('Synthetic provider save rejected')
                        expect(rewrite).to_have_text('Qwen Flash')
                        expect(model_hint).to_have_text('型号：qwen-flash · 尚未保存')
                        expect(section.get_by_text('已保存的整理模型：' + DEFAULT_LABEL, exact=True)).to_be_visible()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_have_count(0)
                        expect(save).to_be_enabled()
                        assert saved_provider(page)['config'] == original
                        assert len(configure_calls(page)) == 1
                        page.screenshot(path=str(args.out / (name + '-preserved-draft.png')), full_page=True)
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                        calls = configure_calls(page)
                        assert len(calls) == 2
                        for call in calls:
                            assert_rewrite_payload(call['payload'], original, 'qwen-flash', 1)
                        assert saved_provider(page)['config']['version'] == 2
                        return_to_settings(page, 'Qwen Flash')
                        row['configurePayloads'] = [call['payload'] for call in calls]
                    elif name == 'rewrite-stale-version':
                        choose_rewrite(page, 'Qwen Flash')
                        page.evaluate("window.__speakmodeProviderQA.updateSavedConfig({rewriteModel:'qwen-plus'})")
                        expect(page.get_by_text(STALE_NOTICE, exact=True)).to_be_visible()
                        expect(rewrite).to_have_text('Qwen Flash')
                        expect(model_hint).to_have_text('型号：qwen-flash · 尚未保存')
                        expect(section.get_by_text('已保存的整理模型：Qwen Plus', exact=True)).to_be_visible()
                        expect(save).to_be_disabled()
                        assert not configure_calls(page)
                        page.screenshot(path=str(args.out / (name + '-blocked.png')), full_page=True)
                        page.get_by_role('button', name='重新载入', exact=True).click()
                        expect(rewrite).to_have_text('Qwen Plus')
                        expect(save).to_be_disabled()
                        expect(page.get_by_text(STALE_NOTICE, exact=True)).to_have_count(0)
                        choose_rewrite(page, DEFAULT_LABEL)
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                        calls = configure_calls(page)
                        assert len(calls) == 1
                        assert_rewrite_payload(calls[0]['payload'], original, DEFAULT_REWRITE, 2)
                        assert saved_provider(page)['config']['version'] == 3
                        row['configurePayload'] = calls[0]['payload']
                    elif name == 'rewrite-keyboard-720':
                        rewrite.scroll_into_view_if_needed()
                        rewrite.focus()
                        page.keyboard.press('Space')
                        expect(page.get_by_role('listbox')).to_be_visible()
                        page.keyboard.press('End')
                        last_label = CATALOG['rewrite'][-1]['label']
                        expect(page.get_by_role('option', name=last_label, exact=True)).to_be_focused()
                        row['menuBounds'] = assert_menu_bounds(page)
                        page.screenshot(path=str(args.out / (name + '-menu.png')), full_page=True)
                        page.keyboard.press('Enter')
                        expect(rewrite).to_have_text(last_label)
                        expect(rewrite).to_be_focused()
                        expect(model_hint).to_contain_text('尚未保存')
                        save.click()
                        expect(page.get_by_text(SAVED_NOTICE, exact=True)).to_be_visible()
                        calls = configure_calls(page)
                        assert len(calls) == 1
                        assert_rewrite_payload(calls[0]['payload'], original, CATALOG['rewrite'][-1]['id'], 1)
                        return_to_settings(page, last_label)
                        row['configurePayload'] = calls[0]['payload']
                    if name != 'browser-preview':
                        assert_isolated(page, credentials=name == 'multi-credentials')
                        row['commands'] = [call['name'] for call in qa.command_ledger(page)]
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
    print(json.dumps({'status': receipt['status'], 'cases': [{'name': r['name'], 'status': r['status'], **({'error': r['error']} if 'error' in r else {})} for r in rows], 'pageErrors': errors, 'externalRequests': external, 'receipt': str(args.out / 'receipt.json')}, ensure_ascii=False))
    return 0 if receipt['status'] == 'PASS' else 1

if __name__ == '__main__':
    sys.exit(main())
