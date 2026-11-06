"""Actual React deletion-reference regressions with synthetic desktop IPC.

Runs the built frontend in isolated headless Edge through the existing managed
loopback preview. External requests are blocked. The fake history backend
deliberately retains and returns the OLD full profile reference after deletion:
PASS proves frontend cache/late-response handling, not native deletion or model
quality. A deliberately rejected synthetic copy request observes the React
record's revision IDs and manual-copy body; no clipboard/native action occurs.
No real user profile, credential, microphone, storage or provider is accessed.
"""
import argparse
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import traceback
from urllib.parse import urlparse

from playwright.sync_api import sync_playwright, expect

spec = importlib.util.spec_from_file_location('personalization_reference_base', Path(__file__).with_name('personalization-browser-qa.py'))
personal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(personal)
qa = personal.qa

FOURTH_ID = 'qa-reference-history-4'
REFERENCE_SENTINEL = '仅合成画像参考哨兵，删除后不得从缓存恢复。'
FOURTH_BODY = '缓存回归第4条正文：材料齐全后，可能还需三个工作日。'


def fixture():
    data = personal.fixture()
    data['histories'] = []
    for index in range(1, 5):
        detail = copy.deepcopy(data['detail'])
        identity = f'qa-reference-history-{index}'
        source_id, output_id = identity + '-source-r1', identity + '-output-r1'
        body = FOURTH_BODY if index == 4 else f'缓存回归第{index}条正文保持不变。'
        detail['session'].update(id=identity, currentSourceId=source_id, currentOutputId=output_id)
        detail['source'].update(id=source_id, sessionId=identity, asrAttemptId=identity + '-asr')
        detail['source']['items'] = [dict(id=identity + '-source-item', providerItemId=identity + '-provider-item', captureOrder=1,
                                        text=body, final=True, startMs=None, endMs=None)]
        detail['output'].update(id=output_id, sessionId=identity, sourceRevisionId=source_id)
        detail['output']['blocks'] = [dict(id=identity + '-block', text=body, sourceRefs=[identity + '-source-item'])]
        detail['output']['writingReceipt'] = dict(scene='document', language='asSpoken', contextUsed=False, contractVersion='synthetic-browser-reference-v1',
            personalization=dict(version=1, profileId='qa-developer', profileName='合成参考身份', scene='document',
                background=dict(profession='软件开发', domain='合成测试领域', description=REFERENCE_SENTINEL, tasks=['合成缓存检查'], audience='同事'),
                rules=[dict(key='paragraphs', text='合成参考规则哨兵。', origin='global')], omitted=[], referenceCleared=False))
        data['histories'].append(detail)
    data['detail'] = copy.deepcopy(data['histories'][0])
    data['snapshot']['usage']['count'] = 4
    return data


HISTORY_EXTENSION = r"""
   if(name==='history.list'){
    return {items:fixture.histories.map(d=>({sessionId:d.session.id,title:d.output.blocks.map(b=>b.text).join(' ').slice(0,48),preview:d.output.blocks.map(b=>b.text).join(' '),
     endedAt:d.session.endedAt,endLocalDate:d.session.endLocalDate,modeId:d.output.modeSnapshot.modeId,modeName:d.output.modeSnapshot.name,
     resolvedMode:d.output.resolvedMode,currentRevisionId:d.output.id,sourceRevisionId:d.source.id,status:'ready',sourceCharCount:d.source.items[0].text.length,captureDurationMs:d.session.captureDurationMs})),total:4,nextCursor:null};
   }
   if(name==='history.get'){
    const found=fixture.histories.find(d=>d.session.id===payload.sessionId);
    if(!found)throw new Error('Unknown reference-regression history ID');
    const frozen=copy(found),audit={sessionId:payload.sessionId,startedConfigVersion:fixture.config.version,outputRevisionId:frozen.output.id,sourceRevisionId:frozen.source.id,returned:false};
    window.__speakmodeQA.historyReadAudit.push(audit);
    if(payload.sessionId==='qa-reference-history-4'&&window.__speakmodeQA.deferFourth){
     window.__speakmodeQA.deferFourth=false;
     window.__speakmodeQA.pendingFourth=true;
     await new Promise(resolve=>{window.__speakmodeQA.releaseFourth=resolve;});
     audit.releasedConfigVersion=fixture.config.version;
    }
    audit.returned=true;
    // Deliberately return the old reference, including after a successful delete.
    return frozen;
   }
   if(name==='result.copy'){
    window.__speakmodeQA.blockedCopyProbes=(window.__speakmodeQA.blockedCopyProbes||0)+1;
    throw new Error('Synthetic clipboard probe rejected: no native/browser clipboard access');
   }
"""


def bridge(data):
    script = personal.bridge(data)
    script = script.replace("   if(name==='state.snapshot')", HISTORY_EXTENSION + "   if(name==='state.snapshot')")
    return script.replace(' window.__TAURI_INTERNALS__ = {',
        " window.__speakmodeQA.historyFixture=fixture.histories; window.__speakmodeQA.historyReadAudit=[];\n window.__TAURI_INTERNALS__ = {")


def fourth(page):
    return page.locator('.history-record-button').filter(has_text=FOURTH_BODY)


def show_history(page):
    page.get_by_role('button', name='历史记录', exact=True).click()
    expect(page.locator('.history-record-button')).to_have_count(4)


def delete_profile(page):
    page.get_by_role('button', name='个性化', exact=True).click()
    expect(page.get_by_role('heading', name='个性化', exact=True)).to_be_visible()
    personal.open_editor(page)
    page.get_by_role('button', name='删除身份', exact=True).click()
    page.get_by_role('button', name='从草稿删除', exact=True).click()
    page.get_by_role('button', name='保存修改', exact=True).click()
    expect(page.locator('.personalization-notice')).to_contain_text('已保存')
    saves = personal.calls(page, 'personalization.save')
    assert saves[-1]['payload']['config']['profiles'] == []


def copy_probe(page):
    """Observe real React record IDs through a rejected, synthetic IPC request."""
    page.get_by_role('button', name='复制文本', exact=True).click()
    expect(page.get_by_role('textbox', name='需要复制的结果', exact=True)).to_have_value(FOURTH_BODY)
    payload = personal.calls(page, 'result.copy')[-1]['payload']
    expected = dict(resultId=FOURTH_ID, sessionId=FOURTH_ID, outputRevisionId=FOURTH_ID + '-output-r1',
                    sourceRevisionId=FOURTH_ID + '-source-r1', sourceOnly=False)
    assert {key: payload[key] for key in expected} == expected
    page.get_by_role('button', name='收起', exact=True).click()
    return expected


def assert_cleared(page):
    expect(page.locator('.history-current')).to_contain_text(FOURTH_BODY)
    expect(page.get_by_text('参考详情已清除。', exact=True)).to_be_visible()
    expect(page.locator('.history-detail-shell').get_by_text(REFERENCE_SENTINEL, exact=True)).to_have_count(0)
    expect(page.locator('.history-detail-shell .personalization-background')).to_have_count(0)
    return copy_probe(page)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    if urlparse(args.url).hostname != '127.0.0.1' or not args.out.is_absolute():
        parser.error('Use loopback URL and a fresh absolute evidence directory')
    args.out.mkdir(parents=True, exist_ok=False)
    rows, errors, external = [], [], []
    receipt = dict(scope=__doc__, cases=rows, pageErrors=errors, externalRequests=external, nativeDeletionProof=False,
                   clipboardAccess=False, backendDeliberatelyReturnsStaleProfile=True)
    sources = ['app/src/services/DesktopProvider.tsx', 'app/src/services/personalizationReferences.ts', 'app/src/pages/HistoryPage.tsx',
               'app/src/pages/PersonalizationPage.tsx', 'scripts/personalization-reference-browser-qa.py', 'scripts/personalization-browser-qa.py',
               'scripts/restore-delivery-browser-qa.py']
    receipt['sourceSha256'] = {name: hashlib.sha256((qa.ROOT / name).read_bytes()).hexdigest() for name in sources}
    receipt['distSha256'] = {str(path.relative_to(qa.ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                            for path in sorted((qa.ROOT / 'app/dist').rglob('*')) if path.is_file()}
    with qa.managed_preview(args.url, args.out, True) as server, sync_playwright() as playwright:
        receipt['server'] = server
        browser = playwright.chromium.launch(channel='msedge', headless=True)
        receipt['browserVersion'] = browser.version
        try:
            for name in ['fourth-cached-reference-after-delete', 'fourth-late-read-after-delete-and-unmount']:
                context = browser.new_context(viewport=dict(width=1280, height=900), locale='zh-CN', reduced_motion='reduce')
                data = fixture()
                context.add_init_script(bridge(data))
                def route(route):
                    if urlparse(route.request.url).hostname not in ('127.0.0.1', None):
                        external.append(route.request.url)
                        route.abort()
                    else:
                        route.continue_()
                context.route('**/*', route)
                page = context.new_page()
                page.on('pageerror', lambda error: errors.append(str(error)))
                row = dict(name=name, status='RUNNING', syntheticBackendRetainsDeletedReference=True)
                rows.append(row)
                try:
                    page.goto(args.url, wait_until='networkidle')
                    show_history(page)
                    if name == 'fourth-cached-reference-after-delete':
                        fourth(page).click()
                        expect(page.locator('.history-current')).to_contain_text(FOURTH_BODY)
                        page.get_by_text(re.compile('查看本次参考信息')).click()
                        expect(page.locator('.personalization-background')).to_contain_text(REFERENCE_SENTINEL)
                        row['beforeRevisionProbe'] = copy_probe(page)
                        page.screenshot(path=str(args.out / (name + '-before.png')), full_page=True)
                        before_reads = len([item for item in personal.calls(page, 'history.get') if item['payload']['sessionId'] == FOURTH_ID])
                        assert before_reads == 1
                        delete_profile(page)
                        show_history(page)
                        fourth(page).click()
                        row['afterRevisionProbe'] = assert_cleared(page)
                        after_reads = len([item for item in personal.calls(page, 'history.get') if item['payload']['sessionId'] == FOURTH_ID])
                        assert after_reads == before_reads, 'cached fourth record must be cleared without requiring a new backend read'
                        assert row['beforeRevisionProbe'] == row['afterRevisionProbe']
                    else:
                        page.evaluate('window.__speakmodeQA.deferFourth=true')
                        fourth(page).click()
                        page.wait_for_function('window.__speakmodeQA.pendingFourth===true')
                        expect(page.get_by_role('heading', name='正在读取完整内容', exact=True)).to_be_visible()
                        delete_profile(page)  # Unmounts the waiting HistoryPage before its read returns.
                        page.evaluate('window.__speakmodeQA.releaseFourth()')
                        page.wait_for_function("window.__speakmodeQA.historyReadAudit.some(item=>item.sessionId==='qa-reference-history-4'&&item.returned)")
                        show_history(page)
                        fourth(page).click()
                        row['afterRevisionProbe'] = assert_cleared(page)
                        audit = page.evaluate("window.__speakmodeQA.historyReadAudit.filter(item=>item.sessionId==='qa-reference-history-4')")
                        assert len(audit) == 1 and audit[0]['startedConfigVersion'] == 1 and audit[0]['releasedConfigVersion'] == 2, audit
                        row['delayedRead'] = audit[0]
                    old_ref = page.evaluate("window.__speakmodeQA.historyFixture[3].output.writingReceipt.personalization")
                    assert old_ref['background']['description'] == REFERENCE_SENTINEL and old_ref['referenceCleared'] is False
                    row['backendStillHasOldReference'] = True
                    row['commands'] = qa.command_ledger(page)
                    blocked = [item['name'] for item in row['commands'] if item['name'].startswith(('credential.', 'session.', 'revision.'))
                               or item['name'] in ['history.rewrite', 'mode.preview', 'personalization.compare']]
                    assert not blocked, blocked
                    row['blockedSyntheticCopyProbes'] = page.evaluate('window.__speakmodeQA.blockedCopyProbes')
                    row['status'] = 'PASS'
                    page.screenshot(path=str(args.out / (name + '-after.png')), full_page=True)
                except Exception as error:
                    row.update(status='FAIL', error=str(error), traceback=traceback.format_exc())
                    page.screenshot(path=str(args.out / (name + '-failure.png')), full_page=True)
                finally:
                    context.close()
        finally:
            browser.close()
    receipt['status'] = 'PASS' if all(row['status'] == 'PASS' for row in rows) and not errors and not external else 'FAIL'
    qa.write_new(args.out / 'receipt.json', receipt, json_value=True)
    print(json.dumps(dict(status=receipt['status'], cases=[dict(name=row['name'], status=row['status'], error=row.get('error')) for row in rows],
                         pageErrors=errors, externalRequests=external), ensure_ascii=False, indent=2))
    return 0 if receipt['status'] == 'PASS' else 1


if __name__ == '__main__':
    raise SystemExit(main())
