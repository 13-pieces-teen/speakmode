import { useEffect, useRef, useState } from 'react';
import { Button, Select } from '@radix-ui/themes';
import { CheckIcon, LockClosedIcon } from '@radix-ui/react-icons';
import { useDesktop } from '../services/DesktopProvider';
import { errorMessage } from '../services/bridge';
import type { ProviderConfig } from '../services/contracts';
import { SettingsSection, SettingRow } from './SettingsControls';

type ProviderDraft = Pick<ProviderConfig, 'region' | 'workspace' | 'asrModel' | 'rewriteModel'>;
const UNSELECTED_REGION = '__choose_region__';
const UNSELECTED_MODEL = '__choose_model__';
function draftFrom(config: ProviderConfig): ProviderDraft {
  return { region: config.region, workspace: config.workspace, asrModel: config.asrModel, rewriteModel: config.rewriteModel };
}

export function ProviderSettings({onDraftStateChange,compact=false}:{compact?:boolean;onDraftStateChange?:(state:{dirty:boolean;pending:boolean})=>void} = {}) {
  const { available, provider, command, refresh } = useDesktop();
  const [draft, setDraft] = useState(() => draftFrom(provider.config));
  const [draftVersion, setDraftVersion] = useState(provider.config.version);
  const [dirty, setDirty] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const dirtyRef = useRef(false);
  // The secret lives only in this mounted password field and is never read back from native storage.
  const [secret, setSecret] = useState('');
  const [pending, setPending] = useState<'config' | 'key' | 'delete' | null>(null);
  const pendingRef = useRef(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const disabled = !available || pending !== null;

  useEffect(() => { onDraftStateChange?.({dirty:dirty || secret.length > 0,pending:pending !== null}); }, [dirty,secret,pending,onDraftStateChange]);

  useEffect(() => {
    if (!dirtyRef.current) {
      setDraft(draftFrom(provider.config));
      setDraftVersion(provider.config.version);
    }
  }, [provider.config]);

  function edit<K extends keyof ProviderDraft>(key: K, value: ProviderDraft[K]) {
    dirtyRef.current = true;
    setDirty(true);
    setDraft(current => ({ ...current, [key]: value }));
    setNotice('');
    setError('');
  }

  function reloadConfig() {
    setDraft(draftFrom(provider.config));
    setDraftVersion(provider.config.version);
    dirtyRef.current = false;
    setDirty(false);
    setError('');
    setNotice('已载入当前保存的配置。');
  }

  async function perform(kind: 'config' | 'key' | 'delete', action: () => Promise<void>, message: string) {
    if (!available || pendingRef.current) return;
    pendingRef.current = true;
    setPending(kind);
    setNotice('');
    setError('');
    try {
      await action();
      await refresh();
      setNotice(message);
    } catch (cause) {
      // Credential failures are generic so a returned request or secret cannot reach the UI.
      setError(kind === 'config' ? errorMessage(cause) : kind === 'key'
        ? '未能确认 API Key 已保存，请重试。'
        : '未能确认 API Key 已删除，请重试。');
    } finally {
      pendingRef.current = false;
      setPending(null);
    }
  }

  function saveConfig() {
    if (!dirty || !draft.region || !modelsCompatible || provider.config.version !== draftVersion) return;
    return perform('config', async () => {
      await command('provider.configure', {
        config: { ...provider.config, ...draft, workspace: draft.workspace?.trim() || null, asrModel: draft.asrModel.trim(), rewriteModel: draft.rewriteModel.trim(), adapterVersion: selectedAsr!.adapterVersion! },
        expectedVersion: draftVersion,
      });
      dirtyRef.current = false;
      setDirty(false);
    }, '服务配置已保存。尚未验证网络、账号权限和模型可用性。');
  }

  function saveKey() {
    if (!secret.trim() || dirty || !configSaved) return;
    return perform('key', async () => {
      await command('credential.set', { providerAccountRef: 'aliyun', expectedProviderVersion: provider.config.version, secret });
      setSecret('');
    }, 'API Key 已保存到系统凭据保管库，输入已清空；尚未验证服务连接。');
  }

  function deleteKey() {
    if (dirty || !configSaved) return;
    return perform('delete', async () => {
      await command('credential.delete', { providerAccountRef: 'aliyun', expectedProviderVersion: provider.config.version });
      setSecret('');
    }, '已删除当前地域与业务空间保存的 API Key。再次使用前需要重新配置。');
  }

  const asrModels = provider.modelCatalog?.asr ?? [];
  const rewriteModels = provider.modelCatalog?.rewrite ?? [];
  const selectedAsr = asrModels.find(model => model.id === draft.asrModel);
  const selectedRewrite = rewriteModels.find(model => model.id === draft.rewriteModel);
  const savedRewrite = rewriteModels.find(model => model.id === provider.config.rewriteModel);
  const modelsCompatible = Boolean(selectedAsr?.implemented && selectedAsr.adapterVersion && selectedRewrite?.implemented);
  const catalogReady = Boolean(provider.modelCatalog);
  const configSaved = (provider.config.region === 'beijing' || provider.config.region === 'singapore')
    && Boolean(asrModels.some(model => model.id === provider.config.asrModel && model.implemented && model.adapterVersion === provider.config.adapterVersion)
      && rewriteModels.some(model => model.id === provider.config.rewriteModel && model.implemented));
  const keyDisabled = disabled || dirty || !configSaved;
  const savedRegion = provider.config.region === 'beijing' ? '中国内地（北京）' : provider.config.region === 'singapore' ? '国际（新加坡）' : '尚未保存地域';
  const savedWorkspace = provider.config.workspace || '默认业务空间';
  const keyStatus = provider.credentialConfigured === true ? '已保存' : provider.credentialConfigured === false ? '未保存' : '尚未确认';
  const staleDraft = dirty && provider.config.version !== draftVersion;

  return <SettingsSection title="云端服务（BYOK）" icon={<LockClosedIcon />}>
    {!compact && <p className="settings-provider-intro">使用你自己的阿里云百炼 API Key。音频发送给语音识别服务，识别文本与 Mode 规则发送给整理模型。本地删除内容不等于删除云端数据。</p>}
    <div className="settings-provider-status" aria-live="polite">
      <span>配置：{configSaved ? '已保存' : '未完成'}</span>
      <span>API Key：{available ? keyStatus : '仅桌面可查询'}</span>
      {!compact && <span>已保存的整理模型：{savedRewrite?.label ?? provider.config.rewriteModel}</span>}
      <span>服务连接：尚未验证</span>
    </div>
    <p className="settings-control-hint">{compact ? '先保存账号地域，再保存 API Key。连接将在口述时验证。' : '配置与 Key 的保存状态不代表网络或模型已经可用。实际请求仍可能遇到地域、权限、余额或连接错误。'}</p>
    {compact && <Button type="button" variant="ghost" size="1" aria-expanded={advanced} onClick={() => setAdvanced(value => !value)}>{advanced ? '收起高级配置' : '模型与业务空间 · 高级配置'}</Button>}
    {provider.message && <p className="settings-inline-note">{provider.message}</p>}

    <form onSubmit={event => { event.preventDefault(); void saveConfig(); }}>
      {(!compact || advanced) && <SettingRow title="整理模型" description="负责口语整理与文字排版。选择型号并保存后，用于下一次口述。" htmlFor="provider-rewrite-model">
        <div className="settings-delivery-control">
          <Select.Root value={draft.rewriteModel || UNSELECTED_MODEL} disabled={disabled || !catalogReady} onValueChange={value => edit('rewriteModel', value)}>
            <Select.Trigger id="provider-rewrite-model" aria-label="整理模型" aria-describedby="provider-rewrite-model-description provider-rewrite-model-id" className="settings-select" variant="surface" />
            <Select.Content position="popper" sideOffset={6}>
              {!selectedRewrite && <Select.Item value={draft.rewriteModel || UNSELECTED_MODEL} disabled>当前配置（尚未确认兼容）</Select.Item>}
              {rewriteModels.map(model => <Select.Item key={model.id} value={model.id} disabled={!model.implemented}>{model.label}</Select.Item>)}
            </Select.Content>
          </Select.Root>
          <p className="settings-control-hint" id="provider-rewrite-model-id">{selectedRewrite
            ? `型号：${selectedRewrite.id}${draft.rewriteModel !== provider.config.rewriteModel ? ' · 尚未保存' : ''}`
            : '请选择已接入的整理模型。'}</p>
        </div>
      </SettingRow>}
      <SettingRow title="账号地域" description="选择 API Key 所属地域，必须与你的百炼账号配置一致。" htmlFor="provider-region">
        <Select.Root value={draft.region ?? UNSELECTED_REGION} disabled={disabled} onValueChange={value => edit('region', value === UNSELECTED_REGION ? null : value)}>
          <Select.Trigger id="provider-region" aria-label="账号地域" aria-describedby="provider-region-description" className="settings-select" variant="surface" />
          <Select.Content position="popper" sideOffset={6}>
            <Select.Item value={UNSELECTED_REGION}>请选择账号地域</Select.Item>
            <Select.Item value="beijing">中国内地（北京）</Select.Item>
            <Select.Item value="singapore">国际（新加坡）</Select.Item>
          </Select.Content>
        </Select.Root>
      </SettingRow>
      {(!compact || advanced) && <SettingRow title="业务空间" description="可选。使用默认业务空间时留空；否则填写 workspace ID。" htmlFor="provider-workspace">
        <input id="provider-workspace" className="settings-text-input" type="text" value={draft.workspace ?? ''} disabled={disabled}
          onChange={event => edit('workspace', event.target.value)} aria-describedby="provider-workspace-description" placeholder="默认业务空间" autoComplete="off" spellCheck={false} maxLength={80} />
      </SettingRow>}
      {(!compact || advanced) && <SettingRow title="语音识别模型" description="更改用于下一次口述，当前录音仍使用开始时的模型。" htmlFor="provider-asr-model">
        <div className="settings-delivery-control">
        <Select.Root value={draft.asrModel || UNSELECTED_MODEL} disabled={disabled || !catalogReady} onValueChange={value => edit('asrModel', value)}>
          <Select.Trigger id="provider-asr-model" aria-label="语音识别模型" aria-describedby="provider-asr-model-description provider-asr-capability" className="settings-select" variant="surface" />
          <Select.Content position="popper" sideOffset={6}>
            {!selectedAsr && <Select.Item value={draft.asrModel || UNSELECTED_MODEL} disabled>当前配置（尚未确认兼容）</Select.Item>}
            {asrModels.map(model => <Select.Item key={model.id} value={model.id} disabled={!model.implemented}>{model.label}</Select.Item>)}
          </Select.Content>
        </Select.Root>
        <p className="settings-control-hint" id="provider-asr-capability">{selectedAsr?.implemented
          ? selectedAsr.dictionarySupport === 'vocabularyAndContext'
            ? '支持词典热词与词汇上下文。会使用你保存的词条；识别效果可与 Qwen3 实时模型比较。'
            : '使用已有实时识别链路。词典通过旧接口传入，效果因词语与发音而异。'
          : '请选择已接入的实时识别模型。'}</p>
        </div>
      </SettingRow>}
      {staleDraft && <p className="settings-inline-note">已保存配置发生变化。当前编辑内容仍保留；请重新载入后再修改，避免覆盖其他更改。</p>}
      <div className="settings-provider-actions">
        <span className="settings-control-hint">{catalogReady ? modelsCompatible ? '模型接口已匹配；保存后仍需实际连接验证。' : '请选择已接入的模型组合。' : available ? '正在读取模型兼容清单。' : '模型清单仅在桌面应用中可用。'}</span>
        {dirty && <Button type="button" variant="ghost" color="gray" disabled={disabled} onClick={reloadConfig}>重新载入</Button>}
        <Button type="submit" size="2" disabled={disabled || !dirty || !draft.region || !modelsCompatible || staleDraft}>
          {pending === 'config' ? '正在保存…' : '保存服务配置'}
        </Button>
      </div>
    </form>

    <form className="settings-credential-form" onSubmit={event => { event.preventDefault(); void saveKey(); }}>
      <SettingRow title="API Key" description={`当前 Key 范围：${savedRegion} / ${savedWorkspace}。仅保存到系统凭据保管库，不写入普通设置；已有 Key 不会显示或回填。`} htmlFor="provider-api-key">
        <div className="settings-delivery-control">
          <input id="provider-api-key" className="settings-text-input" type="password" value={secret} disabled={disabled}
            onChange={event => { setSecret(event.target.value); setNotice(''); setError(''); }} aria-describedby="provider-api-key-description"
            placeholder={provider.credentialConfigured ? '输入新 Key 以替换' : '输入 API Key'} autoComplete="off" spellCheck={false} />
          {(dirty || !configSaved) && <p className="settings-control-hint">请先保存上方服务配置，再保存或删除该地域与业务空间的 Key。</p>}
          <div className="settings-key-actions">
            <Button type="submit" size="2" disabled={keyDisabled || !secret.trim()}>{pending === 'key' ? '正在保存…' : '保存 API Key'}</Button>
            <Button type="button" size="2" variant="ghost" color="gray" disabled={keyDisabled || provider.credentialConfigured === false}
              onClick={() => void deleteKey()}>
              {pending === 'delete' ? '正在删除…' : '删除已保存的 Key'}
            </Button>
          </div>
        </div>
      </SettingRow>
    </form>
    <div className="settings-provider-feedback">
      {error && <p className="settings-error" role="alert">{error}</p>}
      <p className="settings-provider-notice" role="status" aria-live="polite">{notice && <><CheckIcon />{notice}</>}</p>
    </div>
  </SettingsSection>;
}

