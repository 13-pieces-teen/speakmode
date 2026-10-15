"""Production scene UI with isolated synthetic IPC; no native capture or cloud quality claim."""
import hashlib
import importlib.util
import json
import traceback
from pathlib import Path
from urllib.parse import urlparse
from playwright.sync_api import expect

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('candidate_qa', ROOT / 'scripts/candidate-review-browser-qa.py')
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)
SCENES = [('auto', '自动'), ('chat', '聊天消息'), ('document', '文档写作'), ('email', '邮件沟通'), ('technical', '技术沟通')]


def fixture(legacy=False, recorder=False, personal_collision=False):
    data = qa.fixture()
    data['details'][0]['output']['modeSnapshot'] = dict(modeId='auto',name='自动',baseMode='auto',rules='',revision=1)
    data['details'][0]['output']['writingReceipt'] = dict(scene='document',language='english',contextUsed=True,contractVersion='synthetic-scene-v1')
    snap = data['snapshot']
    snap['preferences']['defaultModeId'] = 'light' if legacy else 'auto'
    # Deliberately omit new fields: existing preference payloads must get defaults.
    snap['preferences']['appWriting'] = {'enabled': True, 'overrides': []}
    snap['modes'] = [dict(id=key, name=name, baseMode='auto', rules='', revision=1, builtin=True) for key, name in SCENES]
    snap['modes'] += [dict(id='light', name='轻整理', baseMode='light', rules='', revision=1, builtin=True), dict(id='structure', name='结构', baseMode='structure', rules='', revision=1, builtin=True)]
    snap['modes'].append(dict(id='synthetic-custom', name='邮件沟通' if personal_collision else '项目周报', baseMode='structure', rules='只组织已经说出的项目进展。', revision=3, builtin=False))
    if recorder:
        snap['session'] = dict(sessionId='scene-session', generation=7, state='recording', modeSnapshot=dict(modeId='auto', name='自动', baseMode='auto', rules='', revision=1), workerReleased=False, captureReady=True, providerReady=True, writingScene='document', writingLanguage='english', inputContextStatus='available')
    return data


EXTRA = r"""
(() => {
 const q=window.__speakmodeQA,old=window.__TAURI_INTERNALS__.invoke,copy=x=>structuredClone(x);
 q.emitSession=patch=>{Object.assign(q.fixture.snapshot.session,patch);q.fixture.snapshot.sequence++;q.channel.onmessage({sequence:q.fixture.snapshot.sequence,kind:'session',payload:copy(q.fixture.snapshot.session)});};
 window.__TAURI_INTERNALS__.invoke=async(command,args={})=>{
  if(command==='subscribe_state'){q.channel=args.channel;return old(command,args);}
  if(command!=='app_command')return old(command,args);
  const {name,payload}=args.request;
  if(!['mode.save','session.selectMode'].includes(name))return old(command,args);
  q.ledger.push({name,payload:copy(payload)});
  if(name==='mode.save'){
   const d=payload.draft;const mode={id:d.id??'created-scenario',name:d.name,baseMode:d.baseMode,rules:d.rules,revision:(d.expectedRevision??0)+1,builtin:false};
   q.fixture.snapshot.modes=q.fixture.snapshot.modes.filter(m=>m.id!==mode.id).concat(mode);q.fixture.snapshot.sequence++;return copy(mode);
  }
  const mode=q.fixture.snapshot.modes.find(m=>m.id===payload.modeId);
  q.emitSession({modeSnapshot:{modeId:mode.id,name:mode.name,baseMode:mode.baseMode,rules:mode.rules,revision:mode.revision}});return null;
 };
})();
"""


def settings(page):
    page.get_by_role('navigation', name='主导航').get_by_role('button', name='设置', exact=True).click()
    expect(page.get_by_role('heading', name='设置', exact=True)).to_be_visible()


def preferences(page):
    return page.evaluate('window.__speakmodeQA.fixture.snapshot.preferences')


def run(browser, args, rows, errors, external):
    def case(name, action, width=1280, height=900, legacy=False, recorder=False, personal_collision=False):
        if args.case and args.case != name:
            return
        row = {'name': name, 'status': 'RUNNING'}
        rows.append(row)
        context = browser.new_context(viewport={'width': width, 'height': height}, locale='zh-CN', reduced_motion='reduce')
        context.add_init_script(qa.ui.BRIDGE.replace('__FIXTURE__', json.dumps(fixture(legacy, recorder, personal_collision), ensure_ascii=False)) + qa.EXTRA.replace('__AFTER__', json.dumps(qa.AFTER, ensure_ascii=False)) + EXTRA)
        def route(r):
            if urlparse(r.request.url).hostname not in ('127.0.0.1', None):
                external.append(r.request.url); r.abort()
            else:
                r.continue_()
        context.route('**/*', route)
        page = context.new_page()
        page.on('pageerror', lambda error: errors.append({'case': name, 'error': str(error)}))
        try:
            page.goto(args.url + ('?window=recorder' if recorder else ''), wait_until='networkidle')
            if not recorder:
                expect(page.get_by_role('heading', name='最近一次口述', exact=True)).to_be_visible()
            row['observations'] = action(page)
            row['measurement'] = qa.ui.measurements(page)
            assert not row['measurement']['overflow'], row['measurement']['overflow']
            row['ledger'] = page.evaluate('window.__speakmodeQA.ledger')
            assert not [c for c in row['ledger'] if c['name'].startswith(('delivery.', 'dictionary.')) and c['name'] != 'dictionary.list']
            row['status'] = 'PASS'
        except Exception as error:
            row.update(status='FAIL', error=str(error), traceback=traceback.format_exc())
        finally:
            page.screenshot(path=str(args.out / (name + '.png')))
            (args.out / (name + '.html')).write_text(page.content(), encoding='utf-8')
            context.close()

    def catalog(page):
        choices = page.get_by_role('radiogroup', name='新口述默认场合')
        expect(choices.get_by_role('radio')).to_have_count(6)
        for key, name in SCENES[1:]:
            choices.get_by_role('radio', name=name, exact=False).click()
            page.wait_for_function('(id)=>window.__speakmodeQA.fixture.snapshot.preferences.defaultModeId===id', arg=key)
            assert qa.calls(page, 'mode.setDefault')[-1]['payload']['modeId'] == key
        expect(choices).not_to_contain_text('轻整理')
        return {'fiveScenarioIdsPreserved': True, 'customScenarioAvailable': True}
    case('01-shared-scene-catalog', catalog)

    def privacy_language(page):
        settings(page)
        context_toggle = page.get_by_role('switch', name='输入位置感知', exact=True)
        expect(context_toggle).not_to_be_checked()
        expect(page.locator('#input-context-privacy')).to_contain_text('不采集整页或截图')
        page.get_by_role('combobox', name='成稿语言', exact=True).select_option('english')
        expect(page.get_by_role('combobox', name='成稿语言', exact=True)).to_have_value('english')
        context_toggle.click(); expect(context_toggle).to_be_checked()
        saved = preferences(page)
        assert saved['language'] == 'auto' and saved['appWriting']['outputLanguage'] == 'english'
        assert saved['appWriting']['inputContextEnabled'] is True
        page.evaluate('window.__speakmodeQA.failNextPreference=true')
        context_toggle.click()
        expect(page.locator('[data-setting-feedback="appWriting"]')).to_contain_text('模拟保存失败')
        expect(context_toggle).to_be_checked()
        return {'optInDefaultOff': True, 'languageSeparateFromAsr': True, 'failedSaveKeepsSetting': True}
    case('02-context-opt-in-and-language', privacy_language)

    def exclusions(page):
        settings(page); page.locator('.input-context-exclusions summary').click()
        field = page.get_by_role('textbox', name='程序名', exact=True)
        field.fill('C:\\private\\Code.exe')
        page.get_by_role('button', name='排除此应用', exact=True).click()
        expect(page.locator('#input-context-exclusion-error')).to_contain_text('不包含文件路径')
        assert not qa.calls(page, 'preferences.update')
        field.fill(' Code.EXE '); page.get_by_role('button', name='排除此应用', exact=True).click()
        expect(page.get_by_role('list', name='不读取输入上下文的应用')).to_contain_text('code.exe')
        assert preferences(page)['appWriting']['excludedApplications'] == ['code.exe']
        field.fill('code.exe'); page.get_by_role('button', name='排除此应用', exact=True).click()
        expect(page.locator('#input-context-exclusion-error')).to_contain_text('已被排除')
        page.get_by_role('button', name='取消排除VS Code', exact=True).click()
        expect(page.get_by_role('list', name='不读取输入上下文的应用')).to_have_count(0)
        expect(page.locator('#input-context-exclusion-error')).to_be_empty()
        return {'programKeyValidated': True, 'exclusionAddRemove': True}
    case('03-context-exclusions-narrow', exclusions, 760)

    def override(page):
        settings(page)
        page.get_by_role('combobox', name='选择应用', exact=True).select_option('chrome.exe')
        page.get_by_role('combobox', name='新应用的表达场景', exact=True).select_option('email')
        page.get_by_role('button', name='添加偏好', exact=True).click()
        expect(page.get_by_role('combobox', name='Chrome的表达场景', exact=True)).to_have_value('email')
        page.get_by_role('switch', name='输入位置感知', exact=True).click()
        page.get_by_role('switch', name='自动匹配应用场合', exact=True).click()
        saved = preferences(page)['appWriting']
        assert saved['enabled'] is False and saved['inputContextEnabled'] is True
        assert saved['overrides'] == [{'application': 'chrome.exe', 'scene': 'email'}]
        return {'emailOverride': True, 'contextAndAppMatchingIndependent': True}
    case('04-app-override-preserved', override)

    def legacy(page):
        choices = page.get_by_role('radiogroup', name='新口述默认场合')
        expect(choices.get_by_role('radio', name='轻整理', exact=False)).to_be_checked()
        before = page.evaluate('JSON.stringify(window.__speakmodeQA.fixture.details[0])')
        choices.get_by_role('radio', name='文档写作', exact=False).click()
        expect(choices.get_by_role('radio', name='轻整理', exact=False)).to_have_count(0)
        assert page.evaluate('JSON.stringify(window.__speakmodeQA.fixture.details[0])') == before
        settings(page)
        expect(page.get_by_role('combobox', name='新口述默认场合', exact=True)).to_contain_text('文档写作')
        return {'legacyDefaultKeptUntilUserChange': True, 'historyUnchanged': True}
    case('05-legacy-mode-compatibility', legacy, legacy=True)

    def custom(page):
        page.get_by_role('navigation', name='主导航').get_by_role('button', name='场合', exact=True).click()
        tile = page.locator('.builtin-mode-tile').filter(has=page.get_by_text('文档写作', exact=True))
        tile.get_by_role('button', name='自定义表达', exact=True).click()
        expect(page.get_by_role('heading', name='新建自定义场合', exact=True)).to_be_visible()
        page.get_by_role('textbox', name='名称', exact=True).fill('面试自我介绍')
        expect(page.get_by_role('textbox', name='整理要求', exact=True)).to_contain_text('连贯的自然段')
        page.get_by_role('radio', name='自然段', exact=True).check()
        page.get_by_role('button', name='保存场合', exact=True).click()
        expect(page.get_by_role('heading', name='使用场合', exact=True)).to_be_visible()
        payload = qa.calls(page, 'mode.save')[-1]['payload']['draft']
        assert payload['baseMode'] == 'light' and payload['name'] == '面试自我介绍'
        assert '连贯的自然段' in payload['rules']
        return {'customSceneRules': True, 'organizationPreferencePreservesNativeBase': True}
    case('06-custom-scene-creation', custom)

    def history(page):
        page.get_by_role('button', name='查看与调整', exact=True).click()
        page.get_by_role('button', name='换种整理', exact=True).click()
        page.get_by_role('radiogroup', name='本条候选的使用场合').get_by_role('radio', name='邮件沟通', exact=False).click()
        expect(page.get_by_role('combobox', name='本条成稿语言', exact=True)).to_be_visible()
        page.get_by_role('button', name='生成候选', exact=True).click()
        expect(page.get_by_role('heading', name='新候选', exact=True)).to_be_visible()
        assert qa.calls(page, 'history.rewrite')[-1]['payload']['modeId'] == 'email'
        assert not qa.calls(page, 'revision.adopt')
        return {'historyUsesSceneId': True, 'confirmationFlowRetained': True, 'noAutoAdopt': True}
    case('07-history-scene-candidate', history)

    def recorder_choice(page):
        trigger = page.get_by_role('button', name='本次场合：自动 · 文档 · 英文', exact=True)
        expect(trigger).to_be_visible()
        assert trigger.locator('span').evaluate('e=>e.scrollWidth<=e.clientWidth'), 'Current scene/language label is clipped'
        trigger.click()
        options = page.get_by_role('group', name='本次使用场合', exact=True)
        expect(options.get_by_role('button')).to_have_count(6)
        options.get_by_role('button', name='邮件沟通', exact=True).click()
        payload = qa.calls(page, 'session.selectMode')[-1]['payload']
        assert payload['modeId'] == 'email' and payload['expectedGeneration'] == 7 and payload['revision'] == 1
        assert page.evaluate('window.__speakmodeQA.fixture.snapshot.session.modeSnapshot.baseMode') == 'auto'
        expect(page.get_by_role('button', name='本次场合：邮件沟通 · 英文', exact=True)).to_be_visible()
        return {'sameSceneCatalog': True, 'sessionIdentityPreserved': True, 'reportedLanguageShown': True}
    case('08-recorder-scene-selection', recorder_choice, 440, 420, recorder=True)

    def status(page):
        expect(page.get_by_text('已获取当前输入框上下文', exact=True)).to_be_visible()
        page.evaluate("window.__speakmodeQA.emitSession({state:'rewriting',writingScene:'email'})")
        expect(page.get_by_text('邮件整理', exact=True)).to_be_visible()
        page.evaluate("window.__speakmodeQA.emitSession({state:'ready',rewriteFallback:'meaningRisk',workerReleased:true,writingScene:null})")
        expect(page.get_by_text('已获取当前输入框上下文', exact=True)).to_have_count(0)
        expect(page.get_by_role('button', name='本次场合：自动', exact=True)).to_be_visible()
        return {'availableStatusBoundToSession': True, 'fallbackClearsSceneContextClaim': True}
    case('09-recorder-context-status', status, 440, 420, recorder=True)

    def reference(page):
        page.get_by_role('button', name='查看与调整', exact=True).click()
        expect(page.locator('.history-writing-reference')).to_have_text('成稿参考：文档写作 · 英文成稿 · 已参考输入位置')
        expect(page.locator('.history-tools .mode-badge')).to_have_text('自动 · 文档写作')
        page.screenshot(path=str(args.out/'10-history-writing-reference-available.png'))
        page.evaluate("window.__speakmodeQA.fixture.details[0].output.rewriteFallback='meaningRisk';window.__speakmodeQA.advance()")
        expect(page.locator('.history-writing-reference')).to_have_count(0)
        expect(page.locator('.history-tools .mode-badge')).to_contain_text('识别稿')
        return {'receiptMetadataDisplayed': True, 'fallbackDoesNotClaimContext': True}
    case('10-history-writing-reference', reference)

    def existing_name(page):
        page.get_by_role('navigation', name='主导航').get_by_role('button', name='场合', exact=True).click()
        page.locator('.personal-mode-row').get_by_role('button', name='编辑', exact=True).click()
        name = page.get_by_role('textbox', name='名称', exact=True)
        expect(name).to_have_value('邮件沟通')
        name.fill('技术沟通')
        page.get_by_role('button', name='保存场合', exact=True).click()
        expect(page.locator('#mode-name-error')).to_contain_text('同名场合')
        assert not qa.calls(page, 'mode.save')
        name.fill(' 邮件沟通 ')
        page.get_by_role('textbox', name='整理要求', exact=True).fill('按请求与待确认事项组织邮件，不补写收件人。')
        page.get_by_role('button', name='保存场合', exact=True).click()
        expect(page.get_by_role('heading', name='使用场合', exact=True)).to_be_visible()
        saved = qa.calls(page, 'mode.save')[-1]['payload']['draft']
        assert saved['id'] == 'synthetic-custom' and saved['expectedRevision'] == 3
        assert saved['name'] == '邮件沟通' and saved['baseMode'] == 'structure'
        assert saved['rules'] == '按请求与待确认事项组织邮件，不补写收件人。'
        page.get_by_role('button', name='新建自定义场合', exact=True).click()
        name = page.get_by_role('textbox', name='名称', exact=True)
        for duplicate in ['邮件沟通', '技术沟通']:
            name.fill(duplicate)
            page.get_by_role('button', name='保存场合', exact=True).click()
            expect(page.locator('#mode-name-error')).to_contain_text('同名场合')
        assert len(qa.calls(page, 'mode.save')) == 1
        return {'existingNameAndIdPreserved': True, 'renamedBuiltinCollisionBlocked': True, 'newBuiltinAndPersonalDuplicatesBlocked': True}
    case('11-existing-personal-scene-name', existing_name, personal_collision=True)


if __name__ == '__main__':
    qa.run = run
    code = qa.main()
    import sys
    receipt_path = Path(sys.argv[sys.argv.index('--out') + 1]) / 'receipt.json'
    receipt = json.loads(receipt_path.read_text(encoding='utf-8'))
    receipt['scope'] = __doc__
    receipt['sharedHarnessSha256'] = receipt['scriptSha256']
    receipt['scriptSha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    receipt_path.write_text(json.dumps(receipt, ensure_ascii=False, indent=2), encoding='utf-8')
    raise SystemExit(code)
