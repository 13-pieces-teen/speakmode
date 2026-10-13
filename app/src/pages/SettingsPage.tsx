import { useRef, useState } from 'react';
import { Button, Select, Switch } from '@radix-ui/themes';
import { ArrowRightIcon, CheckIcon, ChevronDownIcon, FileTextIcon, GearIcon, KeyboardIcon, MixerHorizontalIcon, ResetIcon } from '@radix-ui/react-icons';
import type { BuiltinMode, Mode } from '../model';
import { builtinModes, isBuiltinMode } from '../modes';
import { useModes } from '../hooks/useModes';
import { defaultPreferences, languageLabels, type Preferences, type RecognitionLanguage, type DeliveryMode } from '../preferences';
import { PageHeading } from '../components/ui';
import { SettingsSection, SettingRow } from '../components/SettingsControls';
import { ShortcutEditor } from '../components/ShortcutEditor';
import { ProviderSettings } from '../components/ProviderSettings';
import { useDesktop } from '../services/DesktopProvider';
import { errorMessage } from '../services/bridge';
import './settings.css';

export interface SettingsPageProps {
  defaultMode: Mode;
  onDefaultModeChange: (mode: Mode) => Promise<void>;
  preferences: Preferences;
  onPreferencesChange: (next: Preferences, captureId?: string) => Promise<void>;
  onManageModes: () => void;
}

const modeDescriptions: Record<BuiltinMode, string> = {
  自动: '根据表达内容，选择轻整理或结构。',
  轻整理: '清理口语冗余，保留原有顺序。',
  结构: '分点组织内容，保留全部信息。',
};

type FeedbackTarget = keyof Preferences | 'defaultMode' | 'reset';
type SaveFeedback = { target: FeedbackTarget; state: 'saving' | 'saved' | 'error'; message: string };

function SettingFeedback({ feedback, target }: { feedback: SaveFeedback | null; target: FeedbackTarget }) {
  if (!feedback || feedback.target !== target) return null;
  return <p className={`settings-row-feedback${feedback.state === 'error' ? ' settings-error' : ''}`}
    data-setting-feedback={target} data-feedback-state={feedback.state}
    role={feedback.state === 'error' ? 'alert' : 'status'} aria-atomic="true">
    {feedback.state === 'saved' && <CheckIcon aria-hidden="true" />}{feedback.message}
  </p>;
}

function ModeExample({ mode }: { mode: BuiltinMode }) {
  return <details className="settings-example">
    <summary><span><FileTextIcon />查看整理示例</span><ChevronDownIcon /></summary>
    <div className="settings-example-content">
      <div className="settings-example-source"><span>说出的话</span><p>嗯，下周先完成访谈，然后周五之前把原型发给大家。时间还没定，先别发邀请。</p></div>
      <div className="settings-example-output"><span>{mode === '自动' ? '自动选择：结构' : `${mode}结果`}</span>
        {mode === '轻整理'
          ? <p>下周先完成访谈，然后周五之前把原型发给大家。时间还没定，先别发邀请。</p>
          : <ol><li>下周先完成访谈。</li><li>周五之前把原型发给大家。</li><li>时间还没定，先别发邀请。</li></ol>}
      </div>
      <p className="settings-example-note">预置示例，未调用模型。整理方式会变化，时间未定与“先别发邀请”的意思都要保留。</p>
    </div>
  </details>;
}

export function SettingsPage({ defaultMode, onDefaultModeChange, preferences, onPreferencesChange, onManageModes }: SettingsPageProps) {
  const catalog = useModes();
  const { available, devices, devicesStatus, errors, command, refresh } = useDesktop();
  const [feedback, setFeedback] = useState<SaveFeedback | null>(null);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [resetRevision, setResetRevision] = useState(0);
  const disabled = !available || saving;
  const systemDefault = '__system_default__';
  const devicesReady = devicesStatus === 'ready';
  const deviceReadError = errors.find(issue => issue.scope === 'transport' && issue.operation === 'devices')?.message;
  const realDevices = devicesReady ? devices.filter(device => device.id && device.id !== systemDefault) : [];
  const selectedDeviceMissing = preferences.microphoneId !== null && !realDevices.some(device => device.id === preferences.microphoneId);

  async function save(target: FeedbackTarget, action: () => Promise<void>, message: string) {
    if (!available || savingRef.current) {
      const message = available ? '正在保存设置，请稍后再试。' : '请在 SpeakMode 桌面应用中修改设置。';
      if (!savingRef.current) setFeedback({ target, state: 'error', message });
      throw new Error(message);
    }
    savingRef.current = true;
    setSaving(true);
    setFeedback({ target, state: 'saving', message: '正在保存设置…' });
    try {
      await action();
      setFeedback({ target, state: 'saved', message });
    } catch (cause) {
      setFeedback({ target, state: 'error', message: errorMessage(cause) });
      throw cause;
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  }

  function update<K extends keyof Preferences>(key: K, value: Preferences[K], label: string, captureId?: string) {
    return save(key, () => onPreferencesChange({ ...preferences, [key]: value }, captureId), `${label}已保存。`);
  }
  // Render errors at the originating row; the shortcut editor still receives its rejecting promise.
  function handle(promise: Promise<void>) { void promise.catch(() => {}); }
  const isDefault = defaultMode === '自动' && Object.entries(defaultPreferences).every(([key, value]) => key === 'version' || preferences[key as keyof Preferences] === value);
  async function reset() {
    await save('reset', async () => {
      await command('preferences.reset', { expectedVersion: preferences.version });
      await refresh();
      setResetRevision(value => value + 1);
    }, '已恢复默认偏好；个人 Mode、已有记录和 API Key 保持不变。');
  }

  return <div className="settings-page" aria-busy={saving}>
    <PageHeading title="设置" />
    <div className="settings-preview-note"><p id="settings-preview-scope">{available
      ? '设置保存到本机。麦克风、语言与默认 Mode 的更改用于下一次口述。'
      : '当前为浏览器预览。请在 SpeakMode 桌面应用中修改设备、系统偏好和云端服务配置。'}</p></div>
    <div className="settings-sections">
      <SettingsSection title="录音与输入" icon={<KeyboardIcon />}>
        <div className="setting-row settings-shortcut-row">
          <div className="setting-row-copy"><h3>录音快捷键</h3><p>按一次开始，再按一次结束。注册成功后保存。</p></div>
          <ShortcutEditor key={resetRevision} value={preferences.shortcut} disabled={disabled} onChange={(value, captureId) => update('shortcut', value, '录音快捷键', captureId)} />
          {feedback?.state === 'saving' && <SettingFeedback feedback={feedback} target="shortcut" />}
        </div>
        <SettingRow title="麦克风" description="更改用于下一次口述，当前会话不会切换设备。" htmlFor="microphone-device" feedback={<SettingFeedback feedback={feedback} target="microphoneId" />}>
          <div className="settings-delivery-control">
            <Select.Root value={preferences.microphoneId ?? systemDefault} disabled={disabled || !devicesReady}
              onValueChange={value => handle(update('microphoneId', value === systemDefault ? null : value, '麦克风（下一次口述生效）'))}>
              <Select.Trigger id="microphone-device" aria-label="麦克风" aria-describedby="microphone-device-description microphone-device-hint" variant="surface" className="settings-select" />
              <Select.Content position="popper" sideOffset={6}>
                <Select.Item value={systemDefault}>系统默认</Select.Item>
                {selectedDeviceMissing && <Select.Item value={preferences.microphoneId!} disabled>{devicesReady ? '已选设备（当前不可用）' : '已选设备（列表待确认）'}</Select.Item>}
                {realDevices.map(device => <Select.Item key={device.id} value={device.id} disabled={device.available === false}>
                  {device.name}{device.isDefault ? '（默认）' : ''}{device.available === false ? '（不可用）' : ''}
                </Select.Item>)}
              </Select.Content>
            </Select.Root>
            <p id="microphone-device-hint" className="settings-control-hint">{!available ? '设备列表仅在桌面应用中可用。' : devicesStatus === 'error' ? '设备列表读取失败，旧列表暂不可选。已保存的麦克风选择保持不变。' : !devicesReady ? '正在读取设备列表；已保存的麦克风选择保持不变。' : !realDevices.length ? '尚未取得可用设备，请检查麦克风连接和系统权限。' : selectedDeviceMissing ? '已选设备暂不可用；请选择其他设备或系统默认。' : '设备列表来自系统；系统默认会跟随 Windows 输入设置。'}</p>
            {devicesStatus === 'error' && <div className="settings-device-recovery"><p className="settings-error" role="alert">{deviceReadError ?? '无法读取设备列表。'}</p><Button size="2" variant="soft" color="gray" disabled={disabled} onClick={() => void refresh()}>重新读取设备</Button></div>}
          </div>
        </SettingRow>
        <SettingRow title="轻语增强" description="适合安静环境下轻声输入，下次录音生效。" htmlFor="soft-voice-enabled" feedback={<SettingFeedback feedback={feedback} target="softVoiceEnabled" />}>
          <Switch id="soft-voice-enabled" aria-describedby="soft-voice-enabled-description settings-preview-scope" disabled={disabled} checked={preferences.softVoiceEnabled} onCheckedChange={value => handle(update('softVoiceEnabled', value, '轻语增强（下一次口述生效）'))} />
        </SettingRow>
        <SettingRow title="识别语言" description="中英文混合输入时，保留自动检测。下一次口述生效。" htmlFor="recognition-language" feedback={<SettingFeedback feedback={feedback} target="language" />}>
          <Select.Root value={preferences.language} disabled={disabled} onValueChange={value => handle(update('language', value as RecognitionLanguage, '识别语言（下一次口述生效）'))}>
            <Select.Trigger id="recognition-language" aria-label="识别语言" aria-describedby="recognition-language-description settings-preview-scope" variant="surface" className="settings-select" />
            <Select.Content position="popper" sideOffset={6}>
              {Object.entries(languageLabels).map(([value, label]) => <Select.Item key={value} value={value}>{label}</Select.Item>)}
            </Select.Content>
          </Select.Root>
        </SettingRow>
      </SettingsSection>

      <SettingsSection title="整理与输出" icon={<MixerHorizontalIcon />}>
        <SettingRow title="默认 Mode" description="用于下一次口述，已有记录与当前会话保持不变。" htmlFor="settings-default-mode" feedback={<SettingFeedback feedback={feedback} target="defaultMode" />}>
          <div className="settings-mode-control">
            <Select.Root value={defaultMode} disabled={disabled}
              onValueChange={value => handle(save('defaultMode', () => onDefaultModeChange(value as Mode), `默认 Mode 已保存为${catalog.label(value as Mode)}，下一次口述生效。`))}>
              <Select.Trigger id="settings-default-mode" aria-label="新口述默认 Mode" aria-describedby="settings-default-mode-description settings-mode-hint" variant="surface" className="settings-select settings-mode-select" />
              <Select.Content position="popper" sideOffset={6} className="settings-mode-menu">
                <Select.Group><Select.Label>内置模式</Select.Label>{builtinModes.map(mode => <Select.Item key={mode.id} value={mode.id}>{mode.id}</Select.Item>)}</Select.Group>
                {catalog.modes.length > 0 && <><Select.Separator /><Select.Group><Select.Label>我的 Mode</Select.Label>{catalog.modes.map(mode => <Select.Item key={mode.id} value={mode.id}><span className="settings-mode-option-label">{mode.name}</span></Select.Item>)}</Select.Group></>}
              </Select.Content>
            </Select.Root>
            <p id="settings-mode-hint" className="settings-control-hint settings-mode-description">{isBuiltinMode(defaultMode) ? modeDescriptions[defaultMode] : catalog.findMode(defaultMode)?.instructions}</p>
            <Button variant="ghost" size="2" onClick={onManageModes}>管理 Mode<ArrowRightIcon /></Button>
          </div>
        </SettingRow>
        {isBuiltinMode(defaultMode) && <ModeExample mode={defaultMode} />}
        <SettingRow title="成稿交付" description="选择结束口述后如何使用整理结果。下一次口述生效。" htmlFor="delivery-mode" feedback={<SettingFeedback feedback={feedback} target="delivery" />}>
          <div className="settings-delivery-control">
            <Select.Root value={preferences.delivery} disabled={disabled} onValueChange={value => handle(update('delivery', value as DeliveryMode, '成稿交付方式（下一次口述生效）'))}>
              <Select.Trigger id="delivery-mode" aria-label="成稿交付方式" aria-describedby="delivery-mode-description delivery-mode-hint settings-preview-scope" variant="surface" className="settings-select" />
              <Select.Content position="popper" sideOffset={6}>
                <Select.Item value="insert">直接插入</Select.Item>
                <Select.Item value="preview">先预览</Select.Item>
              </Select.Content>
            </Select.Root>
            <p className="settings-control-hint" id="delivery-mode-hint">{preferences.delivery === 'insert' ? '写入结束口述时的输入位置；无法确认时保留成稿供复制。' : '先查看结果，确认后再手动复制。'}</p>
          </div>
        </SettingRow>
      </SettingsSection>

      <SettingsSection title="通用偏好" icon={<GearIcon />}>
        <SettingRow title="保留口述记录" description="保存识别稿与整理结果，方便回看。每次开始口述时确定是否保存。" htmlFor="keep-history" feedback={<SettingFeedback feedback={feedback} target="keepHistory" />}>
          <Switch id="keep-history" aria-describedby="keep-history-description settings-preview-scope" disabled={disabled} checked={preferences.keepHistory} onCheckedChange={value => handle(update('keepHistory', value, '记录偏好（下一次口述生效）'))} />
        </SettingRow>
        {!preferences.keepHistory && <p className="settings-inline-note">新口述仅保留临时结果与不含正文的使用统计，不写入历史。已有记录不会删除，当前已开始的口述仍按开始时的选择处理。</p>}
        <SettingRow title="开机启动" description="登录 Windows 后，在托盘中待命，不自动开始录音。" htmlFor="launch-on-startup" feedback={<SettingFeedback feedback={feedback} target="launchOnStartup" />}>
          <Switch id="launch-on-startup" aria-describedby="launch-on-startup-description settings-preview-scope" disabled={disabled} checked={preferences.launchOnStartup} onCheckedChange={value => handle(update('launchOnStartup', value, '开机启动偏好'))} />
        </SettingRow>
      </SettingsSection>
      <ProviderSettings />
    </div>
    <footer className="settings-footer">
      <div className="settings-feedback"><SettingFeedback feedback={feedback} target="reset" /></div>
      <Button variant="ghost" color="gray" disabled={disabled || isDefault} onClick={() => handle(reset())}><ResetIcon />恢复默认设置</Button>
    </footer>
  </div>;
}
