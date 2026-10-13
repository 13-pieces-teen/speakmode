"""Real settings UI with an isolated synthetic IPC bridge; no personal profile.

SQLite and lifecycle behavior are tested in Rust. This checks UI behavior only.
Uses this repository's existing Windows Playwright/Edge fixture and owned server.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import traceback
from urllib.parse import urlparse
from playwright.sync_api import sync_playwright, expect

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('base', ROOT/'scripts/soft-voice-settings-browser-qa.py')
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)

def snapshot(page, out, name):
    panel = page.locator('.app-writing-settings')
    panel.scroll_into_view_if_needed()
    sizes = panel.evaluate('''root => ({width:innerWidth,documentWidth:document.documentElement.scrollWidth,
        overflow:[...root.querySelectorAll('select,input,button,label')].filter(el=>{
            const r=el.getBoundingClientRect();return r.width>0&&(r.left<0||r.right>innerWidth+1);
        }).map(el=>el.getAttribute('aria-label')||el.textContent)})''')
    assert sizes['documentWidth'] <= sizes['width'] and not sizes['overflow'], sizes
    page.get_by_role('switch',name='按应用调整表达',exact=True).scroll_into_view_if_needed()
    page.screenshot(path=str(out/f'{name}-overview.png'))
    page.get_by_role('combobox',name='选择应用',exact=True).scroll_into_view_if_needed()
    page.screenshot(path=str(out/f'{name}-editor.png'))
    return sizes

def run(args):
    args.out.mkdir(parents=True, exist_ok=False)
    results=[];errors=[];external=[]
    report={'scope':'PRODUCTION_UI_SYNTHETIC_IPC_NO_NATIVE_AUDIO_OR_CLOUD','status':'RUNNING','checks':results,'pageErrors':errors,'externalRequests':external}
    def passed(name): results.append({'name':name,'passed':True})
    with base.managed_dev(args.url,args.out) as server, sync_playwright() as p:
        browser=p.chromium.launch(channel='msedge',headless=True)
        report.update(server=server,browserVersion=browser.version)
        try:
            for width,height in [(1280,800),(800,640),(430,800)]:
                tag=f'{width}x{height}'
                data=base.qa.fixture() # Deliberately omit appWriting: old-profile UI compatibility.
                script=base.bridge(data).replace('speakmode-soft-voice-synthetic-preferences','speakmode-app-writing-fixture')
                script=script.replace('softVoiceEnabled:false,keepHistory:true','softVoiceEnabled:false,appWriting:{enabled:true,overrides:[]},keepHistory:true')
                context=browser.new_context(viewport={'width':width,'height':height},locale='zh-CN',reduced_motion='reduce')
                context.add_init_script(script)
                def route(request):
                    if urlparse(request.request.url).hostname not in ('127.0.0.1',None):
                        external.append(request.request.url);request.abort()
                    else: request.continue_()
                context.route('**/*',route)
                page=context.new_page();page.on('pageerror',lambda error:errors.append(str(error)))
                try:
                    page.goto(args.url,wait_until='networkidle');base.open_settings(page)
                    toggle=page.get_by_role('switch',name='按应用调整表达',exact=True)
                    expect(toggle).to_be_checked();passed(tag+' legacy default')
                    app=page.get_by_role('combobox',name='选择应用',exact=True)
                    app.select_option('msedge.exe')
                    page.get_by_role('combobox',name='新应用的表达场景',exact=True).select_option('chat')
                    page.get_by_role('button',name='添加偏好',exact=True).click()
                    edge=page.get_by_role('combobox',name='Edge的表达场景',exact=True)
                    expect(edge).to_have_value('chat');passed(tag+' add browser override')
                    page.evaluate('window.__speakmodeQA.failNextPreference=true')
                    edge.select_option('general')
                    expect(page.locator('[data-setting-feedback="appWriting"]')).to_have_attribute('data-feedback-state','error')
                    expect(edge).to_have_value('chat');passed(tag+' save failure preserves saved scene')
                    edge.select_option('general');expect(edge).to_have_value('general')
                    page.reload(wait_until='networkidle');base.open_settings(page)
                    expect(page.get_by_role('combobox',name='Edge的表达场景',exact=True)).to_have_value('general')
                    passed(tag+' saved scene reloads in fixture')
                    app=page.get_by_role('combobox',name='选择应用',exact=True)
                    app.select_option('msedge.exe');page.get_by_role('button',name='添加偏好',exact=True).click()
                    expect(page.get_by_text('这个应用已有偏好，请在上方修改。',exact=True)).to_be_visible();passed(tag+' duplicate rejected')
                    app.select_option('custom');page.get_by_role('textbox',name='应用程序名',exact=True).fill('C:/private/Notes.exe')
                    page.get_by_role('button',name='添加偏好',exact=True).click()
                    expect(page.get_by_text('请填写程序名，例如 Code.exe，不包含文件路径。',exact=True)).to_be_visible()
                    page.get_by_role('textbox',name='应用程序名',exact=True).fill('Notes.exe')
                    page.get_by_role('combobox',name='新应用的表达场景',exact=True).select_option('document')
                    page.get_by_role('button',name='添加偏好',exact=True).click()
                    expect(page.get_by_role('combobox',name='notes.exe的表达场景',exact=True)).to_have_value('document')
                    passed(tag+' custom name validation and save')
                    toggle.focus();page.keyboard.press('Space');expect(toggle).not_to_be_checked()
                    assert len(page.evaluate('window.__speakmodeQA.fixture.snapshot.preferences.appWriting.overrides'))==2
                    page.keyboard.press('Space');expect(toggle).to_be_checked()
                    passed(tag+' keyboard toggle retains overrides')
                    page.get_by_text('查看不同场景的呈现',exact=True).click()
                    page.get_by_role('combobox',name='示例场景',exact=True).select_option('chat')
                    expect(page.locator('.app-writing-example .settings-example-output ol')).to_have_count(0)
                    page.get_by_role('combobox',name='示例场景',exact=True).select_option('technical')
                    expect(page.locator('.app-writing-example .settings-example-output ol li')).to_have_count(2)
                    passed(tag+' scene examples')
                    snapshot(page,args.out,tag+'-configured');passed(tag+' layout has no overflow')
                    page.get_by_role('button',name='移除notes.exe偏好',exact=True).click()
                    expect(page.get_by_role('combobox',name='notes.exe的表达场景',exact=True)).to_have_count(0)
                    expect(page.get_by_role('combobox',name='选择应用',exact=True)).to_be_focused()
                    passed(tag+' remove restores focus')
                    page.get_by_role('button',name='恢复默认设置',exact=True).click()
                    expect(page.locator('[data-setting-feedback="reset"]')).to_have_attribute('data-feedback-state','saved')
                    expect(page.get_by_role('combobox',name='Edge的表达场景',exact=True)).to_have_count(0)
                    expect(toggle).to_be_checked();passed(tag+' reset defaults')
                finally: context.close()
            for scene,label in [('chat','聊天整理'),('document','文档整理'),('technical','技术整理'),('general','正在整理')]:
                data=base.qa.fixture()
                data['snapshot']['session']={'sessionId':'synthetic-scene-session','generation':7,'state':'rewriting',
                    'modeSnapshot':{'modeId':'auto','name':'自动','baseMode':'auto','rules':'','revision':1},
                    'writingScene':scene,'captions':[],'workerReleased':False}
                context=browser.new_context(viewport={'width':460,'height':180},locale='zh-CN',reduced_motion='reduce')
                context.add_init_script(base.qa.BRIDGE.replace('__FIXTURE__',json.dumps(data,ensure_ascii=False)))
                context.route('**/*',route)
                page=context.new_page();page.on('pageerror',lambda error:errors.append(str(error)))
                try:
                    page.goto(args.url+'?window=recorder',wait_until='networkidle')
                    expect(page.locator('.recorder-phase')).to_have_text(label)
                    expect(page.get_by_role('button',name='本次 Mode：自动',exact=True)).to_be_disabled()
                    page.screenshot(path=str(args.out/f'recorder-{scene}.png'))
                    passed('recorder '+scene+' scene and frozen Mode')
                finally: context.close()
            assert not errors and not external
            report['status']='PASS'
        except Exception as error:
            report.update(status='FAIL',failure=str(error),traceback=traceback.format_exc())
        finally:
            browser.close()
    (args.out/'receipt.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({'status':report['status'],'checks':len(results),'receipt':str(args.out/'receipt.json')},ensure_ascii=False))
    return 0 if report['status']=='PASS' else 1

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--url',default='http://127.0.0.1:4187');parser.add_argument('--out',required=True,type=Path)
    raise SystemExit(run(parser.parse_args()))
