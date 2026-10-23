import { useEffect, useRef, useState } from 'react';
import { Button, Select } from '@radix-ui/themes';
import { CheckIcon, LockClosedIcon } from '@radix-ui/react-icons';
import { useDesktop } from '../services/DesktopProvider';
import { errorMessage } from '../services/bridge';
import type { ProviderConfig, ProviderDefinition, ProviderStatus } from '../services/contracts';
import { SettingsSection, SettingRow } from './SettingsControls';

type Stage = 'asr' | 'rewrite';
type ProviderDraft = Pick<ProviderConfig, 'provider' | 'rewriteProvider' | 'rewriteEndpoint' | 'region' | 'workspace' | 'asrModel' | 'rewriteModel'>;
type Pending = 'config' | `key:${Stage}` | `delete:${Stage}`;
type SecretDraft = { value:string; scope:string };
const UNSELECTED = '__choose__';
const EMPTY_SECRETS:Record<Stage,SecretDraft> = { asr:{value:'',scope:''}, rewrite:{value:'',scope:''} };

function draftFrom(config: ProviderConfig): ProviderDraft {
  return { provider: config.provider, rewriteProvider: config.rewriteProvider ?? 'aliyun', rewriteEndpoint: config.rewriteEndpoint ?? null,
    region: config.region, workspace: config.workspace, asrModel: config.asrModel, rewriteModel: config.rewriteModel };
}

function catalogFrom(provider: ProviderStatus): ProviderDefinition[] {
  if (provider.providerCatalog) return provider.providerCatalog;
  if (!provider.modelCatalog) return [];
  // Older snapshots expose only the selected models. They cannot authorize a new supplier.
  if (provider.config.provider === 'aliyun') return [{ id: 'aliyun', label: '阿里云百炼', ...provider.modelCatalog }];
  return [
    { id: provider.config.provider, label: provider.config.provider, asr: provider.modelCatalog.asr, rewrite: [] },
    { id: provider.config.rewriteProvider ?? 'aliyun', label: provider.config.rewriteProvider ?? '阿里云百炼', asr: [], rewrite: provider.modelCatalog.rewrite },
  ];
}

function validEndpoint(value: string | null): boolean {
  try {
    const url = new URL(value?.trim() ?? '');
    return !url.username && !url.password && !url.search && !url.hash
      && (url.protocol === 'https:' || url.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname));
  } catch { return false; }
}

function accountScope(config: ProviderDraft, stage: Stage): string {
  const id = stage === 'asr' ? config.provider : config.rewriteProvider;
  return id === 'aliyun' ? `aliyun:${config.region ?? ''}:${config.workspace?.trim() || 'default'}`
    : id === 'custom' ? `custom:${config.rewriteEndpoint?.trim().replace(/\/+$/, '') ?? ''}` : id;
}

function statusLabel(value: boolean | undefined) { return value === true ? '已保存' : value === false ? '未保存' : '尚未确认'; }

export function ProviderSettings({onDraftStateChange,compact=false}:{compact?:boolean;onDraftStateChange?:(state:{dirty:boolean;pending:boolean})=>void} = {}) {
  const { available, provider, command, refresh } = useDesktop();
  const [draft, setDraft] = useState(() => draftFrom(provider.config));
  const [draftVersion, setDraftVersion] = useState(provider.config.version);
  const [dirty, setDirty] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const dirtyRef = useRef(false);
  // Secrets are never read back and cannot move with an edited provider/account scope.
  const [secrets, setSecrets] = useState(EMPTY_SECRETS);
  const [pending, setPending] = useState<Pending | null>(null);
  const pendingRef = useRef(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const disabled = !available || pending !== null;
  const saved = draftFrom(provider.config);
  const savedAsrScope = accountScope(saved, 'asr');
  const savedRewriteScope = accountScope(saved, 'rewrite');

  useEffect(() => { onDraftStateChange?.({dirty:dirty || !!secrets.asr.value || !!secrets.rewrite.value,pending:pending !== null}); }, [dirty,secrets,pending,onDraftStateChange]);
  useEffect(() => {
    if (!dirtyRef.current) {
      setDraft(draftFrom(provider.config));
      setDraftVersion(provider.config.version);
    }
  }, [provider.config]);
  useEffect(() => { setSecrets(EMPTY_SECRETS); }, [savedAsrScope, savedRewriteScope]);

  function edit(patch: Partial<ProviderDraft>) {
    dirtyRef.current = true;
    setDirty(true);
    setDraft(current => ({ ...current, ...patch }));
    if (['provider', 'rewriteProvider', 'rewriteEndpoint', 'region', 'workspace'].some(key => key in patch)) setSecrets(EMPTY_SECRETS);
    setNotice(''); setError('');
  }
  function reloadConfig() {
    setDraft(draftFrom(provider.config)); setDraftVersion(provider.config.version);
    dirtyRef.current = false; setDirty(false); setSecrets(EMPTY_SECRETS);
    setError(''); setNotice('已载入当前保存的配置。');
  }
  async function perform(kind: Pending, action: () => Promise<void>, message: string) {
    if (!available || pendingRef.current) return;
    pendingRef.current = true; setPending(kind); setNotice(''); setError('');
    try { await action(); await refresh(); setNotice(message); }
    catch (cause) {
      // Credential failures stay generic so neither a request nor a secret reaches the UI.
      setError(kind === 'config' ? errorMessage(cause) : kind.startsWith('key:')
        ? '未能确认 API Key 已保存，请重试。' : '未能确认 API Key 已删除，请重试。');
    } finally { pendingRef.current = false; setPending(null); }
  }

  const catalog = catalogFrom(provider);
  const asrProviders = catalog.filter(item => item.asr.length > 0);
  const rewriteProviders = catalog.filter(item => item.rewrite.length > 0 || item.id === 'custom');
  const asrModels = catalog.find(item => item.id === draft.provider)?.asr ?? [];
  const rewriteModels = catalog.find(item => item.id === draft.rewriteProvider)?.rewrite ?? [];
  const selectedAsr = asrModels.find(model => model.id === draft.asrModel);
  const selectedRewrite = rewriteModels.find(model => model.id === draft.rewriteModel);
  const custom = draft.rewriteProvider === 'custom';
  const needsAliyun = draft.provider === 'aliyun' || draft.rewriteProvider === 'aliyun';
  const regionValid = !needsAliyun || draft.region === 'beijing' || draft.region === 'singapore';
  const rewriteValid = custom ? !!draft.rewriteModel.trim() && draft.rewriteModel.trim().length <= 100 && validEndpoint(draft.rewriteEndpoint)
    : !!selectedRewrite?.implemented;
  const modelsCompatible = !!selectedAsr?.implemented && !!selectedAsr.adapterVersion && rewriteValid;
  const catalogReady = catalog.length > 0;
  const savedAsr = catalog.find(item => item.id === saved.provider)?.asr.find(item => item.id === saved.asrModel);
  const savedRewrite = catalog.find(item => item.id === saved.rewriteProvider)?.rewrite.find(item => item.id === saved.rewriteModel);
  const savedAliyun = saved.provider === 'aliyun' || saved.rewriteProvider === 'aliyun';
  const configSaved = (!savedAliyun || saved.region === 'beijing' || saved.region === 'singapore')
    && !!savedAsr?.implemented && savedAsr.adapterVersion === provider.config.adapterVersion
    && (saved.rewriteProvider === 'custom' ? !!saved.rewriteModel.trim() && validEndpoint(saved.rewriteEndpoint) : !!savedRewrite?.implemented);
  const keyDisabled = disabled || dirty || !configSaved;
  const staleDraft = dirty && provider.config.version !== draftVersion;
  const sharedAliyun = saved.provider === 'aliyun' && saved.rewriteProvider === 'aliyun';
  const asrKey = provider.asrCredentialConfigured ?? (saved.provider === 'aliyun' ? provider.credentialConfigured : undefined);
  const rewriteKey = provider.rewriteCredentialConfigured ?? (sharedAliyun ? provider.credentialConfigured : undefined);
  const labelFor = (id: string) => catalog.find(item => item.id === id)?.label ?? id;
  const keyGroups: {stage:Stage;id:string;label:string;configured:boolean|undefined}[] = [
    { stage:'asr', id:saved.provider, label:sharedAliyun ? '语音识别与文字整理' : '语音识别', configured:asrKey },
    ...(!sharedAliyun ? [{ stage:'rewrite' as const, id:saved.rewriteProvider, label:'文字整理', configured:rewriteKey }] : []),
  ];

  function saveConfig() {
    if (!dirty || !regionValid || !modelsCompatible || provider.config.version !== draftVersion) return;
    return perform('config', async () => {
      await command('provider.configure', {
        config: { ...provider.config, ...draft, workspace:draft.workspace?.trim() || null,
          rewriteEndpoint:custom ? draft.rewriteEndpoint?.trim() || null : null,
          asrModel:draft.asrModel.trim(), rewriteModel:draft.rewriteModel.trim(), adapterVersion:selectedAsr!.adapterVersion! },
        expectedVersion:draftVersion,
      });
      dirtyRef.current = false; setDirty(false);
    }, '服务配置已保存。尚未验证网络、账号权限和模型可用性。');
  }
  function saveKey(stage: Stage, id: string) {
    const secret = secrets[stage].value;
    if (!secret.trim() || secrets[stage].scope !== accountScope(saved,stage) || keyDisabled) return;
    return perform(`key:${stage}`, async () => {
      await command('credential.set', {stage,providerAccountRef:id,expectedProviderVersion:provider.config.version,secret});
      setSecrets(current => ({...current,[stage]:{value:'',scope:''}}));
    }, 'API Key 已保存到系统凭据保管库，输入已清空；尚未验证服务连接。');
  }
  function deleteKey(stage: Stage, id: string) {
    if (keyDisabled) return;
    return perform(`delete:${stage}`, async () => {
      await command('credential.delete', {stage,providerAccountRef:id,expectedProviderVersion:provider.config.version});
      setSecrets(current => ({...current,[stage]:{value:'',scope:''}}));
    }, '已删除当前账号范围保存的 API Key。再次使用前需要重新配置。');
  }

  return <SettingsSection title="云端服务（BYOK）" icon={<LockClosedIcon />}>
    {!compact && <p className="settings-provider-intro">语音识别与文字整理可以使用不同供应商。音频发送给识别服务；识别文字与场合规则发送给整理服务。Key 仅保存在系统凭据保管库。</p>}
    <div className="settings-provider-status" aria-live="polite">
      <span>配置：{configSaved ? '已保存' : '未完成'}</span>
      <span>识别 Key：{available ? statusLabel(asrKey) : '仅桌面可查询'}</span>
      <span>整理 Key：{available ? statusLabel(rewriteKey) : '仅桌面可查询'}</span>
      {!compact && <span>已保存的整理模型：{savedRewrite?.label ?? saved.rewriteModel}</span>}
      <span>服务连接：尚未验证</span>
    </div>
    <p className="settings-control-hint">{compact ? '选择服务并保存配置，再填写各自的 Key。连接将在口述时验证。' : '保存配置与 Key 后，用一次口述验证连接、账号权限和模型可用性。配置更改用于下一次口述。'}</p>
    {compact && <Button type="button" variant="ghost" size="1" aria-expanded={advanced} onClick={() => setAdvanced(value => !value)}>{advanced ? '收起高级配置' : '模型与业务空间 · 高级配置'}</Button>}
    {provider.message && <p className="settings-inline-note">{provider.message}</p>}

    <form onSubmit={event => {event.preventDefault();void saveConfig();}}>
      <SettingRow title="语音识别供应商" description="接收录音并生成识别原文。" htmlFor="provider-asr-provider">
        <Select.Root value={draft.provider || UNSELECTED} disabled={disabled || !catalogReady} onValueChange={value => {
          const model = catalog.find(item => item.id === value)?.asr.find(item => item.implemented && item.adapterVersion);
          edit({provider:value,asrModel:model?.id ?? ''});
        }}>
          <Select.Trigger id="provider-asr-provider" aria-label="语音识别供应商" aria-describedby="provider-asr-provider-description" className="settings-select" variant="surface"/>
          <Select.Content position="popper" sideOffset={6}>
            {!asrProviders.some(item => item.id === draft.provider) && <Select.Item value={draft.provider || UNSELECTED} disabled>当前供应商（尚未确认兼容）</Select.Item>}
            {asrProviders.map(item => <Select.Item key={item.id} value={item.id}>{item.label}</Select.Item>)}
          </Select.Content>
        </Select.Root>
      </SettingRow>
      {draft.provider === 'google' && <p className="settings-inline-note">Google 在结束录音后上传音频并转写，录音期间不显示实时识别字幕。{selectedAsr?.productAudioLimitSeconds ? `单次录音最长 ${selectedAsr.productAudioLimitSeconds % 60 === 0 ? `${selectedAsr.productAudioLimitSeconds / 60} 分钟` : `${selectedAsr.productAudioLimitSeconds} 秒`}。` : ''}</p>}
      {(!compact || advanced) && <SettingRow title="语音识别模型" description="当前录音继续使用开始时的供应商与模型。" htmlFor="provider-asr-model">
        <div className="settings-delivery-control">
          <Select.Root key={draft.provider} value={draft.asrModel || UNSELECTED} disabled={disabled || !catalogReady} onValueChange={value => {
            if (asrModels.some(model => model.id === value)) edit({asrModel:value});
          }}>
            <Select.Trigger id="provider-asr-model" aria-label="语音识别模型" aria-describedby="provider-asr-model-description provider-asr-capability" className="settings-select" variant="surface"/>
            <Select.Content position="popper" sideOffset={6}>
              {!selectedAsr && <Select.Item value={draft.asrModel || UNSELECTED} disabled>当前配置（尚未确认兼容）</Select.Item>}
              {asrModels.map(model => <Select.Item key={model.id} value={model.id} disabled={!model.implemented}>{model.label}</Select.Item>)}
            </Select.Content>
          </Select.Root>
          <p className="settings-control-hint" id="provider-asr-capability">{!selectedAsr?.implemented ? '请选择已接入的识别模型。' : draft.provider === 'google' ? '结束录音后转写；保留识别原文，随后交给你选择的整理服务。' : selectedAsr.dictionarySupport === 'vocabularyAndContext' ? '支持词典热词与词汇上下文。会使用你保存的词条，效果因词语与发音而异。' : '使用实时识别链路；词典通过旧接口传入，效果因词语与发音而异。'}</p>
        </div>
      </SettingRow>}
      <SettingRow title="文字整理供应商" description="整理口语、保留原意，并按当前场合排版。" htmlFor="provider-rewrite-provider">
        <Select.Root value={draft.rewriteProvider || UNSELECTED} disabled={disabled || !catalogReady} onValueChange={value => {
          const model = catalog.find(item => item.id === value)?.rewrite.find(item => item.implemented);
          edit({rewriteProvider:value,rewriteModel:model?.id ?? '',rewriteEndpoint:null});
        }}>
          <Select.Trigger id="provider-rewrite-provider" aria-label="文字整理供应商" aria-describedby="provider-rewrite-provider-description" className="settings-select" variant="surface"/>
          <Select.Content position="popper" sideOffset={6}>
            {!rewriteProviders.some(item => item.id === draft.rewriteProvider) && <Select.Item value={draft.rewriteProvider || UNSELECTED} disabled>当前供应商（尚未确认兼容）</Select.Item>}
            {rewriteProviders.map(item => <Select.Item key={item.id} value={item.id}>{item.label}</Select.Item>)}
          </Select.Content>
        </Select.Root>
      </SettingRow>
      {custom && <SettingRow title="服务地址（Base URL）" description="填写兼容 Chat Completions 的服务地址，例如 https://api.example.com/v1。本机服务可用 HTTP；请勿在地址中填写 Key。" htmlFor="provider-rewrite-endpoint">
        <input id="provider-rewrite-endpoint" className="settings-text-input" type="url" value={draft.rewriteEndpoint ?? ''} disabled={disabled}
          onChange={event => edit({rewriteEndpoint:event.target.value})} aria-describedby="provider-rewrite-endpoint-description" placeholder="https://api.example.com/v1" autoComplete="off" spellCheck={false} maxLength={2048}/>
      </SettingRow>}
      {(!compact || advanced || custom) && <SettingRow title="整理模型" description={custom ? '填写该服务提供的准确模型 ID。接口兼容性会在实际口述时验证。' : '选择该供应商已接入的模型。'} htmlFor="provider-rewrite-model">
        <div className="settings-delivery-control">
          {custom ? <input id="provider-rewrite-model" className="settings-text-input" value={draft.rewriteModel} disabled={disabled}
            onChange={event => edit({rewriteModel:event.target.value})} aria-label="整理模型" aria-describedby="provider-rewrite-model-description provider-rewrite-model-id" placeholder="模型 ID" autoComplete="off" spellCheck={false} maxLength={100}/>
          : <Select.Root key={draft.rewriteProvider} value={draft.rewriteModel || UNSELECTED} disabled={disabled || !catalogReady} onValueChange={value => {
            if (rewriteModels.some(model => model.id === value)) edit({rewriteModel:value});
          }}>
            <Select.Trigger id="provider-rewrite-model" aria-label="整理模型" aria-describedby="provider-rewrite-model-description provider-rewrite-model-id" className="settings-select" variant="surface"/>
            <Select.Content position="popper" sideOffset={6}>
              {!selectedRewrite && <Select.Item value={draft.rewriteModel || UNSELECTED} disabled>当前配置（尚未确认兼容）</Select.Item>}
              {rewriteModels.map(model => <Select.Item key={model.id} value={model.id} disabled={!model.implemented}>{model.label}</Select.Item>)}
            </Select.Content>
          </Select.Root>}
          <p className="settings-control-hint" id="provider-rewrite-model-id">{selectedRewrite || custom && draft.rewriteModel.trim()
            ? `型号：${draft.rewriteModel}${draft.rewriteProvider !== saved.rewriteProvider || draft.rewriteModel !== saved.rewriteModel ? ' · 尚未保存' : ''}` : '请选择已接入的整理模型。'}</p>
        </div>
      </SettingRow>}
      {needsAliyun && <SettingRow title="阿里云账号地域" description="仅用于阿里云服务；识别与整理均选阿里云时，共用该地域与业务空间。" htmlFor="provider-region">
        <Select.Root value={draft.region ?? UNSELECTED} disabled={disabled} onValueChange={value => edit({region:value === UNSELECTED ? null : value})}>
          <Select.Trigger id="provider-region" aria-label="阿里云账号地域" aria-describedby="provider-region-description" className="settings-select" variant="surface"/>
          <Select.Content position="popper" sideOffset={6}>
            <Select.Item value={UNSELECTED}>请选择账号地域</Select.Item><Select.Item value="beijing">中国内地（北京）</Select.Item><Select.Item value="singapore">国际（新加坡）</Select.Item>
          </Select.Content>
        </Select.Root>
      </SettingRow>}
      {needsAliyun && (!compact || advanced) && <SettingRow title="阿里云业务空间" description="使用默认业务空间时留空；否则填写 workspace ID。" htmlFor="provider-workspace">
        <input id="provider-workspace" className="settings-text-input" value={draft.workspace ?? ''} disabled={disabled} onChange={event => edit({workspace:event.target.value})}
          aria-describedby="provider-workspace-description" placeholder="默认业务空间" autoComplete="off" spellCheck={false} maxLength={80}/>
      </SettingRow>}
      {staleDraft && <p className="settings-inline-note">已保存配置发生变化。当前编辑内容仍保留；请重新载入后再修改，避免覆盖其他更改。</p>}
      <div className="settings-provider-actions">
        <span className="settings-control-hint">{catalogReady ? modelsCompatible && regionValid ? '保存后仍需实际连接验证。' : '请完成所选服务的模型、地址与账号地域配置。' : available ? '正在读取模型兼容清单。' : '模型清单仅在桌面应用中可用。'}</span>
        {dirty && <Button type="button" variant="ghost" color="gray" disabled={disabled} onClick={reloadConfig}>重新载入</Button>}
        <Button type="submit" size="2" disabled={disabled || !dirty || !regionValid || !modelsCompatible || staleDraft}>{pending === 'config' ? '正在保存…' : '保存服务配置'}</Button>
      </div>
    </form>

    {keyGroups.map(group => {
      const inputId = group.stage === 'asr' ? 'provider-api-key' : 'provider-rewrite-api-key';
      const secret = secrets[group.stage].scope === accountScope(saved,group.stage) ? secrets[group.stage].value : '';
      const scope = group.id === 'aliyun' ? `${saved.region === 'beijing' ? '中国内地（北京）' : saved.region === 'singapore' ? '国际（新加坡）' : '尚未保存地域'} / ${saved.workspace || '默认业务空间'}` : group.id === 'custom' ? saved.rewriteEndpoint || '尚未保存服务地址' : labelFor(group.id);
      return <form key={`${group.stage}:${accountScope(saved,group.stage)}`} className="settings-credential-form" onSubmit={event => {event.preventDefault();void saveKey(group.stage,group.id);}}>
        <SettingRow title={`${group.label} Key`} description={`${labelFor(group.id)} · ${scope}。${sharedAliyun ? '这一个 Key 同时用于识别与整理。' : ''}已有 Key 不会显示或回填。`} htmlFor={inputId}>
          <div className="settings-delivery-control">
            <p className="settings-control-hint">API Key：{available ? statusLabel(group.configured) : '仅桌面可查询'}</p>
            <input id={inputId} className="settings-text-input" type="password" value={secret} disabled={keyDisabled}
              onChange={event => {const value=event.target.value,scope=accountScope(saved,group.stage);setSecrets(current => ({...current,[group.stage]:{value,scope}}));setNotice('');setError('');}}
              aria-describedby={`${inputId}-description`} placeholder={group.configured ? '输入新 Key 以替换' : '输入 API Key'} autoComplete="off" spellCheck={false}/>
            {(dirty || !configSaved) && <p className="settings-control-hint">请先保存上方服务配置，再保存或删除对应账号的 Key。</p>}
            <div className="settings-key-actions">
              <Button type="submit" size="2" disabled={keyDisabled || !secret.trim()}>{pending === `key:${group.stage}` ? '正在保存…' : '保存 API Key'}</Button>
              <Button type="button" size="2" variant="ghost" color="gray" disabled={keyDisabled || group.configured === false} onClick={() => void deleteKey(group.stage,group.id)}>{pending === `delete:${group.stage}` ? '正在删除…' : '删除已保存的 Key'}</Button>
            </div>
          </div>
        </SettingRow>
      </form>;
    })}
    <div className="settings-provider-feedback">
      {error && <p className="settings-error" role="alert">{error}</p>}
      <p className="settings-provider-notice" role="status" aria-live="polite">{notice && <><CheckIcon/>{notice}</>}</p>
    </div>
  </SettingsSection>;
}
