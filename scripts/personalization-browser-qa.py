"""SpeakMode personalization React acceptance with isolated synthetic desktop IPC.

Runs the built frontend in headless Edge. External requests are blocked. Frozen
fake configuration and fake model responses exercise visible state and IPC
contracts only. This is not native storage, model quality, audio or delivery
evidence; no real profile, credential, microphone or clipboard is accessed.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import traceback
from urllib.parse import urlparse
from playwright.sync_api import sync_playwright, expect

spec = importlib.util.spec_from_file_location('personalization_base', Path(__file__).with_name('restore-delivery-browser-qa.py'))
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)

STYLE = dict(sentenceStyle='default', tone='natural', keepEnglishTerms=False, fewerCourtesies=False, paragraphs=False, lists=False)


def fixture(empty=False):
    data = qa.fixture()
    data['snapshot']['preferences']['appWriting'] = dict(enabled=True, overrides=[], inputContextEnabled=False, excludedApplications=[], outputLanguage='asSpoken')
    data['snapshot']['preferences']['softVoiceEnabled'] = False
    data['snapshot']['provider']['config']['rewriteProvider'] = 'custom'
    data['snapshot']['modes'].append(dict(id='document',name='文档写作',baseMode='light',rules='保留原意',revision=1,builtin=True))
    data['config'] = dict(version=1, enabled=not empty, activeProfileId=None if empty else 'qa-developer',
        profiles=[] if empty else [dict(id='qa-developer', name='AI 后端开发', profession='软件开发', domain='AI／后端', description='合成测试背景，非用户资料。',
            tasks=['技术讨论', '项目进展'], audience='同事', scenes=['document', 'technical'], instructions='')], preferences=dict(STYLE, paragraphs=not empty), sceneOverrides=[])
    return data


EXTENSION = r"""
   if(name==='usage.today')return copy(fixture.snapshot.usage);
   if(name==='usage.total'){const {date,...totals}=fixture.snapshot.usage;return copy(totals);}
   if(name==='personalization.get')return {...copy(fixture.config),capability:{supported:true,protocol:'personalization-v1',modelQuality:'notEvaluated'}};
   if(name==='personalization.save'){
    if(Object.keys(payload).sort().join(',')!=='config,expectedVersion,requestId')throw new Error('Unexpected save payload');
    if(window.__speakmodeQA.saveError)throw new Error('合成保存失败，草稿需要保留。');
    if(window.__speakmodeQA.saveConflict){window.__speakmodeQA.saveConflict=false;fixture.config.version++;throw {code:'revisionConflict',message:'合成版本冲突'};}
    if(payload.expectedVersion!==fixture.config.version)throw {code:'revisionConflict',message:'合成版本冲突'};
    fixture.config=copy(payload.config);fixture.config.version++;
    return copy(fixture.config);
   }
   if(name==='personalization.compare'){
    if(Object.keys(payload).sort().join(',')!=='modeId,operationId,requestId,source')throw new Error('Unexpected comparison payload');
    if(!/^[a-z0-9-]+$/.test(payload.operationId))throw new Error('Missing comparison identity');
    window.__speakmodeQA.compareOperation=payload.operationId;
    if(window.__speakmodeQA.deferCompare)await new Promise(resolve=>{window.__speakmodeQA.releaseCompare=resolve;});
    const failed=window.__speakmodeQA.failPersonalized;
    const result={id:payload.operationId,comparisonId:payload.operationId,sourceKind:payload.source.kind,sourceRevisionId:'qa-source',
      modeSnapshot:fixture.detail.output.modeSnapshot,scene:'document',provider:'synthetic',model:'frozen-fixture',
      baseline:{status:'success',text:'登录页已经完成。支付页还在联调，预计周五完成。'},
      personalized:failed?{status:'failed',errorCode:'synthetic_failure'}:{status:'success',text:'登录页已经完成。\n\n支付页还在联调，预计周五完成。'},
      valid:!failed,identical:false,reference:{version:fixture.config.version,profileId:fixture.config.activeProfileId,profileName:'AI 后端开发',scene:'document',background:{profession:'软件开发',domain:'AI／后端',description:'合成参考，非用户资料',tasks:['技术讨论'],audience:'同事'},rules:[{key:'paragraphs',text:'按已有话题分段。',origin:'global'}],omitted:[],referenceCleared:false}};
    window.__speakmodeQA.result=result;
    return result;
   }
   if(name==='personalization.cancel'){if(payload.operationId!==window.__speakmodeQA.compareOperation)throw new Error('Wrong cancellation identity');window.__speakmodeQA.cancelled=true;return null;}
   if(name==='personalization.feedback'){if(Object.keys(payload).sort().join(',')!=='comparisonId,requestId,verdict')throw new Error('Unexpected feedback payload');window.__speakmodeQA.verdict=payload.verdict;return null;}
   if(name==='personalization.stats'){
    if(window.__speakmodeQA.statsError)throw new Error('合成统计读取失败');
    const result=window.__speakmodeQA.result,valid=result?.valid?1:0,verdict=window.__speakmodeQA.verdict;
    return {window:payload.window,since:payload.window==='week'?'2026-10-16':null,normal:{total:0,personalized:0},preview:{total:0,personalized:0},
     comparisons:{total:result?1:0,valid,failed:result&&!valid?1:0,fallback:0,cancelled:window.__speakmodeQA.cancelled?1:0,pending:0,feedback:{better:verdict==='better'?1:0,same:verdict==='same'?1:0,worse:verdict==='worse'?1:0,total:verdict?1:0}},scenes:[],records:[]};
   }
"""


def bridge(data):
    return qa.BRIDGE.replace("   if(name==='state.snapshot')", EXTENSION + "   if(name==='state.snapshot')").replace('__FIXTURE__', json.dumps(data, ensure_ascii=False))


def layout(page, width):
    page.set_viewport_size(dict(width=width, height=900))
    result=page.evaluate("""()=>{
      const shown=el=>{const r=el.getBoundingClientRect();return r.width>0&&r.height>0&&getComputedStyle(el).visibility!=='hidden';};
      const offscreen=[...document.querySelectorAll('.personalization-page button,.personalization-page input,.personalization-page textarea,.personalization-page [role="combobox"]')].filter(shown).map(el=>({label:el.getAttribute('aria-label')||el.textContent,box:el.getBoundingClientRect().toJSON()})).filter(item=>item.box.left < -1 || item.box.right>innerWidth+1);
      return {width:innerWidth,documentWidth:document.documentElement.scrollWidth,offscreen};
    }""")
    assert result['documentWidth'] <= width+1 and not result['offscreen'], result
    return result


def open_editor(page, tab='工作背景'):
    page.get_by_role('button', name='编辑画像', exact=True).click()
    page.get_by_role('tab', name=tab, exact=True).click()


def calls(page, name):
    return [item for item in qa.command_ledger(page) if item['name']==name]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url',required=True)
    parser.add_argument('--out',required=True,type=Path)
    args=parser.parse_args()
    if urlparse(args.url).hostname!='127.0.0.1' or not args.out.is_absolute():
        parser.error('Use loopback URL and a fresh absolute evidence directory')
    args.out.mkdir(parents=True,exist_ok=False)
    rows,errors,external=[],[],[]
    receipt=dict(scope=__doc__,cases=rows,pageErrors=errors,externalRequests=external)
    receipt['sourceSha256']={name:hashlib.sha256((qa.ROOT/name).read_bytes()).hexdigest() for name in ['app/src/pages/PersonalizationPage.tsx','app/src/pages/personalization.css','app/src/services/personalization.ts','scripts/personalization-browser-qa.py']}
    receipt['distSha256']={str(path.relative_to(qa.ROOT)):hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted((qa.ROOT/'app/dist').rglob('*')) if path.is_file()}
    cases=['empty-skip-global-style','save-failure-navigation-draft','revision-conflict','scene-override','three-identity-limit','unicode-limit','history-source','cleared-history-reference','scope-and-template-opt-in','scope-availability','comparison-success-feedback','comparison-partial-failure','cancel-late-success','disable-during-comparison','delete-reference','statistics-independent-failure','keyboard-layout']
    with qa.managed_preview(args.url,args.out,True) as server,sync_playwright() as playwright:
        receipt['server']=server
        browser=playwright.chromium.launch(channel='msedge',headless=True)
        receipt['browserVersion']=browser.version
        try:
            for name in cases:
                context=browser.new_context(viewport=dict(width=1280,height=900),locale='zh-CN',reduced_motion='reduce')
                data=fixture(empty=name=='empty-skip-global-style')
                if name=='scope-availability':data['config']['preferences']=dict(STYLE)
                if name=='three-identity-limit':
                    for index in [2,3]:
                        profile=json.loads(json.dumps(data['config']['profiles'][0]));profile['id']=f'qa-profile-{index}';profile['name']=f'测试身份 {index}';data['config']['profiles'].append(profile)
                if name=='cleared-history-reference':
                    data['detail']['output']['writingReceipt']=dict(scene='document',language='asSpoken',contextUsed=False,contractVersion='synthetic',personalization=dict(version=1,profileId=None,profileName=None,scene='document',background=None,rules=[],omitted=[],referenceCleared=True))
                context.add_init_script(bridge(data))
                def route(route):
                    if urlparse(route.request.url).hostname not in ('127.0.0.1',None):external.append(route.request.url);route.abort()
                    else:route.continue_()
                context.route('**/*',route)
                page=context.new_page();page.on('pageerror',lambda error:errors.append(str(error)))
                row=dict(name=name,status='RUNNING');rows.append(row)
                try:
                    page.goto(args.url,wait_until='networkidle')
                    page.get_by_role('button',name='个性化',exact=True).click()
                    expect(page.get_by_role('heading',name='个性化',exact=True)).to_be_visible()
                    expect(page.get_by_role('heading',name='使用记录',exact=True)).to_be_visible()
                    if name=='empty-skip-global-style':
                        expect(page.get_by_text('还没有对照评价。试写一段话，再告诉我们哪份更符合你。',exact=True)).to_be_visible()
                        page.get_by_role('button',name='稍后设置',exact=True).click()
                        expect(page.get_by_role('heading',name='个性化',exact=True)).not_to_be_visible()
                        page.get_by_role('button',name='个性化',exact=True).click()
                        page.get_by_role('button',name='先只设置表达习惯',exact=True).click()
                        page.get_by_role('radio',name=re.compile('更紧凑')).check()
                        page.get_by_role('switch').check()
                        page.get_by_role('button',name='保存修改',exact=True).click()
                        expect(page.locator('.personalization-notice')).to_contain_text('已保存')
                        assert calls(page,'personalization.save')[-1]['payload']['config']['profiles']==[]
                    elif name=='save-failure-navigation-draft':
                        open_editor(page)
                        field=page.get_by_role('textbox',name=re.compile('^身份名称'))
                        field.fill('我的草稿身份')
                        page.evaluate('window.__speakmodeQA.saveError=true')
                        page.get_by_role('button',name='保存修改',exact=True).click()
                        expect(page.get_by_role('alert')).to_contain_text('合成保存失败')
                        expect(field).to_have_value('我的草稿身份')
                        page.get_by_role('button',name='首页',exact=True).click()
                        guard=page.get_by_role('alertdialog',name='保存这次修改？',exact=True)
                        expect(guard).to_be_visible();guard.get_by_role('button',name='继续编辑',exact=True).click()
                        expect(field).to_have_value('我的草稿身份')
                        page.evaluate('window.__speakmodeQA.saveError=false')
                        page.get_by_role('button',name='保存修改',exact=True).click()
                        expect(page.locator('.personalization-notice')).to_contain_text('已保存')
                    elif name=='scope-and-template-opt-in':
                        open_editor(page,'作用范围')
                        page.get_by_role('checkbox',name='聊天',exact=True).check()
                        page.get_by_role('tab',name='表达习惯',exact=True).click()
                        expect(page.get_by_role('checkbox',name=re.compile('^保留常见英文术语'))).not_to_be_checked()
                        page.get_by_role('button',name='将建议加入当前草稿',exact=True).click()
                        expect(page.get_by_role('checkbox',name=re.compile('^保留常见英文术语'))).to_be_checked()
                        page.get_by_role('button',name='保存修改',exact=True).click()
                        expect(page.locator('.personalization-notice')).to_contain_text('已保存')
                        config=calls(page,'personalization.save')[-1]['payload']['config'];assert 'chat' in config['profiles'][0]['scenes'];assert config['preferences']['keepEnglishTerms']
                        page.get_by_role('switch').uncheck();page.get_by_role('button',name='保存修改',exact=True).click()
                        expect(page.locator('.personalization-notice')).to_contain_text('已停用画像')
                        expect(page.get_by_role('button',name='对比整理',exact=True)).to_be_disabled()
                    elif name=='scope-availability':
                        expect(page.get_by_role('button',name='对比整理',exact=True)).to_be_disabled()
                        expect(page.get_by_text('通用场合目前没有可参考的画像内容。可选择身份已启用的场合，或保存一条通用表达习惯。',exact=True)).to_be_visible()
                        page.get_by_role('combobox',name='试写场合',exact=True).click()
                        page.get_by_role('option',name='文档写作',exact=True).click()
                        expect(page.get_by_role('button',name='对比整理',exact=True)).to_be_enabled()
                    elif name=='revision-conflict':
                        open_editor(page)
                        field=page.get_by_role('textbox',name=re.compile('^身份名称'));field.fill('版本冲突时保留的草稿')
                        page.evaluate('window.__speakmodeQA.saveConflict=true')
                        page.get_by_role('button',name='保存修改',exact=True).click()
                        expect(page.get_by_text('画像已在别处更新。草稿已保留；请读取最新版本后核对，再保存。',exact=True)).to_be_visible()
                        expect(field).to_have_value('版本冲突时保留的草稿')
                        page.get_by_text('查看最新设置，再决定是否覆盖',exact=True).click()
                        page.get_by_role('button',name='确认用当前草稿覆盖 v2',exact=True).click()
                        page.get_by_role('button',name='保存修改',exact=True).click()
                        expect(page.locator('.personalization-notice')).to_contain_text('已保存')
                        assert [item['payload']['expectedVersion'] for item in calls(page,'personalization.save')]==[1,2]
                    elif name=='scene-override':
                        open_editor(page,'表达习惯')
                        page.get_by_role('combobox',name='表达习惯适用范围',exact=True).click()
                        page.get_by_role('option',name='邮件场合覆盖',exact=True).click()
                        page.get_by_role('button',name='为邮件单独设置',exact=True).click()
                        page.get_by_role('radio',name=re.compile('完整句子')).check()
                        page.get_by_role('button',name='保存修改',exact=True).click()
                        expect(page.locator('.personalization-notice')).to_contain_text('已保存')
                        config=calls(page,'personalization.save')[-1]['payload']['config']
                        assert config['preferences']['sentenceStyle']=='default';assert config['sceneOverrides'][0]['scene']=='email';assert config['sceneOverrides'][0]['preferences']['sentenceStyle']=='complete'
                    elif name=='three-identity-limit':
                        open_editor(page)
                        expect(page.get_by_role('combobox',name='添加身份',exact=True)).to_be_disabled()
                        expect(page.get_by_text('3 / 3',exact=True)).to_be_visible()
                    elif name=='unicode-limit':
                        open_editor(page)
                        field=page.get_by_role('textbox',name=re.compile('^工作描述'))
                        field.fill('😀'*300)
                        expect(page.get_by_role('button',name='保存修改',exact=True)).to_be_enabled()
                        field.fill('😀'*301)
                        expect(page.get_by_role('button',name='保存修改',exact=True)).to_be_disabled()
                        expect(field).to_have_value('😀'*301)
                        expect(page.get_by_role('alert')).to_contain_text('工作描述最多 300 字')
                    elif name=='history-source':
                        page.get_by_role('combobox',name='试写内容来源',exact=True).click()
                        page.get_by_role('option',name='从历史选择',exact=True).click()
                        page.get_by_role('combobox',name='用于对照的历史记录',exact=True).click()
                        page.get_by_role('option',name=re.compile('🙂进行测试')).click()
                        page.get_by_role('button',name='对比整理',exact=True).click()
                        expect(page.locator('.personalization-result-grid')).to_be_visible()
                        source=calls(page,'personalization.compare')[-1]['payload']['source'];assert source==dict(kind='history',sessionId='synthetic-session',sourceRevisionId='synthetic-source-r1')
                    elif name=='cleared-history-reference':
                        page.get_by_role('button',name='历史记录',exact=True).click()
                        page.locator('.history-record-button').first.click()
                        expect(page.get_by_text('参考详情已清除。',exact=True)).to_be_visible()
                        expect(page.locator('.personalization-background')).to_have_count(0)
                    elif name.startswith('comparison'):
                        if name=='comparison-partial-failure':page.evaluate('window.__speakmodeQA.failPersonalized=true')
                        page.get_by_role('button',name='对比整理',exact=True).click()
                        expect(page.locator('.personalization-result-grid')).to_be_visible()
                        expect(page.locator('.personalization-result-grid article').first).to_contain_text('已完成')
                        if name=='comparison-partial-failure':
                            expect(page.locator('.personalization-result-grid article').nth(1)).to_contain_text('失败')
                            expect(page.get_by_role('button',name='画像更符合我',exact=True)).to_have_count(0)
                        else:
                            page.get_by_text(re.compile('查看本次参考信息')).click()
                            expect(page.locator('.personalization-background')).to_contain_text('软件开发')
                            page.get_by_role('button',name='画像更符合我',exact=True).click()
                            expect(page.get_by_role('button',name='画像更符合我',exact=True)).to_have_attribute('aria-pressed','true')
                            assert calls(page,'personalization.feedback')[-1]['payload']['verdict']=='better'
                            page.screenshot(path=str(args.out/'comparison-reference.png'))
                        assert len(calls(page,'personalization.compare'))==1
                    elif name=='cancel-late-success':
                        page.evaluate('window.__speakmodeQA.deferCompare=true')
                        page.get_by_role('button',name='对比整理',exact=True).click()
                        expect(page.get_by_role('button',name='取消对照',exact=True)).to_be_visible()
                        page.get_by_role('button',name='首页',exact=True).click()
                        guard=page.get_by_role('alertdialog',name='保存这次修改？',exact=True)
                        expect(guard.get_by_role('button',name='放弃修改',exact=True)).to_be_disabled()
                        guard.get_by_role('button',name='继续编辑',exact=True).click()
                        page.get_by_role('button',name='取消对照',exact=True).click()
                        expect(page.get_by_role('button',name='正在取消…',exact=True)).to_be_disabled()
                        page.evaluate('window.__speakmodeQA.releaseCompare()')
                        expect(page.locator('.personalization-notice')).to_contain_text('对照已取消')
                        expect(page.locator('.personalization-result-grid')).to_have_count(0)
                        assert calls(page,'personalization.cancel')[-1]['payload']['operationId']==calls(page,'personalization.compare')[-1]['payload']['operationId']
                    elif name=='disable-during-comparison':
                        page.evaluate('window.__speakmodeQA.deferCompare=true')
                        page.get_by_role('button',name='对比整理',exact=True).click()
                        expect(page.get_by_role('button',name='取消对照',exact=True)).to_be_visible()
                        page.get_by_role('switch').uncheck()
                        expect(page.locator('.personalization-notice')).to_contain_text('已停用画像')
                        assert calls(page,'personalization.save')[-1]['payload']['config']['enabled'] is False
                        page.evaluate('window.__speakmodeQA.releaseCompare()')
                        expect(page.get_by_role('button',name='取消对照',exact=True)).to_have_count(0)
                        expect(page.locator('.personalization-result-grid')).to_have_count(0)
                        expect(page.get_by_role('switch')).not_to_be_checked()
                    elif name=='delete-reference':
                        page.get_by_role('button',name='对比整理',exact=True).click()
                        expect(page.locator('.personalization-result-grid')).to_be_visible()
                        open_editor(page)
                        page.get_by_role('button',name='删除身份',exact=True).click()
                        page.get_by_role('button',name='从草稿删除',exact=True).click()
                        page.get_by_role('button',name='保存修改',exact=True).click()
                        expect(page.locator('.personalization-result-grid')).to_have_count(0)
                        assert calls(page,'personalization.save')[-1]['payload']['config']['profiles']==[]
                    elif name=='statistics-independent-failure':
                        page.evaluate('window.__speakmodeQA.statsError=true')
                        page.get_by_role('combobox',name='统计时间范围').click()
                        page.get_by_role('option',name='全部时间',exact=True).click()
                        expect(page.get_by_role('alert')).to_contain_text('合成统计读取失败')
                        expect(page.get_by_role('switch')).to_be_enabled()
                        page.evaluate('window.__speakmodeQA.statsError=false')
                        page.get_by_role('button',name='重新读取统计',exact=True).click()
                        expect(page.locator('.personalization-metrics')).to_be_visible()
                    elif name=='keyboard-layout':
                        open_editor(page)
                        task=page.get_by_role('textbox',name='常见任务',exact=True)
                        task.fill('技术讨论、客户沟通');expect(task).to_have_value('技术讨论、客户沟通')
                        page.get_by_role('tab',name='工作背景',exact=True).focus();page.keyboard.press('ArrowRight')
                        expect(page.get_by_role('tab',name='表达习惯',exact=True)).to_be_focused()
                        row['layouts']=[]
                        for width in [1280,800,430]:
                            row['layouts'].append(layout(page,width))
                            page.locator('.app-main').evaluate('(el)=>el.scrollTop=0')
                            page.screenshot(path=str(args.out/f'panel-{width}.png'))
                    row['commands']=qa.command_ledger(page)
                    assert not any(item['name'].startswith(('credential.','session.')) or item['name'] in ['result.copy','mode.preview'] for item in row['commands'])
                    row['status']='PASS'
                except Exception as error:
                    row['status']='FAIL';row['error']=str(error);row['traceback']=traceback.format_exc()
                    page.screenshot(path=str(args.out/f'{name}-failure.png'))
                finally:context.close()
        finally:browser.close()
    receipt['status']='PASS' if all(row['status']=='PASS' for row in rows) and not errors and not external else 'FAIL'
    (args.out/'receipt.json').write_text(json.dumps(receipt,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(dict(status=receipt['status'],cases=[dict(name=row['name'],status=row['status'],error=row.get('error')) for row in rows],pageErrors=errors),ensure_ascii=False,indent=2))
    return 0 if receipt['status']=='PASS' else 1


if __name__=='__main__':raise SystemExit(main())
