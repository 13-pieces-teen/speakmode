"""Measure usage-period layout stability with isolated synthetic IPC.

Local production browser only: no native database, provider, microphone, profile,
or clipboard access. --baseline records existing shifts without accepting them
as the regression target. Every animation frame is retained in a JSON trace.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import traceback
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('usage_typography_qa', ROOT / 'scripts/usage-typography-browser-qa.py')
typography = importlib.util.module_from_spec(spec)
spec.loader.exec_module(typography)
ui, base = typography.ui, typography.base
FIXED_TIME = '2026-10-09T06:00:00+00:00'

RECORDER = r'''()=>{
 const q=window.__speakmodeQA;
 const box=e=>{if(!e)return null;const r=e.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height,bottom:r.bottom};};
 const numeric=e=>{const s=getComputedStyle(e),r=document.createRange();r.selectNodeContents(e);return {...box(e),text:e.textContent,fontSizeInline:e.style.fontSize,fontSize:parseFloat(s.fontSize),lineHeight:parseFloat(s.lineHeight),transitionProperty:s.transitionProperty,transitionDuration:s.transitionDuration,textFragments:[...r.getClientRects()].map(b=>({x:b.x,y:b.y,width:b.width,height:b.height,bottom:b.bottom}))};};
 const read=()=>{const main=document.querySelector('.app-main'),status=document.querySelector('.usage-read-status');return {at:performance.now(),phase:q.geometryPhase,main:{...box(main),scrollTop:main.scrollTop,scrollHeight:main.scrollHeight,clientHeight:main.clientHeight},overview:box(document.querySelector('.usage-overview')),header:box(document.querySelector('.usage-period')),metrics:box(document.querySelector('.usage-metrics')),cards:[...document.querySelectorAll('.usage-metric')].map(e=>({...box(e),dd:box(e.querySelector('dd')),text:e.innerText,numbers:[...e.querySelectorAll('strong')].map(numeric),pieces:[...e.querySelector('dd').children].map(numeric)})),preferences:box(document.querySelector('.input-preferences')),latest:box(document.querySelector('.latest-card')),status:status?{...box(status),display:getComputedStyle(status).display,text:status.innerText}:null,documentWidth:document.documentElement.scrollWidth};};
 q.geometryFrames=[];q.geometryPhase='initial';q.readGeometry=read;q.geometryRunning=true;
 q.geometryAfterLayout=[];q.geometryResizeObserver=new ResizeObserver(entries=>q.geometryAfterLayout.push({...read(),observedContentHeight:entries[0].contentRect.height}));q.geometryResizeObserver.observe(document.querySelector('.usage-metrics'));
 q.geometryLayoutShifts=[];q.geometryShiftObserver=new PerformanceObserver(list=>q.geometryLayoutShifts.push(...list.getEntries().map(e=>({at:e.startTime,value:e.value,hadRecentInput:e.hadRecentInput,sources:e.sources?.map(s=>({node:s.node?.className,previous:s.previousRect.toJSON(),current:s.currentRect.toJSON()}))}))));q.geometryShiftObserver.observe({type:'layout-shift'});
 const frame=()=>{if(!q.geometryRunning)return;q.geometryFrames.push(read());q.geometryRaf=requestAnimationFrame(frame);};frame();
}'''


def hashes(folder):
    return {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(folder.rglob('*')) if p.is_file()}


def fixture(zero=False):
    data = typography.fixture()
    data['snapshot']['onboarding'] = dict(progress=dict(version=1, autoShow=False, step='ready', completedLessons=['chat', 'document'], completedAt=FIXED_TIME), active=False, shortcutPresses=2)
    data['snapshot']['preferences'].update(microphoneId='ui-microphone', language='zh')
    if zero:
        data['snapshot']['usage'].update(count=0, sourceCharCount=0, captureDurationMs=0)
    return data


def values(page):
    return page.locator('.usage-metric dd').all_inner_texts()


def phase(page, out, name, label, snapshots):
    page.evaluate('(label)=>window.__speakmodeQA.geometryPhase=label', label)
    page.wait_for_timeout(180)
    snapshots[label] = page.evaluate('window.__speakmodeQA.readGeometry()')
    snapshots[label]['motion'] = page.locator('.u047').evaluate('''e=>{const k=e.querySelector('.u047-knob'),s=getComputedStyle(k);return {mode:e.dataset.motion,classes:e.className,left:s.left,right:s.right,duration:s.transitionDuration,delay:s.transitionDelay,animations:k.getAnimations().map(a=>({property:a.transitionProperty,currentTime:a.currentTime,playState:a.playState,duration:a.effect.getTiming().duration,delay:a.effect.getTiming().delay}))};}''')
    page.mouse.move(0, 0)
    page.screenshot(path=str(out / f'{name}-{label}.png'))


def set_mode(page, mode):
    page.evaluate('(mode)=>window.__speakmodeQA.usageMode=mode', mode)


def select(page, label):
    page.get_by_role('group', name='统计范围', exact=True).get_by_role('button', name=label, exact=True).click()


def resolve(page, reject=False):
    page.evaluate('''reject=>{const q=window.__speakmodeQA;q.pendingUsage.splice(0).forEach(p=>reject?p.reject(new Error('Synthetic usage read failure')):p.resolve(p.value));}''', reject)


def ready(page, first):
    expect(page.locator('.usage-overview')).to_have_attribute('aria-busy', 'false')
    expect(page.locator('.usage-metric dd').first).to_contain_text(first)


def summarize(frames, snapshots):
    result = {}
    paths = [('overview',), ('header',), ('metrics',), ('preferences',), ('latest',)]
    paths += [('cards', i) for i in range(3)]
    for path in paths:
        rows = frames
        for key in path:
            rows = [r[key] for r in rows]
        result['.'.join(map(str, path))] = {axis: round(max(r[axis] for r in rows) - min(r[axis] for r in rows), 4) for axis in ['y', 'height']}
    result['main.scrollTop'] = round(max(r['main']['scrollTop'] for r in frames) - min(r['main']['scrollTop'] for r in frames), 4)
    initial = snapshots['initial']
    result['phaseDeltas'] = {label: {key: {axis: round(row[key][axis] - initial[key][axis], 4) for axis in ['y', 'height']} for key in ['overview', 'metrics', 'preferences', 'latest']} for label, row in snapshots.items()}
    return result


def exercise(page, out, name, scenario, info_open=False, assert_single_line=True, scrolled_motion=False):
    snapshots = {}
    ready(page, '0' if scenario == 'zero' else '28')
    if info_open:
        page.locator('.usage-info > summary').click()
        expect(page.locator('.usage-info')).to_have_attribute('open', '')
        initial_info = page.locator('.usage-info > p').inner_text()
    if scrolled_motion:
        page.locator('.app-main').evaluate('e=>e.scrollTop=40')
        expect(page.get_by_role('group', name='统计范围', exact=True)).to_be_in_viewport(ratio=1)
        assert page.locator('.app-main').evaluate('e=>e.scrollTop') == 40
    page.evaluate(RECORDER)
    phase(page, out, name, 'initial', snapshots)
    if scenario == 'large':
        page.evaluate('window.__speakmodeQA.totalUsage={count:1234567,sourceCharCount:987654321,captureDurationMs:74074076000}')
    set_mode(page, 'hold')
    select(page, '累计')
    page.wait_for_function('window.__speakmodeQA.pendingUsage.length===1')
    assert all('—' in v and '0' not in v for v in values(page)), values(page)
    phase(page, out, name, 'loading-total', snapshots)
    if scrolled_motion:
        motion = snapshots['loading-total']['motion']
        assert motion['mode'] == 'normal' and 'go-r' in motion['classes'], motion
        assert motion['duration'] == '0.3s, 0.5s' and motion['delay'] == '0s, 0.07s', motion
        assert any(a['playState'] == 'running' for a in motion['animations']), motion
    resolve(page, reject=scenario == 'failure')
    if scenario == 'failure':
        expect(page.get_by_role('button', name='重新读取', exact=True)).to_be_visible()
        assert all('—' in v for v in values(page)), values(page)
        phase(page, out, name, 'failed-total', snapshots)
        page.get_by_role('button', name='重新读取', exact=True).click()
        page.wait_for_function('window.__speakmodeQA.pendingUsage.length===1')
        phase(page, out, name, 'retry-pending', snapshots)
        resolve(page)
    ready(page, '1,234,567' if scenario == 'large' else '97')
    expect(page.locator('.usage-metric dd').nth(1)).to_contain_text('987,654,321' if scenario == 'large' else '56,432')
    if scenario == 'large':
        expect(page.locator('.usage-metric dd').nth(2)).to_contain_text('1,234,567')
        assert page.locator('.usage-metric strong').all_inner_texts() == ['1,234,567', '987,654,321', '1,234,567', '56']
    phase(page, out, name, 'ready-total', snapshots)
    if scenario == 'large' and assert_single_line:
        for card in snapshots['ready-total']['cards']:
            for number in card['numbers']:
                assert len(number['textFragments']) == 1, number
            line_height = max(piece['lineHeight'] for piece in card['pieces'])
            assert max(piece['bottom'] for piece in card['pieces']) <= card['dd']['y'] + line_height + 1, card
            assert all(piece['x'] >= card['x'] and piece['x'] + piece['width'] <= card['x'] + card['width'] for piece in card['pieces']), card
    if scenario == 'failure':
        # A focus refresh of the same range keeps the authoritative prior read.
        prior = values(page)
        page.evaluate("window.dispatchEvent(new Event('focus'))")
        page.wait_for_function('window.__speakmodeQA.pendingUsage.length===1')
        assert values(page) == prior, values(page)
        phase(page, out, name, 'old-value-refresh-pending', snapshots)
        resolve(page, reject=True)
        expect(page.locator('.usage-read-status')).to_contain_text('更新失败')
        assert values(page) == prior, values(page)
        phase(page, out, name, 'old-value-refresh-failed', snapshots)
        page.get_by_role('button', name='重新读取', exact=True).click()
        page.wait_for_function('window.__speakmodeQA.pendingUsage.length===1')
        resolve(page)
        ready(page, '97')
        phase(page, out, name, 'old-value-retry-ready', snapshots)
    select(page, '今日')
    page.wait_for_function('window.__speakmodeQA.pendingUsage.length===1')
    phase(page, out, name, 'loading-today', snapshots)
    resolve(page)
    ready(page, '0' if scenario == 'zero' else '28')
    phase(page, out, name, 'ready-today', snapshots)
    if info_open:
        expect(page.locator('.usage-info')).to_have_attribute('open', '')
        assert page.locator('.usage-info > p').inner_text() == initial_info
    if scenario == 'success':
        # Two overlapping reads: obsolete total must never replace selected today.
        select(page, '累计'); select(page, '今日')
        page.wait_for_function('window.__speakmodeQA.pendingUsage.length===2')
        page.evaluate('''()=>{const q=window.__speakmodeQA,p=q.pendingUsage.splice(0);p[1].resolve(p[1].value);q.staleUsage=p[0];}''')
        ready(page, '28')
        page.evaluate('window.__speakmodeQA.staleUsage.resolve(window.__speakmodeQA.staleUsage.value)')
        page.wait_for_timeout(60)
        ready(page, '28')
        expect(page.locator('.usage-metric dd').nth(1)).to_contain_text('12,864')
        phase(page, out, name, 'race-ignored-obsolete-total', snapshots)
    page.evaluate('''()=>{const q=window.__speakmodeQA;q.geometryRunning=false;cancelAnimationFrame(q.geometryRaf);q.geometryResizeObserver.disconnect();q.geometryShiftObserver.disconnect();}''')
    frames = page.evaluate('window.__speakmodeQA.geometryFrames')
    assert len(frames) >= 15, len(frames)
    assert all(r['documentWidth'] <= page.viewport_size['width'] for r in frames), 'Horizontal overflow'
    after_layout = page.evaluate('window.__speakmodeQA.geometryAfterLayout')
    shifts = page.evaluate('window.__speakmodeQA.geometryLayoutShifts')
    outer_shifts = [dict(at=entry['at'], value=entry['value'], hadRecentInput=entry['hadRecentInput'], source=source) for entry in shifts for source in entry.get('sources', []) if source.get('node') and any(token in source['node'] for token in ['home-preferences', 'input-preferences', 'latest-card', 'usage-metric']) and abs(source['current']['y'] - source['previous']['y']) > .5]
    return dict(snapshots=snapshots, frameCount=len(frames), ranges=summarize(frames, snapshots), afterLayout=after_layout, afterLayoutRanges=summarize(after_layout, snapshots), layoutShifts=shifts, outerLayoutShifts=outer_shifts, values=values(page), reads=page.evaluate('window.__speakmodeQA.usageReads'), frames=frames)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--baseline', action='store_true')
    parser.add_argument('--case')
    args = parser.parse_args()
    if urlparse(args.url).scheme != 'http' or urlparse(args.url).hostname != '127.0.0.1' or not args.out.is_absolute():
        parser.error('Requires local http://127.0.0.1 URL and absolute fresh --out')
    args.out.mkdir(parents=True, exist_ok=False)
    receipt = dict(status='RUNNING', scope=__doc__, baseline=args.baseline, startedAt=datetime.now(timezone.utc).isoformat(), tests=[], pageErrors=[], externalRequests=[], sourceSha256Start=hashes(ROOT / 'app/src'), distSha256Start=hashes(ROOT / 'app/dist'))
    with sync_playwright() as p:
        browser = p.chromium.launch(channel='msedge', headless=True, ignore_default_args=['--hide-scrollbars'])
        receipt['browser'] = dict(version=browser.version, channel='msedge', headless=True, ignoreDefaultArgs=['--hide-scrollbars'])
        for width, height, scenarios in [(1220, 820, ['success', 'failure', 'zero']), (375, 812, ['success', 'failure', 'zero', 'large', 'info-open', 'success-scrolled-motion']), (720, 560, ['large'])]:
            for scenario in scenarios:
                name = f'{width}x{height}-{scenario}'
                if args.case and args.case != name:
                    continue
                row = dict(name=name, status='RUNNING'); receipt['tests'].append(row)
                context = browser.new_context(viewport=dict(width=width, height=height), locale='zh-CN', timezone_id='Asia/Shanghai', reduced_motion='no-preference' if scenario == 'success-scrolled-motion' else 'reduce')
                context.add_init_script(ui.BRIDGE.replace('__FIXTURE__', json.dumps(fixture(zero=scenario == 'zero'), ensure_ascii=False)))
                context.add_init_script(typography.USAGE_BRIDGE)
                def route(r):
                    if urlparse(r.request.url).hostname not in ('127.0.0.1', None):
                        receipt['externalRequests'].append(r.request.url); r.abort()
                    else:
                        r.continue_()
                context.route('**/*', route)
                context.tracing.start(screenshots=True, snapshots=True, sources=False)
                page = context.new_page()
                page.on('pageerror', lambda e: receipt['pageErrors'].append(str(e)))
                try:
                    page.clock.install(time=datetime.fromisoformat(FIXED_TIME))
                    page.goto(args.url, wait_until='networkidle')
                    expect(page.locator('.home-hero h1')).to_be_visible()
                    page.evaluate('document.fonts.ready')
                    observations = exercise(page, args.out, name, 'failure' if scenario == 'info-open' else 'success' if scenario == 'success-scrolled-motion' else scenario, info_open=scenario == 'info-open', assert_single_line=not args.baseline, scrolled_motion=scenario == 'success-scrolled-motion')
                    frames = observations.pop('frames')
                    (args.out / f'{name}-frames.json').write_text(json.dumps(frames, ensure_ascii=False, indent=2), encoding='utf-8')
                    row['observations'] = observations
                    if not args.baseline:
                        for sampling in ['ranges', 'afterLayoutRanges']:
                            for key, ranges in observations[sampling].items():
                                if key == 'phaseDeltas':
                                    continue
                                if key == 'main.scrollTop':
                                    assert ranges <= 0.5, (sampling, key, ranges)
                                else:
                                    assert max(ranges.values()) <= 0.5, (sampling, key, ranges)
                        assert not observations['outerLayoutShifts'], observations['outerLayoutShifts']
                    row['status'] = 'PASS'
                except Exception as e:
                    row.update(status='FAIL', error=str(e), traceback=traceback.format_exc())
                    frames = page.evaluate('window.__speakmodeQA?.geometryFrames||[]')
                    (args.out / f'{name}-frames.json').write_text(json.dumps(frames, ensure_ascii=False, indent=2), encoding='utf-8')
                    page.screenshot(path=str(args.out / f'{name}-failure.png'))
                finally:
                    context.tracing.stop(path=str(args.out / f'{name}-trace.zip'))
                    context.close()
        browser.close()
    receipt['distSha256End'] = hashes(ROOT / 'app/dist')
    receipt['distStable'] = receipt['distSha256Start'] == receipt['distSha256End']
    receipt['sourceSha256End'] = hashes(ROOT / 'app/src')
    receipt['sourceStable'] = receipt['sourceSha256Start'] == receipt['sourceSha256End']
    receipt['counts'] = dict(passed=sum(t['status'] == 'PASS' for t in receipt['tests']), failed=sum(t['status'] == 'FAIL' for t in receipt['tests']))
    receipt['status'] = 'PASS' if receipt['tests'] and not receipt['counts']['failed'] and not receipt['pageErrors'] and not receipt['externalRequests'] and receipt['distStable'] and (args.baseline or receipt['sourceStable']) else 'FAIL'
    receipt['completedAt'] = datetime.now(timezone.utc).isoformat()
    (args.out / 'receipt.json').write_text(json.dumps(receipt, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps({k: receipt[k] for k in ['status', 'counts', 'pageErrors', 'externalRequests', 'distStable', 'sourceStable']}, indent=2, ensure_ascii=False))
    return 0 if receipt['status'] == 'PASS' else 1


if __name__ == '__main__':
    raise SystemExit(main())
