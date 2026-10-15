import { useRef, useState } from 'react';
import { Button, Switch, TextField } from '@radix-ui/themes';
import { PlusIcon, Cross2Icon } from '@radix-ui/react-icons';
import { applicationKey, applicationLabel, writingApplications, writingSceneLabels, type AppWritingPreferences, type AppWritingScene } from '../appWriting';
import { SettingRow } from './SettingsControls';

interface Props { value: AppWritingPreferences; disabled: boolean; onChange: (next: AppWritingPreferences) => Promise<void> }
function SceneOptions() {
  return Object.entries(writingSceneLabels).map(([id, label]) => <option key={id} value={id}>{label}</option>);
}
export function AppWritingSettings({ value, disabled, onChange }: Props) {
  const [preset, setPreset] = useState('wechat.exe');
  const [custom, setCustom] = useState('');
  const [scene, setScene] = useState<AppWritingScene>('chat');
  const [error, setError] = useState('');
  const [example, setExample] = useState<AppWritingScene>('chat');
  const [excludedApplication, setExcludedApplication] = useState('');
  const [exclusionError, setExclusionError] = useState('');
  const applicationSelect = useRef<HTMLSelectElement>(null);
  const exclusionInput = useRef<HTMLInputElement>(null);
  const fieldDisabled = disabled || !value.enabled;
  const handle = (promise: Promise<void>) => { void promise.catch(() => {}); };
  async function add() {
    const key = applicationKey(preset === 'custom' ? custom : preset);
    if (!key) { setError('请填写程序名，例如 Code.exe，不包含文件路径。'); return; }
    if (value.overrides.some(item => item.application.toLowerCase() === key)) { setError('这个应用已有偏好，请在上方修改。'); return; }
    if (value.overrides.length >= 32) { setError('应用偏好最多保存 32 项，请先移除不需要的项目。'); return; }
    setError('');
    try {
      await onChange({ ...value, overrides: [...value.overrides, { application: key, scene }] });
      setCustom('');
      const nextApp = writingApplications.find(app => app.id !== key && !value.overrides.some(item => item.application === app.id));
      setPreset(nextApp?.id ?? 'custom'); setScene(nextApp?.scene ?? 'general');
      applicationSelect.current?.focus();
    } catch { /* The originating settings row displays the persistence error. */ }
  }
  function selectPreset(next: string) {
    setPreset(next); setError('');
    setScene(writingApplications.find(app => app.id === next)?.scene ?? 'general');
  }
  async function exclude() {
    const key = applicationKey(excludedApplication);
    if (!key) { setExclusionError('请填写程序名，例如 Code.exe，不包含文件路径。'); return; }
    if (value.excludedApplications.includes(key)) { setExclusionError('这个应用已被排除。'); return; }
    if (value.excludedApplications.length >= 32) { setExclusionError('最多排除 32 个应用。'); return; }
    setExclusionError('');
    try { await onChange({...value,excludedApplications:[...value.excludedApplications,key]}); setExcludedApplication(''); exclusionInput.current?.focus(); }
    catch { /* Settings feedback owns persistence errors. */ }
  }
  return <div className="app-writing-settings">
    <SettingRow title="成稿语言" htmlFor="draft-output-language" description="独立于识别语言。英文成稿会整理中英文卡顿，只在含义明确时恢复英文术语。">
      <select id="draft-output-language" aria-label="成稿语言" disabled={disabled} value={value.outputLanguage}
        onChange={event=>handle(onChange({...value,outputLanguage:event.target.value as AppWritingPreferences['outputLanguage']}))}>
        <option value="asSpoken">跟随口述语言</option><option value="english">英文成稿</option>
      </select>
    </SettingRow>
    <SettingRow title="输入位置感知" htmlFor="input-context-enabled"
      description="将当前输入框附近的文字与选中文字交给已配置的云端整理模型，帮助判断场合、术语和续写语气。默认关闭。">
      <Switch id="input-context-enabled" checked={value.inputContextEnabled} disabled={disabled} aria-describedby="input-context-enabled-description input-context-privacy"
        onCheckedChange={inputContextEnabled=>handle(onChange({...value,inputContextEnabled}))}/>
    </SettingRow>
    <p id="input-context-privacy" className="settings-control-hint">开启后只尝试读取当前聚焦输入框内的有限上下文；不采集整页或截图。无法读取或位置已变化时，按口述内容整理。更改用于下一次口述。</p>
    <details className="input-context-exclusions">
      <summary>排除应用{value.excludedApplications.length ? `（${value.excludedApplications.length}）` : ''}</summary>
      <p className="settings-control-hint">这些应用中的输入内容不会用作上下文。排除浏览器会同时排除它的所有页面。</p>
      {value.excludedApplications.length > 0 && <ul className="input-context-exclusion-list" aria-label="不读取输入上下文的应用">
        {value.excludedApplications.map(application=><li key={application}><span>{applicationLabel(application)} <small>{application}</small></span>
          <Button variant="ghost" color="gray" disabled={disabled} aria-label={`取消排除${applicationLabel(application)}`}
            onClick={()=>handle(onChange({...value,excludedApplications:value.excludedApplications.filter(item=>item!==application)}).then(()=>{setExclusionError('');exclusionInput.current?.focus();}))}><Cross2Icon/></Button></li>)}
      </ul>}
      <div className="input-context-exclusion-editor">
        <label htmlFor="input-context-excluded-app">程序名<TextField.Root ref={exclusionInput} id="input-context-excluded-app" placeholder="例如 Code.exe" disabled={disabled} value={excludedApplication} maxLength={80} aria-describedby="input-context-exclusion-error"
          onChange={event=>{setExcludedApplication(event.target.value);setExclusionError('');}}/></label>
        <Button variant="soft" color="gray" disabled={disabled||value.excludedApplications.length>=32} onClick={()=>void exclude()}><PlusIcon/>排除此应用</Button>
      </div>
      <p id="input-context-exclusion-error" className="settings-error" role="alert">{exclusionError}</p>
    </details>
    <SettingRow title="自动匹配应用场合" htmlFor="app-writing-enabled"
      description="自动场合会参考应用类型。手动选择的场合和自定义规则优先。">
      <Switch id="app-writing-enabled" checked={value.enabled} disabled={disabled}
        aria-describedby="app-writing-enabled-description app-writing-privacy"
        onCheckedChange={enabled => handle(onChange({ ...value, enabled }))} />
    </SettingRow>
    <p id="app-writing-privacy" className="settings-control-hint">应用匹配只使用程序名。输入位置感知单独控制是否读取附近文字；偏好更改用于下一次口述。</p>
    {value.enabled && <div className="app-writing-preferences">
      <p className="settings-control-hint">常见聊天应用、Word 和代码编辑器会自动匹配。浏览器使用通用场合；开启输入位置感知后，可用的输入框上下文帮助区分聊天与文档。</p>
      {value.overrides.length > 0 && <ul className="app-writing-list" aria-label="应用表达偏好">
        {value.overrides.map(item => <li key={item.application}>
          <span className="app-writing-app"><strong>{applicationLabel(item.application)}</strong><small>{item.application}</small></span>
          <select aria-label={`${applicationLabel(item.application)}的表达场景`} value={item.scene} disabled={fieldDisabled}
            onChange={event => handle(onChange({ ...value, overrides: value.overrides.map(row => row.application === item.application ? { ...row, scene: event.target.value as AppWritingScene } : row) }))}><SceneOptions /></select>
          <Button variant="ghost" color="gray" disabled={fieldDisabled} aria-label={`移除${applicationLabel(item.application)}偏好`}
            onClick={() => handle(onChange({ ...value, overrides: value.overrides.filter(row => row.application !== item.application) }).then(() => applicationSelect.current?.focus()))}><Cross2Icon /></Button>
        </li>)}
      </ul>}
      <fieldset className="app-writing-editor" disabled={fieldDisabled}>
        <legend>添加应用偏好</legend>
        <label>应用<select ref={applicationSelect} aria-label="选择应用" value={preset} onChange={event => selectPreset(event.target.value)}>
          {writingApplications.map(app => <option key={app.id} value={app.id}>{app.name}</option>)}<option value="custom">其他应用</option>
        </select></label>
        <label>表达场景<select aria-label="新应用的表达场景" value={scene} onChange={event => setScene(event.target.value as AppWritingScene)}><SceneOptions /></select></label>
        <Button type="button" variant="soft" color="gray" disabled={fieldDisabled || value.overrides.length >= 32} onClick={() => void add()}><PlusIcon />添加偏好</Button>
        {preset === 'custom' && <label className="app-writing-custom">程序名<TextField.Root aria-label="应用程序名" placeholder="例如 Notepad.exe" value={custom} maxLength={80} onChange={event => { setCustom(event.target.value); setError(''); }} /></label>}
      </fieldset>
      <p className="settings-control-hint">浏览器偏好适用于该浏览器的所有页面。选择“通用”时按口述和可用上下文判断；移除偏好则恢复自动匹配。</p>
      {error && <p role="alert" className="settings-error">{error}</p>}
    </div>}
    <details className="settings-example app-writing-example">
      <summary>查看不同场景的呈现</summary>
      <label className="app-writing-example-picker">示例场景<select aria-label="示例场景" value={example} onChange={event => setExample(event.target.value as AppWritingScene)}><SceneOptions /></select></label>
      <div className="settings-example-content">
        <div className="settings-example-source"><span>说出的话</span><p>帮我先检查麦克风，再录十秒音频。断开就停下来，不要自动外放。</p></div>
        <div className="settings-example-output"><span>{writingSceneLabels[example]}</span>{example !== 'technical'
          ? <p>帮我先检查麦克风，再录十秒音频。<br />断开就停下来，不要自动外放。</p>
          : <ol><li>帮我先检查麦克风，再录十秒音频。</li><li>断开就停下来，不要自动外放。</li></ol>}</div>
        <p className="settings-example-note">预置排版示意。实际成稿保留口述中的意思、条件与代码；通用场景由内容决定。</p>
      </div>
    </details>
  </div>;
}
