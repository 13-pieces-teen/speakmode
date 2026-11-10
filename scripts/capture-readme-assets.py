"""Capture README screenshots from the current built React UI with fixed demo data.

Uses the existing isolated browser fixture, never native credentials, audio,
storage, clipboard or provider requests. Run `npm --prefix app run build` first.
Requires Python Playwright and an existing Microsoft Edge installation.
"""
import importlib.util
import json
from pathlib import Path
from tempfile import mkdtemp
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / 'docs/assets/readme'
AUDIT = ROOT / '.validation/readme-assets'
URL = 'http://127.0.0.1:14329'


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


personal = load('readme_personalization', 'personalization-browser-qa.py')
provider = load('readme_providers', 'provider-model-browser-qa.py')
qa = personal.qa


def demo_fixture():
    data = personal.fixture()
    snapshot, detail = data['snapshot'], data['detail']
    timestamp = '2026-11-10T06:28:00Z'
    mode = dict(modeId='email', name='邮件沟通', baseMode='light',
                rules='清楚表达来意与请求，保留原有条件。', revision=1)
    original = [
        '嗯，大家好，那个周五下午三点开评审会。',
        '请大家提前把演示材料发给我。',
        '如果会议室还没确认的话，先别发邀请。',
    ]
    final = ['大家好：', '周五下午三点召开评审会，请大家提前将演示材料发给我。',
             '如果会议室尚未确认，请先不要发送邀请。']
    detail['source']['items'] = [dict(id=f'demo-source-{i}', providerItemId=f'demo-{i}',
        captureOrder=i, text=text, final=True, startMs=None, endMs=None)
        for i, text in enumerate(original)]
    detail['source']['createdAt'] = timestamp
    detail['output'].update(modeSnapshot=mode, resolvedMode='light', createdAt=timestamp,
        blocks=[dict(id=f'demo-block-{i}', text=text,
                     sourceRefs=['demo-source-0', 'demo-source-1'] if i < 2 else ['demo-source-2'])
                for i, text in enumerate(final)],
        writingReceipt=dict(scene='email', language='asSpoken', contextUsed=False,
                            contractVersion='readme-demo'))
    detail['session'].update(modeSnapshot=mode, startedAt=timestamp, endedAt=timestamp,
                             endLocalDate='2026-11-10', captureDurationMs=26000)
    snapshot['preferences'].update(defaultModeId='email', softVoiceEnabled=False,
                                    recorderCaptionsEnabled=False)
    snapshot['modes'] = [dict(id=mid, name=name, baseMode='auto' if mid == 'auto' else 'light',
                            rules='保留原意', revision=1, builtin=True)
        for mid, name in [('auto', '自动'), ('chat', '聊天消息'), ('document', '文档写作'),
                          ('email', '邮件沟通'), ('technical', '技术沟通')]]
    snapshot['usage'] = dict(date='2026-11-10', count=1,
                             sourceCharCount=sum(map(len, original)), captureDurationMs=26000)
    snapshot['dictionary'] = dict(version=1, entries=[dict(id=f'demo-word-{i}', term=term,
        createdAt=timestamp, updatedAt=timestamp) for i, term in enumerate([
            'SpeakMode', 'Tauri', 'WebView2', 'TypeScript', 'Qwen', 'API', '大语言模型', '人机交互', '语音输入'])])
    snapshot['provider'].update(modelCatalog=provider.CATALOG,
        providerCatalog=provider.PROVIDER_CATALOG, configured=True,
        credentialConfigured=True, asrCredentialConfigured=True, rewriteCredentialConfigured=True)
    snapshot['provider']['config'].update(provider='aliyun', rewriteProvider='aliyun',
        rewriteEndpoint=None, region='beijing', workspace=None, asrModel='qwen3-asr-flash-realtime',
        rewriteModel='qwen3.8-flash', adapterVersion='qwen-realtime-v1')
    profile = data['config']['profiles'][0]
    profile.update(name='开发与产品', profession='软件开发', domain='AI 与产品研发',
        description='把技术讨论整理为清楚、简洁的工作记录。',
        tasks=['技术讨论', '项目进展', '文档写作'], audience='同事',
        scenes=['document', 'technical', 'email'])
    data['config']['preferences'].update(keepEnglishTerms=True, fewerCourtesies=True)
    data['title'] = '周五评审安排'
    return data


def bridge(data):
    script = personal.bridge(data)
    script = script.replace("   if(name==='audio.listDevices')return [];", """
   if(name==='audio.listDevices')return [{id:'demo-mic',name:'系统默认麦克风',isDefault:true,available:true}];
   if(name==='dictionary.list')return copy(fixture.snapshot.dictionary);
   if(name==='app.ackMainResult')return null;
""")
    script = script.replace("title:o.blocks.map(b=>b.text).join(' ').slice(0,48)", 'title:fixture.title')
    return script


def main():
    ASSETS.mkdir(parents=True, exist_ok=True)
    AUDIT.mkdir(parents=True, exist_ok=True)
    run_dir = Path(mkdtemp(prefix='capture-', dir=AUDIT))
    errors, external, captures = [], [], []
    with qa.managed_preview(URL, run_dir, True) as server, sync_playwright() as p:
        browser = p.chromium.launch(channel='msedge', headless=True)
        try:
            context = browser.new_context(viewport=dict(width=1360, height=900),
                device_scale_factor=1, locale='zh-CN', reduced_motion='reduce')
            context.add_init_script(bridge(demo_fixture()))

            def route(request):
                if urlparse(request.request.url).hostname not in ('127.0.0.1', None):
                    external.append(request.request.url)
                    request.abort()
                else:
                    request.continue_()

            context.route('**/*', route)
            page = context.new_page()
            page.on('pageerror', lambda error: errors.append(str(error)))

            def capture(name, selector=None):
                page.evaluate('document.fonts.ready')
                banners = page.locator('.environment-error').all_text_contents()
                assert not banners, banners
                assert page.get_by_text('请在 SpeakMode 桌面应用中使用', exact=True).count() == 0
                assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                target = page.locator(selector) if selector else page
                target.screenshot(path=str(ASSETS / name), animations='disabled')
                captures.append(dict(file=name, title=page.title(), viewport=page.viewport_size,
                    alerts=[text for text in page.locator('[role="alert"]').all_text_contents() if text.strip()]))

            page.goto(URL, wait_until='networkidle')
            page.screenshot(path=str(run_dir / 'initial-ui.png'))
            expect(page.locator('.home-hero h1')).to_be_visible()
            capture('home.png')

            page.get_by_role('button', name='场合：邮件沟通，更改用于下一次口述', exact=True).click()
            expect(page.get_by_role('menu')).to_be_visible()
            capture('scenes.png')
            page.keyboard.press('Escape')

            page.get_by_role('button', name='查看与调整', exact=True).click()
            expect(page.locator('.history-detail')).to_be_visible()
            page.set_viewport_size(dict(width=1360, height=1120))
            page.locator('.history-compare-toggle').click()
            expect(page.get_by_role('heading', name='识别稿与当前文本', exact=True)).to_be_visible()
            capture('history.png')

            page.set_viewport_size(dict(width=1360, height=900))
            page.get_by_role('button', name='词典', exact=True).click()
            expect(page.get_by_role('heading', name='个人词典', exact=True)).to_be_visible()
            expect(page.locator('.dictionary-list > li')).to_have_count(9)
            capture('dictionary.png')

            page.get_by_role('button', name='个性化', exact=True).click()
            expect(page.get_by_role('heading', name='个性化', exact=True)).to_be_visible()
            capture('personalization.png')

            page.get_by_role('button', name='设置', exact=True).click()
            expect(page.get_by_role('combobox', name='语音识别供应商', exact=True)).to_be_visible()
            page.get_by_role('combobox', name='文字整理供应商', exact=True).scroll_into_view_if_needed()
            page.get_by_role('combobox', name='文字整理供应商', exact=True).click()
            expect(page.get_by_role('option', name='DeepSeek', exact=True)).to_be_visible()
            capture('providers.png')
            context.close()
        finally:
            browser.close()
    receipt = dict(scope=__doc__, server=server, captures=captures,
                   pageErrors=errors, externalRequests=external)
    (run_dir / 'capture-receipt.json').write_text(json.dumps(receipt, ensure_ascii=False, indent=2), encoding='utf-8')
    assert not errors and not external, receipt
    assert not any(row['alerts'] for row in captures), receipt
    print(json.dumps(receipt, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
