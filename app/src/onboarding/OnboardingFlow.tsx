import {useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode} from 'react';
import {ArrowLeftIcon, ArrowRightIcon, ChatBubbleIcon, CheckCircledIcon, CheckIcon, CursorTextIcon, ExitIcon, FileTextIcon, KeyboardIcon, LockClosedIcon, SpeakerLoudIcon} from '@radix-ui/react-icons';
import {useDesktop} from '../services/DesktopProvider';
import {errorMessage} from '../services/bridge';
import {WindowChrome} from '../components/WindowChrome';
import {ProviderSettings} from '../components/ProviderSettings';
import {ShortcutEditor} from '../components/ShortcutEditor';
import {CopyAction} from '../components/CopyAction';
import {fromDetail} from '../model';
import {shortcutCanonical, shortcutLabel} from '../preferences';
import {onboardingSteps, type OnboardingState, type OnboardingStep} from './types';
import {usePractice} from './usePractice';
import '../pages/settings.css';
import './onboarding.css';

const copy: Record<OnboardingStep,{eyebrow:string;title:ReactNode;description:string}> = {
  welcome:{eyebrow:'从一句话开始',title:<>想到什么，<br/><span>就说什么。</span></>,description:'把停顿和改口交给 SpeakMode。\n用两次真实口述，找到自然成稿的感觉。'},
  setup:{eyebrow:'先准备好声音',title:<>连接好，<br/><span>就能开口。</span></>,description:'选择麦克风，连接你自己的云端服务。\n接下来的练习会实际验证录音和成稿。'},
  shortcut:{eyebrow:'一个按键，开始表达',title:<>按一下，<br/><span>开始说话。</span></>,description:'按下你的录音快捷键，看看右侧的反馈。\n这一步只测试按键，下一步才会开始录音。'},
  chat:{eyebrow:'01 / 自然说话',title:<>可以停顿，<br/><span>也可以改口。</span></>,description:'照着下面说，或换成你自己的话。\n说完再按一次快捷键，看看这条消息怎么成稿。'},
  document:{eyebrow:'02 / 合适的场合',title:<>换个场合，<br/><span>换种表达。</span></>,description:'试着把一段想法说成文档。\n保留事实和条件，让段落更适合阅读。'},
  ready:{eyebrow:'准备好，去表达',title:<>把光标放好，<br/><span>接下来，用说的。</span></>,description:'开始使用后，在常用应用里点一下输入框，\n再按快捷键。SpeakMode 会按你的日常设置交付文字。'},
};
const cnChat = '那个，小林，会议明天下午三点，呃，不对，四点。你方便的话，把会议链接发给我。';
const enChat = 'Hi Alex, the meeting is tomorrow at three, uh, actually four. Could you send me the meeting link?';
const cnDocument = '今天同步三件事。登录页已经完成。支付页还在联调，预计周五完成。发布前先补一轮回归测试，通过以后再上线。';
const enDocument = 'A quick project update. The login page is complete. Payment integration is in progress and should finish on Friday. Run the regression tests before release, and only launch if they pass.';

function Keycap({value}:{value:string}) { return <kbd className="ob-keycap">{value === 'alt_right' ? 'Right Alt' : shortcutLabel(value)}</kbd>; }
function Source({children}:{children:ReactNode}) { return <div className="ob-source"><p><SpeakerLoudIcon aria-hidden="true"/>可以这样说</p><blockquote>{children}</blockquote></div>; }

export function OnboardingFlow({state}:{state:OnboardingState}) {
  const desktop = useDesktop();
  const step = state.progress.step;
  const index = onboardingSteps.indexOf(step);
  const lesson = step === 'chat' || step === 'document' ? step : null;
  const practice = usePractice(lesson);
  const [error, setError] = useState('');
  const [pending, setPending] = useState(false);
  const [manualReduced, setManualReduced] = useState(false);
  const [setupDraft, setSetupDraft] = useState({dirty:false,pending:false});
  const heading = useRef<HTMLHeadingElement>(null);
  const operation = useRef(false);
  const mounted = useRef(true);
  const instant = useRef(true);
  const [noEnter, setNoEnter] = useState(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { setNoEnter(instant.current); heading.current?.focus({preventScroll:true}); setError(''); }, [step]);
  const onDraftStateChange = useCallback((value:{dirty:boolean;pending:boolean}) => setSetupDraft(value), []);
  const busy = pending || practice.pending || practice.busy || (step === 'setup' && setupDraft.pending);
  const unsaved = step === 'setup' && setupDraft.dirty;
  const device = desktop.devices.find(value => value.id === desktop.preferences.microphoneId);
  const deviceReady = desktop.devicesStatus === 'ready' && desktop.devices.some(value => value.available !== false)
    && (!desktop.preferences.microphoneId || !!device && device.available !== false);
  const canSetupContinue = desktop.provider.configured && deviceReady && !unsaved;
  const detail = practice.detail;
  const output = detail?.output;
  const hasResult = !!output && practice.session?.state.toLowerCase() === 'ready' && !practice.busy;
  const fallback = !!output?.rewriteFallback || output?.kind === 'useSource';
  const fallbackReason = output?.rewriteFallback === 'timeout' ? '整理等待超时' : output?.rewriteFallback === 'serviceUnavailable' ? '整理服务暂时不可用' : output?.rewriteFallback === 'meaningRisk' ? '整理结果未通过原意检查' : '整理未完成';
  const source = detail?.source.items.map(item => item.text).join('\n') ?? '';
  const outputText = output?.blocks.map(block => block.text).join('\n\n') ?? '';
  const english = desktop.preferences.language === 'en';
  const tested = state.shortcutPresses > 0;
  const serviceError = [...new Set(desktop.errors.filter(issue => issue.message !== practice.session?.error && issue.message !== practice.error).map(issue => issue.message))].join('；');
  const label = copy[step];

  async function run(action:()=>Promise<unknown>) {
    if (operation.current) return;
    operation.current = true; setPending(true); setError('');
    try { await action(); await desktop.refresh(); }
    catch (e) { if (mounted.current) setError(errorMessage(e)); }
    finally { operation.current = false; if (mounted.current) setPending(false); }
  }
  function move(next:OnboardingStep, immediate:boolean) {
    instant.current = immediate; setNoEnter(immediate);
    void run(() => desktop.command('onboarding.step',{step:next,expectedVersion:state.progress.version}));
  }
  function continueLesson(immediate:boolean) {
    instant.current = immediate; setNoEnter(immediate);
    void run(async () => {
      const accepted = await desktop.command<OnboardingState>('onboarding.acceptLesson',{sessionId:practice.id,expectedGeneration:practice.generation,expectedVersion:state.progress.version,...(fallback ? {acceptedFallbackRevisionId:output?.id} : {})});
      await desktop.command('onboarding.step',{step:step === 'chat' ? 'document' : 'ready',expectedVersion:accepted.progress.version});
    });
  }
  function finish() { void run(() => desktop.command('onboarding.finish',{expectedVersion:state.progress.version})); }
  const previous = index > 0 ? onboardingSteps[index - 1] : null;
  const sourceText = lesson === 'chat' ? english ? enChat : cnChat : english ? enDocument : cnDocument;

  return <div className={`ob-window${manualReduced ? ' ob-reduced' : ''}${noEnter ? ' ob-transition-instant' : ''}`}>
    <WindowChrome available={desktop.available}/>
    <div className="ob-progress" role="progressbar" aria-label="使用引导进度" aria-valuemin={0} aria-valuemax={5} aria-valuenow={index} aria-valuetext={label.eyebrow}>
      <div style={{transform:`scaleX(${index / 5})`}}/>
    </div>
    <div className="ob-body">
      <section className="ob-guide" aria-labelledby="ob-heading">
        <div className="ob-top"><button type="button" className={`ob-quiet${!previous ? ' ob-hidden' : ''}`} disabled={busy || unsaved || !previous} onClick={event => previous && move(previous,event.detail === 0)}><ArrowLeftIcon/>返回</button><span className="ob-count">{String(index + 1).padStart(2,'0')} <span>/ 06</span></span></div>
        <div key={step} className={`ob-copy${noEnter ? ' ob-instant' : ''}`}>
          <span className="ob-eyebrow">{label.eyebrow}</span>
          <h1 ref={heading} id="ob-heading" tabIndex={-1}>{label.title}</h1>
          <p className="ob-description">{label.description}</p>
          {step === 'welcome' && <div className="ob-promises"><span><KeyboardIcon/>一个快捷键</span><span><ChatBubbleIcon/>两次真实练习</span><span><LockClosedIcon/>由你掌控</span></div>}
          {step === 'setup' && <div className="ob-setup-notes"><p><LockClosedIcon/>Key 仅保存在系统凭据保管库。</p><p>识别与整理可以选择不同供应商。录音发送给识别服务，识别文字发送给整理服务；练习不采集其他应用的输入框上下文。</p><p>练习同样计入口述统计，文字遵循你的历史保存设置。</p></div>}
          {step === 'shortcut' && <div className="ob-shortcut-config"><ShortcutEditor value={shortcutLabel(desktop.preferences.shortcut)} disabled={busy} onChange={async (value,captureId) => {await desktop.command('preferences.update',{patch:{shortcut:shortcutCanonical(value)},expectedVersion:desktop.preferences.version,captureId});await desktop.refresh();}}/><p>如果按键没有反应，可以换一个快捷键；也可以先用录音按钮体验。</p></div>}
          {lesson && <Source>{sourceText}</Source>}
          {lesson && <p className="ob-lesson-note">本次练习使用「{lesson === 'chat' ? '聊天消息' : '文档写作'}」，结果留在右侧。日常使用仍采用你保存的设置。</p>}
          {step === 'ready' && <ol className="ob-next"><li>点击「开始使用」，退出练习。</li><li>打开聊天、邮件或文档，点进输入框。</li><li>按 <Keycap value={desktop.preferences.shortcut}/> 开始，再按一次结束。</li></ol>}
        </div>
        <div className="ob-bottom">
          {(error || practice.error) && <p className="ob-error" role="alert">{error || practice.error}</p>}
          {serviceError && <p className="ob-error" role="alert">{serviceError}<button type="button" className="ob-link" onClick={() => void desktop.refresh()}>重新读取状态</button></p>}
          {desktop.status === 'error' && <p className="ob-error" role="alert">桌面状态读取失败。<button type="button" className="ob-link" onClick={() => void desktop.refresh()}>重新读取</button></p>}
          <div className="ob-actions">
            {step === 'welcome' && <button className="ob-primary" disabled={busy} onClick={event => move('setup',event.detail === 0)}>开始体验<ArrowRightIcon/></button>}
            {step === 'setup' && <button className="ob-primary" disabled={busy || !canSetupContinue} onClick={event => move('shortcut',event.detail === 0)}>测试快捷键<ArrowRightIcon/></button>}
            {step === 'shortcut' && <button className="ob-primary" disabled={busy} onClick={event => move('chat',event.detail === 0)}>{tested ? '收到，开始练习' : '先用按钮体验'}<ArrowRightIcon/></button>}
            {lesson && <button className="ob-primary" disabled={busy || !hasResult} onClick={event => continueLesson(event.detail === 0)}>{fallback && hasResult ? lesson === 'chat' ? '确认原文并继续' : '确认原文并完成' : lesson === 'chat' ? '继续，试试文档' : '完成练习'}<ArrowRightIcon/></button>}
            {step === 'ready' && <button className="ob-primary" disabled={busy} onClick={finish}>开始使用<ArrowRightIcon/></button>}
            {pending && <span className="ob-note" role="status">正在保存…</span>}
          </div>
          <p className="ob-action-hint">{unsaved ? '请先保存或清空右侧未保存的配置。' : lesson && !hasResult ? '先完成一次口述，再继续。' : step === 'setup' && !canSetupContinue ? '保存服务配置与 Key，并确认麦克风可用。' : step === 'ready' ? '实际输入效果，请在常用应用里试一次。' : '进度会保存在本机，随时可以回来继续。'}</p>
        </div>
      </section>
      <section className="ob-stage" aria-label="互动练习区">
        <button type="button" className="ob-quiet ob-exit" disabled={busy || unsaved} onClick={() => void run(() => desktop.command('onboarding.dismiss',{expectedVersion:state.progress.version}))}><ExitIcon/>稍后继续</button>
        <div className={`ob-stage-card${step === 'setup' ? ' ob-stage-card-setup' : ''}`}>
          {step === 'welcome' && <WelcomeStage/>}
          {step === 'setup' && <div className="ob-setup-card">
            <div className="ob-card-heading"><SpeakerLoudIcon/><span>准备就绪</span><span className="ob-badge">本机设置</span></div>
            <div className="ob-setup-scroll">
              <label className="ob-field" htmlFor="ob-microphone">录音麦克风<select id="ob-microphone" value={desktop.preferences.microphoneId ?? ''} disabled={pending || desktop.devicesStatus !== 'ready'} onChange={event => {const value = event.target.value;void run(() => desktop.command('preferences.update',{patch:{microphoneId:value || null},expectedVersion:desktop.preferences.version}));}}><option value="">系统默认麦克风</option>{desktop.preferences.microphoneId && !device && <option value={desktop.preferences.microphoneId} disabled>已保存的设备（当前不可用）</option>}{desktop.devices.map(value => <option key={value.id} value={value.id} disabled={value.available === false}>{value.name}{value.available === false ? '（不可用）' : ''}</option>)}</select></label>
              <div className="ob-device-status"><span>{desktop.devicesStatus === 'loading' ? '正在读取麦克风…' : deviceReady ? '已找到录音设备，下一步实际试音。' : '没有可用麦克风，请检查设备连接与系统权限。'}</span><button type="button" className="ob-link" disabled={pending} onClick={() => void desktop.refresh()}>刷新设备</button></div>
              <details className="ob-provider" open={!desktop.provider.configured || undefined}><summary><span>{desktop.provider.configured ? '服务配置已保存' : '连接云端服务'}</span><span>{desktop.provider.configured ? '查看 / 修改' : '需要 API Key'}</span></summary><ProviderSettings compact onDraftStateChange={onDraftStateChange}/></details>
              {desktop.provider.configured && <p className="ob-inline-note">配置已保存；连接、模型和录音将在真实练习中验证。</p>}
            </div>
          </div>}
          {step === 'shortcut' && <div className="ob-shortcut-stage"><div className="ob-card-heading"><KeyboardIcon/><span>你的录音快捷键</span></div><div className="ob-keyboard" aria-hidden="true"><div className="ob-key-row">{Array.from({length:6},(_,i) => <span key={i}/>)}</div><div className="ob-key-row">{Array.from({length:5},(_,i) => <span key={i}/>)}</div><div className={`ob-large-key${tested ? ' ob-key-tested' : ''}`}><Keycap value={desktop.preferences.shortcut}/>{tested && <CheckIcon/>}</div></div><p className="ob-key-status" role="status">{tested ? <><CheckCircledIcon/>已收到真实快捷键信号。</> : <><CursorTextIcon/>按下快捷键，试试看。</>}</p><p className="ob-card-footnote">这一步只测试按键，还没开始录音。</p></div>}
          {lesson && <div className="ob-practice-card">
            <div className="ob-card-heading">{lesson === 'chat' ? <ChatBubbleIcon/> : <FileTextIcon/>}<span>{lesson === 'chat' ? '团队消息' : '项目进展'}</span><span className="ob-badge">真实口述 · 练习</span></div>
            {lesson === 'chat' && <div className="ob-message-context"><div className="ob-avatar" aria-hidden="true">林</div><div><strong>{english ? 'Alex' : '小林'}</strong><p>{english ? 'What time is the meeting tomorrow?' : '明天的会议，几点方便？'}</p></div></div>}
            {lesson === 'document' && <div className="ob-document-title"><span>工作文档</span><h2>项目进展</h2></div>}
            <div className={`ob-result${hasResult ? ' ob-result-ready' : ''}`}>
              <div className="ob-result-label"><span>{hasResult ? fallback ? '识别原文' : '当前成稿' : practice.recording ? '正在听你说' : practice.busy ? '正在成稿' : '等你开口'}</span><span>{hasResult ? '尚未发送' : <Keycap value={desktop.preferences.shortcut}/>}</span></div>
              {hasResult ? <div className="ob-result-text" aria-label={fallback ? "识别原文" : "练习成稿"}>{outputText}</div> : <div className="ob-live-text" aria-label="实时识别文字">{practice.session?.partial || (practice.recording ? '开口说话，字幕会显示在这里。' : practice.busy ? '正在处理刚才的口述，请稍候。' : '按一下快捷键，或点击下方开始录音。')}</div>}
            </div>
            {hasResult && <p className={`ob-result-note${fallback ? ' ob-warning' : ''}`} role="status">{practice.rewriting ? '正在重新整理这份识别原文，无需重录。' : fallback ? `${fallbackReason}，识别原文已保留。可重试整理，或核对原文后继续。` : '这是你的真实成稿。请核对时间、数字与意思是否准确。'}</p>}
            {detail && <details className="ob-source-details"><summary>对照识别原文</summary><p>{source}</p></details>}
            <div className="ob-record-controls">
              <div className="ob-record-status"><div className="ob-level" role="meter" aria-label="麦克风输入音量" aria-valuemin={0} aria-valuemax={1} aria-valuenow={Math.max(0,Math.min(1,practice.session?.level ?? 0))}>{Array.from({length:11},(_,i) => <i key={i} style={{transform:`scaleY(${practice.recording ? Math.max(.08,Math.min(1,(practice.session?.level ?? 0) * 3) * (1 - Math.abs(5 - i) / 8)) : .08})`}} />)}</div><span role="status">{practice.recording ? practice.session?.captureReady ? practice.session?.providerReady ? '正在录音' : '正在连接识别服务' : '正在启动麦克风' : practice.busy || practice.rewriting ? '正在整理' : hasResult ? fallback ? '识别原文已保留' : '成稿已就绪' : practice.session?.state.toLowerCase() === 'cancelled' ? '本次录音已取消' : '准备录音'}</span></div>
              <button type="button" className={`ob-record-button${practice.recording ? ' ob-recording' : ''}`} disabled={practice.pending || (!practice.recording && practice.busy) || !desktop.available} onClick={() => void practice.perform(practice.recording ? 'session.stop' : 'session.start')}>{practice.recording ? <><span className="ob-stop"/>结束录音</> : <><SpeakerLoudIcon/>{hasResult || practice.session ? '再说一次' : '开始录音'}</>}</button>
              {practice.busy && <button type="button" className="ob-link" disabled={practice.pending} onClick={() => void practice.perform('session.cancel')}>取消这次录音</button>}
            </div>
            {practice.session?.error && <p className="ob-error" role="alert">{practice.session.error}</p>}
            {!practice.busy && <div className="ob-recovery-actions">{hasResult && fallback && <button type="button" className="ob-retry-rewrite" disabled={practice.pending} onClick={() => void practice.retryRewrite()}>{practice.rewriting ? '正在重试整理…' : '重试整理，无需重录'}</button>}{practice.session?.canRetryAsr && <button type="button" className="ob-link" disabled={practice.pending} onClick={() => void practice.perform('asr.retry')}>重试识别</button>}{practice.session?.canRetryRewrite && <button type="button" className="ob-link" disabled={practice.pending} onClick={() => void practice.perform('rewrite.retry')}>重试整理</button>}{practice.session?.canRetrySave && <button type="button" className="ob-link" disabled={practice.pending} onClick={() => void practice.perform('result.retrySave')}>重试保存</button>}{practice.session?.canUseSource && <button type="button" className="ob-link" disabled={practice.pending} onClick={() => void practice.perform('result.useSource')}>使用识别原文</button>}{practice.error && <button type="button" className="ob-link" onClick={practice.reload}>重新读取结果</button>}{hasResult && detail && <CopyAction record={fromDetail(detail,detail.session.storagePolicy === 'memoryOnly')} label={fallback ? "复制识别原文" : "复制成稿"}/>}</div>}
          </div>}
          {step === 'ready' && <div className="ob-finish-stage"><div className="ob-finish-mark"><CheckIcon/></div><span className="ob-eyebrow">两次口述，已经完成</span><h2>说得自然，<br/>写得合适。</h2><div className="ob-finish-items"><span><CheckCircledIcon/>聊天消息</span><span><CheckCircledIcon/>文档写作</span></div><div className="ob-delivery-note"><CursorTextIcon/><div><strong>{desktop.preferences.deliveryMode === 'direct' ? '日常交付：直接输入' : '日常交付：先预览'}</strong><p>{desktop.preferences.deliveryMode === 'direct' ? '成稿后尝试写入光标位置。无法输入时，仍可查看或复制。' : '成稿后回到主界面查看，再复制到输入框。'}</p></div></div><p className="ob-card-footnote">在设置中可再次打开引导。</p></div>}
        </div>
        <p className="ob-stage-caption">{step === 'setup' ? '先保存配置，再用真实口述验证。' : lesson ? '按你的意思成稿，由你决定怎样使用。' : 'SpeakMode · 让表达自然发生'}</p>
      </section>
    </div>
    <footer className="ob-footer"><span>SpeakMode / 开始使用</span><button type="button" className="ob-quiet" aria-pressed={manualReduced} onClick={() => setManualReduced(value => !value)}>{manualReduced ? '已减少动态效果' : '减少动态效果'}</button></footer>
  </div>;
}

function WelcomeStage() {
  return <div className="ob-welcome-stage"><div className="ob-welcome-icon"><SpeakerLoudIcon/></div><span className="ob-eyebrow">你的声音，变成文字。</span><h2>少一点斟酌，<br/>多一点表达。</h2><div className="ob-welcome-line" aria-hidden="true">{Array.from({length:25},(_,i) => <i key={i} style={{'--bar':`${10 + Math.sin(i * .8) ** 2 * 38}px`} as CSSProperties}/>)}</div><p>先试一句，再试一个场合。</p></div>;
}
