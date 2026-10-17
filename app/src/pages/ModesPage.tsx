import { useCallback, useEffect, useRef, useState } from 'react';
import { Badge, Button, IconButton, RadioCards, Select, TextArea, TextField } from '@radix-ui/themes';
import { ArrowLeftIcon, ArrowRightIcon, CheckIcon, CheckCircledIcon, FileTextIcon, Pencil2Icon, PlusIcon, TrashIcon, UpdateIcon } from '@radix-ui/react-icons';
import type { Dictation, ResolvedMode, Mode } from '../model';
import { allBuiltinModes, builtinModes, instructionTemplates, layoutInstructionTemplates, withLayoutInstruction, projectSample, type PersonalMode } from '../modes';
import { useModes, type ModeDraft as Draft } from '../hooks/useModes';
import { useDesktop } from '../services/DesktopProvider';
import { ModeIcon, OutputBlocks, PageHeading } from '../components/ui';
import { focusInView } from '../components/focusInView';
import { ExpressionPreferences } from '../components/ExpressionPreferences';
import './modes.css';

interface ModePreviewResult {
  blocks: { id: string; text: string; sourceRefs: unknown[] }[];
  resolvedMode: 'light' | 'structure';
}
interface PreviewSource {
  sourceKind: 'history' | 'synthetic';
  sourceItemCount: number;
  sourceRevisionId: string;
  promptVersion: string;
}
interface ModeComparisonResult extends PreviewSource {
  comparison: { light: ModePreviewResult; structure: ModePreviewResult };
}
type TrialResult = PreviewSource & { fingerprint: string } & (
  { kind: 'personal'; output: ModePreviewResult } |
  { kind: 'comparison'; comparison: ModeComparisonResult['comparison'] }
);
const desktopRequired = '请在 SpeakMode 桌面应用中保存、管理和试用场合。当前页面可编辑草稿与输入样例，但不会联网或保存。';

function scalarCount(value: string) {
  const characters = Array.from(value);
  return characters.some(character => {
    const point = character.codePointAt(0)!;
    return point >= 0xd800 && point <= 0xdfff;
  }) ? -1 : characters.length;
}

function errorMessage(error: unknown, fallback: string) {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === 'string' && error) return error;
  if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') return error.message;
  return fallback;
}

function isPreviewResult(value: unknown): value is ModePreviewResult {
  if (!value || typeof value !== 'object' || !('blocks' in value) || !('resolvedMode' in value)) return false;
  return (value.resolvedMode === 'light' || value.resolvedMode === 'structure') && Array.isArray(value.blocks)
    && value.blocks.length > 0 && value.blocks.every(block => block && typeof block.id === 'string'
      && typeof block.text === 'string' && Array.isArray(block.sourceRefs));
}
function hasPreviewSource(value: unknown): value is PreviewSource {
  if (!value || typeof value !== 'object') return false;
  return 'sourceKind' in value && (value.sourceKind === 'history' || value.sourceKind === 'synthetic')
    && 'sourceItemCount' in value && typeof value.sourceItemCount === 'number'
    && Number.isInteger(value.sourceItemCount) && value.sourceItemCount > 0
    && 'sourceRevisionId' in value && typeof value.sourceRevisionId === 'string'
    && 'promptVersion' in value && typeof value.promptVersion === 'string';
}
function isComparisonResult(value: unknown): value is ModeComparisonResult {
  if (!hasPreviewSource(value) || !('comparison' in value) || !value.comparison || typeof value.comparison !== 'object') return false;
  return 'light' in value.comparison && isPreviewResult(value.comparison.light) && value.comparison.light.resolvedMode === 'light'
    && 'structure' in value.comparison && isPreviewResult(value.comparison.structure) && value.comparison.structure.resolvedMode === 'structure';
}
interface Props {
  records: Dictation[];
  origin: string;
  onBack: () => void;
  onDirtyChange: (dirty: boolean) => void;
  leaving: boolean;
  onKeepEditing: () => void;
  onDiscardAndLeave: () => void;
}

function LeaveNotice({ onKeep, onDiscard }: { onKeep: () => void; onDiscard: () => void }) {
  const notice = useRef<HTMLDivElement>(null);
  useEffect(() => { focusInView(notice.current); }, []);
  return <div ref={notice} tabIndex={-1} className="mode-leave-notice" role="alert">
    <div><strong>还有未保存的修改</strong><p>可以继续编辑，或放弃这次修改后离开。</p></div>
    <div className="mode-inline-actions"><Button size="2" variant="soft" color="gray" onClick={onDiscard}>放弃修改并离开</Button><Button size="2" onClick={onKeep}>继续编辑</Button></div>
  </div>;
}

function ModeEditor({ initial, records, isNew, onSave, onClose, onDirtyChange, leaving, onKeepEditing, onDiscardAndLeave }: {
  initial: Draft; records: Dictation[]; isNew: boolean; onSave: (draft: Draft) => Promise<void>; onClose: () => void;
  onDirtyChange: (dirty: boolean) => void; leaving: boolean; onKeepEditing: () => void; onDiscardAndLeave: () => void;
}) {
  const { modes } = useModes();
  const { available, command } = useDesktop();
  const [draft, setDraft] = useState<Draft>(initial);
  const [sampleId, setSampleId] = useState('project');
  const [sample, setSample] = useState(projectSample);
  const [preview, setPreview] = useState<TrialResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [trialKind, setTrialKind] = useState<'personal' | 'comparison'>('personal');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState('');
  const [previewError, setPreviewError] = useState('');
  const [previewStatus, setPreviewStatus] = useState('');
  const [triedSubmit, setTriedSubmit] = useState(false);
  const [closePending, setClosePending] = useState(false);
  const [sampleError, setSampleError] = useState('');
  const runGeneration = useRef(0);
  const activePreview = useRef<{runId:string;generation:number}|null>(null);
  const savePending = useRef(false);
  const mounted = useRef(false);
  const nameRef = useRef<HTMLInputElement>(null);
  const instructionsRef = useRef<HTMLTextAreaElement>(null);
  const previewHeading = useRef<HTMLHeadingElement>(null);
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial);
  const isHistorySample = sampleId !== 'project' && sampleId !== 'custom';
  const historySample = isHistorySample ? records.find(record => record.id === sampleId) : undefined;
  const sampleText = isHistorySample ? historySample?.source.join('\n') ?? '' : sample;
  const inputFingerprint = JSON.stringify([sampleId, isHistorySample ? historySample?.sourceRevisionId ?? null : sample]);
  const fingerprint = JSON.stringify([draft.name, draft.base, draft.instructions, inputFingerprint]);
  const stale = preview !== null && preview.fingerprint !== (preview.kind === 'comparison' ? inputFingerprint : fingerprint);
  const currentMode = modes.find(mode => mode.id === draft.id);
  const changedElsewhere = !isNew && (!currentMode || currentMode.version !== initial.expectedRevision);
  const nameLength = scalarCount(draft.name.trim());
  const rulesLength = scalarCount(draft.instructions.trim());
  const normalizedName = draft.name.trim().toLocaleLowerCase();
  // Newly reserved scenario names must not force existing personal modes to rename.
  const keepsExistingName = !isNew && currentMode?.name.trim().toLocaleLowerCase() === normalizedName;
  const builtinNameTaken = !keepsExistingName && allBuiltinModes.some(mode => mode.id.toLocaleLowerCase() === normalizedName);
  const nameError = nameLength === 0 ? '给这个场合起一个名字。' : nameLength < 0 || nameLength > 40 ? '名称需为 1–40 个有效 Unicode 字符。' : builtinNameTaken || modes.some(mode => mode.id !== draft.id && mode.name.trim().toLocaleLowerCase() === normalizedName) ? '已经有同名场合，换个名字便于区分。' : '';
  const instructionError = rulesLength === 0 ? '写下你希望怎样整理这段表达。' : rulesLength < 0 || rulesLength > 4000 ? '整理要求需为 1–4000 个有效 Unicode 字符。' : '';
  const invalidatePreview = useCallback((cancelling = false) => {
    // Invalidate locally before the IPC round trip. Each native cancellation is
    // scoped to its run: a slow old cancel cannot stop a later preview.
    const generation = ++runGeneration.current;
    const active = activePreview.current;
    activePreview.current = null;
    setBusy(false);
    setPreviewError('');
    setPreviewStatus(cancelling && active ? '正在取消本次试用…' : '');
    if (!active) return;
    void command('mode.cancelPreview', {runId: active.runId}).then(() => {
      if (mounted.current && generation === runGeneration.current && cancelling) {
        setPreviewStatus('已取消本次试用，不会采用其返回结果。');
      }
    }).catch(() => {
      if (mounted.current && generation === runGeneration.current) {
        setPreviewStatus('');
        setPreviewError('未能确认服务已取消；本次返回仍会忽略，可重新试用。');
      }
    });
  }, [command]);
  function changeDraft(next: Draft) {
    invalidatePreview();
    setDraft(next);
    setSaveError('');
  }

  function setBase(base: ResolvedMode) {
    changeDraft({ ...draft, base, instructions: withLayoutInstruction(draft.instructions, base) });
  }
  function chooseSample(id: string) {
    invalidatePreview();
    setSampleId(id);
    if (id === 'project') setSample(projectSample);
    else if (id === 'custom') setSample(sampleText);
    setSampleError('');
  }
  useEffect(() => { focusInView(nameRef.current); }, []);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      runGeneration.current += 1;
      const active = activePreview.current;
      activePreview.current = null;
      if (active) void command('mode.cancelPreview', {runId: active.runId}).catch(() => {});
    };
  }, [command]);
  useEffect(() => { onDirtyChange(dirty); }, [dirty, onDirtyChange]);
  useEffect(() => () => { onDirtyChange(false); }, [onDirtyChange]);
  useEffect(() => { if (!available) invalidatePreview(); }, [available, invalidatePreview]);
  // A source replaced or removed by another window must invalidate an in-flight
  // trial even though the user has not touched the sample picker in this window.
  useEffect(() => { invalidatePreview(); }, [inputFingerprint, invalidatePreview]);
  async function runPreview(kind: 'personal' | 'comparison') {
    if (!available || busy || saving || activePreview.current) return;
    if (isHistorySample && !historySample) { setSampleError('这条记录已不可用，请重新选择试用文本。'); return; }
    if (!sampleText.trim()) { setSampleError('先输入一段想试用的文本。'); return; }
    if (kind === 'personal' && (nameError || instructionError)) { setTriedSubmit(true); focusInView(nameError ? nameRef.current : instructionsRef.current); return; }
    const generation = ++runGeneration.current;
    const runId = crypto.randomUUID();
    activePreview.current = {runId, generation};
    setSampleError('');
    setPreviewError('');
    setPreviewStatus('');
    setTrialKind(kind);
    setBusy(true);
    try {
      const result = await command<unknown>('mode.preview', {
        ...(kind === 'comparison' ? { compareBuiltins: true } : {
          draft: { name: draft.name.trim(), baseMode: draft.base === '轻整理' ? 'light' : 'structure', rules: draft.instructions.trim() },
        }),
        ...(historySample ? { source: { sessionId: historySample.id, sourceRevisionId: historySample.sourceRevisionId } } : { sample: sampleText.trim() }),
        runId,
        requestId: crypto.randomUUID(),
      });
      if (!mounted.current || generation !== runGeneration.current || activePreview.current?.runId !== runId) return;
      if (kind === 'comparison') {
        if (!isComparisonResult(result)) throw new Error('未收到完整的组织方式对比，请重新试用。');
        setPreview({ ...result, kind: 'comparison', fingerprint: inputFingerprint });
      } else {
        if (!isPreviewResult(result) || !hasPreviewSource(result)) throw new Error('未收到有效的整理结果，请重新试用。');
        setPreview({ ...result, kind: 'personal', output: result, fingerprint });
      }
      setPreviewStatus(kind === 'comparison' ? '同一输入，对比已更新' : '预览已更新');
      requestAnimationFrame(() => {
        if (mounted.current && generation === runGeneration.current) focusInView(previewHeading.current);
      });
    } catch (error) {
      if (mounted.current && generation === runGeneration.current) setPreviewError(errorMessage(error, '试用失败，请检查模型配置后重试。'));
    } finally {
      if (activePreview.current?.runId === runId) activePreview.current = null;
      if (mounted.current && generation === runGeneration.current) setBusy(false);
    }
  }
  function cancelPreview() { invalidatePreview(true); }
  async function save() {
    if (!available || savePending.current) return;
    setTriedSubmit(true);
    if (nameError || instructionError) { focusInView(nameError ? nameRef.current : instructionsRef.current); return; }
    savePending.current = true;
    setSaving(true);
    setSaveError('');
    invalidatePreview();
    try {
      await onSave({ ...draft, name: draft.name.trim(), instructions: draft.instructions.trim() });
    } catch (error) {
      if (mounted.current) setSaveError(errorMessage(error, '保存失败，草稿已保留，请重试。'));
    } finally {
      savePending.current = false;
      if (mounted.current) setSaving(false);
    }
  }
  return <div className="mode-editor">
    <Button className="mode-back" variant="ghost" color="gray" disabled={saving} onClick={() => dirty ? setClosePending(true) : onClose()}><ArrowLeftIcon />返回场合管理</Button>
    <div className="mode-page-header">
      <PageHeading title={isNew ? '新建自定义场合' : '编辑自定义场合'} description="描述你在什么场合使用、希望怎样表达，录音时直接选用。" />
      <Button type="submit" form="mode-editor-form" disabled={!available || saving}>{saving ? <UpdateIcon className="mode-busy-icon" /> : <CheckIcon />}{saving ? '正在保存' : '保存场合'}</Button>
    </div>
    {!available && <p className="mode-session-note" role="status">{desktopRequired}</p>}
    {saveError && <p className="mode-field-error" role="alert">{saveError} 草稿仍保留；若版本已变化，请返回管理重新打开后核对。</p>}
    {changedElsewhere && <p className="mode-field-error" role="alert">{currentMode ? '此场合已有新版本。' : '此场合已被删除。'}当前草稿仍保留，保存不会覆盖新版或重新创建；请返回管理重新打开后核对。</p>}
    {(leaving || closePending) && <LeaveNotice onKeep={() => { setClosePending(false); onKeepEditing(); }} onDiscard={leaving ? onDiscardAndLeave : onClose} />}
    <form id="mode-editor-form" noValidate onSubmit={event => { event.preventDefault(); void save(); }} className="mode-editor-grid" aria-busy={saving}>
      <section className="mode-definition surface" aria-labelledby="mode-definition-title">
        <div className="mode-section-heading"><Pencil2Icon /><h2 id="mode-definition-title">场合与表达偏好</h2></div>
        <div className="mode-field">
          <label htmlFor="personal-mode-name">名称</label>
          <TextField.Root ref={nameRef} id="personal-mode-name" value={draft.name} disabled={saving} onChange={event => changeDraft({ ...draft, name: event.target.value })} placeholder="例如：面试自我介绍、项目周报" aria-invalid={triedSubmit && Boolean(nameError)} aria-describedby={triedSubmit && nameError ? 'mode-name-hint mode-name-error' : 'mode-name-hint'} />
          <p className="mode-field-hint" id="mode-name-hint">名称 1–40 个字符。</p>
          {triedSubmit && nameError && <p className="mode-field-error" id="mode-name-error" role="alert">{nameError}</p>}
        </div>
        <div className="mode-field">
          <span id="mode-base-label" className="mode-field-label">组织方式</span>
          <RadioCards.Root className="mode-base-options" aria-labelledby="mode-base-label" value={draft.base} onValueChange={value => setBase(value as ResolvedMode)} columns="2" size="1">
            {(['轻整理', '结构'] as const).map(base => <RadioCards.Item key={base} value={base} disabled={saving || scalarCount(withLayoutInstruction(draft.instructions, base)) > 4000}><ModeIcon mode={base} /><span>{base === '轻整理' ? '自然段' : '主题分点'}</span>{draft.base === base && <CheckIcon />}</RadioCards.Item>)}
          </RadioCards.Root>
          <p className="mode-field-hint">{draft.base === '轻整理' ? '让口语更自然，保留叙述顺序。' : '把同一主题的补充和条件放在一起。'}</p>
        </div>
        <div className="mode-field">
          <ExpressionPreferences value={draft.instructions} disabled={saving} onChange={instructions => changeDraft({ ...draft, instructions })} />
          <label htmlFor="personal-mode-instructions">整理要求</label>
          <p className="mode-field-hint" id="mode-instruction-boundary">在这里选择怎样整理，口述只需说正文。口述中说出的“请分点”等请求仍作为正文保留，不会自动当成整理命令。</p>
          <div className="mode-expression-preferences">
            <div className="mode-preference-options" role="group" aria-label="写入排版要求">
              {layoutInstructionTemplates.map(template => {
                const selected=draft.base===template.base&&draft.instructions.split(/\r?\n/).some(line=>line.trim()===template.rule);
                return <Button key={template.base} type="button" size="1" variant={selected?'soft':'outline'} color={selected?'blue':'gray'} aria-pressed={selected}
                  disabled={saving || scalarCount(withLayoutInstruction(draft.instructions,template.base))>4000} onClick={()=>setBase(template.base)}>
                  {selected?<CheckIcon/>:<PlusIcon/>}{template.label}
                </Button>;
              })}
            </div>
            <p className="mode-field-hint">点选会同步组织方式，并替换已有排版模板；你自己写的其他要求会保留，请检查是否一致。</p>
          </div>
          <TextArea ref={instructionsRef} id="personal-mode-instructions" className="mode-instructions" value={draft.instructions} disabled={saving} onChange={event => changeDraft({ ...draft, instructions: event.target.value })} placeholder="例如：按自然段输出，保留中英文术语，不添加标题。" aria-invalid={triedSubmit && Boolean(instructionError)} aria-describedby="mode-instruction-boundary mode-instruction-hint mode-instruction-error" />
          <p className="mode-field-hint" id="mode-instruction-hint">这里只填写格式、语气与保留要求，正文放在试用样例或录音中。1–4000 个字符。</p>
          <p className="mode-field-error" id="mode-instruction-error" role="alert">{triedSubmit ? instructionError : ''}</p>
        </div>
        <div className="mode-preserve-note"><CheckCircledIcon /><p>整理时应保留事实、条件与否定。</p></div>
      </section>
      <section className="mode-tryout surface" aria-labelledby="mode-tryout-title">
        <div className="mode-section-heading"><FileTextIcon /><h2 id="mode-tryout-title">试用效果</h2><Badge variant="soft" color="gray">{available ? '真实试用' : '需要桌面应用'}</Badge></div>
        <div className="mode-sample-heading"><label htmlFor="mode-sample">输入样例</label>
          <Select.Root value={sampleId} onValueChange={chooseSample} disabled={saving}>
            <Select.Trigger aria-label="选择试用文本" variant="soft" color="gray" className="mode-sample-picker" />
            <Select.Content position="popper"><Select.Item value="project">项目进展示例</Select.Item><Select.Item value="custom">自行输入</Select.Item><Select.Separator />{records.map(record => <Select.Item value={record.id} key={record.id}>{record.title}</Select.Item>)}</Select.Content>
          </Select.Root>
        </div>
        <TextArea id="mode-sample" className="mode-sample" value={sampleText} disabled={saving} readOnly={isHistorySample} maxLength={4000} onChange={event => { invalidatePreview(); setSample(event.target.value); setSampleId('custom'); setSampleError(''); }} aria-invalid={Boolean(sampleError)} aria-describedby="mode-sample-error mode-sample-origin" />
        <div className="mode-sample-origin" id="mode-sample-origin">
          <span>{isHistorySample
            ? historySample ? `历史识别稿 · ${historySample.detail?.source.items.length ?? historySample.source.length} 个原始片段 · 只读` : '所选历史记录已不可用'
            : '文本样例 · 换行区分输入片段'}</span>
          {isHistorySample && historySample && <Button type="button" size="1" variant="ghost" disabled={saving} onClick={() => chooseSample('custom')}>转为自行输入</Button>}
        </div>
        <p className="mode-field-error" id="mode-sample-error" role="alert">{sampleError}</p>
        <div className="mode-try-action">
          <div className="mode-inline-actions">
            <Button type="button" variant="soft" disabled={!available || busy || saving} onClick={() => { void runPreview('personal'); }}>{busy && trialKind === 'personal' ? <UpdateIcon className="mode-busy-icon" /> : <ArrowRightIcon />}试用这个场合</Button>
            <Button type="button" variant="outline" color="gray" disabled={!available || busy || saving} onClick={() => { void runPreview('comparison'); }}>{busy && trialKind === 'comparison' && <UpdateIcon className="mode-busy-icon" />}对比两种组织方式</Button>
            {busy && <Button type="button" variant="ghost" color="gray" onClick={cancelPreview}>取消试用</Button>}
          </div>
          <p id="mode-preview-boundary">组织方式对比会发起两次云端请求，不使用上方自定义规则。</p>
        </div>
        {previewError && <p className="mode-field-error" role="alert">{previewError}</p>}
        <div className="mode-preview" data-stale={stale} aria-busy={busy}>
          <div className="mode-preview-header"><h3 ref={previewHeading} tabIndex={-1}>{preview?.kind === 'comparison' ? '组织方式对比' : '整理预览'}</h3><span role="status" aria-live="polite">{busy ? trialKind === 'comparison' ? '正在生成两份结果…' : '正在生成预览…' : stale ? preview?.kind === 'comparison' ? '输入已变化，请重新对比' : '规则或输入已变化，请重新试用' : previewStatus || (previewError ? '试用未完成' : preview ? '预览已更新' : '尚未试用')}</span></div>
          {preview ? <>
            <p className="mode-preview-origin">{preview.sourceKind === 'history' ? '历史识别稿' : '文本样例'} · {preview.sourceItemCount} 个输入片段</p>
            {preview.kind === 'comparison' ? <div className="mode-comparison" role="group" aria-label="同一输入的组织方式对比">
              {(['light', 'structure'] as const).map(mode => <section key={mode} className="mode-comparison-pane" aria-label={mode === 'light' ? '自然段结果' : '主题分点结果'}>
                <div className="mode-comparison-title"><ModeIcon mode={mode === 'light' ? '轻整理' : '结构'} /><h4>{mode === 'light' ? '自然段' : '主题分点'}</h4></div>
                <p className="mode-comparison-intent">{mode === 'light' ? '自然表达，保持叙述顺序' : '按主题归组，合并补充说明'}</p>
                <OutputBlocks texts={preview.comparison[mode].blocks.map(block => block.text)} className="mode-preview-text" />
              </section>)}
            </div> : <OutputBlocks texts={preview.output.blocks.map(block => block.text)} className="mode-preview-text" />}
            {preview.kind === 'comparison' && <p className="mode-comparison-note">简短、清楚的输入，两种结果可能相近。请一并检查事实、条件和否定是否保留。</p>}
          </> : <div className="mode-preview-empty"><p>试用你的规则，或用同一段话对比两种组织方式。</p></div>}
        </div>
        <details className="mode-preview-help"><summary>预览说明</summary><p>使用当前整理模型生成结果。历史样例保留原始识别分片；自行输入按换行分片，不代表真实录音。组织方式对比仅比较自然段与主题分点，不推断场合。试用不新增历史、不计入口述统计，也不修改已有记录。取消、修改规则/样例或离开时会取消本次请求，并忽略迟到结果。</p></details>
      </section>
    </form>
  </div>;
}

export function ModesPage({ records, origin, onBack, onDirtyChange, leaving, onKeepEditing, onDiscardAndLeave }: Props) {
  const { modes, defaultMode, setDefaultMode, saveMode, removeMode } = useModes();
  const { available } = useDesktop();
  const [editor, setEditor] = useState<{ draft: Draft; isNew: boolean } | null>(null);
  const [notice, setNotice] = useState('');
  const [actionError, setActionError] = useState('');
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [deleteId, setDeleteId] = useState<PersonalMode['id'] | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const editButtons = useRef(new Map<string, HTMLButtonElement>());
  const mounted = useRef(false);
  const editorGeneration = useRef(0);
  const actionPending = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; editorGeneration.current += 1; };
  }, []);
  useEffect(() => { focusInView(heading.current); }, []);
  function create(templateIndex = 0) {
    editorGeneration.current += 1;
    const template = instructionTemplates[templateIndex];
    setEditor({ isNew: true, draft: { id: `custom:${crypto.randomUUID()}`, name: templateIndex === 2 && !modes.some(mode => mode.name === template.name) ? template.name : '', base: template.base, instructions: template.text, expectedRevision: null } });
    setNotice(''); setActionError(''); setDeleteId(null);
  }
  function createForScenario(id:Mode) {
    const scenario = builtinModes.find(mode=>mode.id===id);
    if (!scenario) return;
    editorGeneration.current += 1;
    setEditor({isNew:true,draft:{id:`custom:${crypto.randomUUID()}`,name:'',base:'轻整理',instructions:scenario.detail,expectedRevision:null}});
    setNotice(''); setActionError(''); setDeleteId(null);
  }
  async function setBuiltinDefault(id:Mode) {
    if (!available || actionPending.current) return;
    actionPending.current=true;setPendingAction(`default:${id}`);setActionError('');
    try { await setDefaultMode(id); if(mounted.current)setNotice(`下次口述使用“${id}”。`); }
    catch(error) { if(mounted.current)setActionError(errorMessage(error,'设置默认场合失败，请重试。')); }
    finally { actionPending.current=false;if(mounted.current)setPendingAction(null); }
  }
  function edit(mode: PersonalMode) {
    editorGeneration.current += 1;
    setEditor({ draft: { id: mode.id, name: mode.name, instructions: mode.instructions, base: mode.base, expectedRevision: mode.version }, isNew: false });
    setNotice(''); setActionError(''); setDeleteId(null);
  }
  function closeEditor() {
    editorGeneration.current += 1;
    setEditor(null); onDirtyChange(false); onKeepEditing();
    requestAnimationFrame(() => { if (mounted.current) focusInView(heading.current); });
  }
  async function save(draft: Draft) {
    if (!available) throw new Error(desktopRequired);
    const generation = editorGeneration.current;
    const saved = await saveMode(draft);
    if (!mounted.current || generation !== editorGeneration.current) return;
    setNotice(`已保存“${saved.name}”。可在场合菜单选用，已有记录保持不变。`);
    closeEditor();
  }
  async function manage(mode: PersonalMode, action: 'default' | 'delete') {
    if (!available || actionPending.current) return;
    actionPending.current = true;
    setPendingAction(`${action}:${mode.id}`);
    setActionError(''); setNotice('');
    try {
      if (action === 'default') await setDefaultMode(mode.id);
      else await removeMode(mode.id);
      if (!mounted.current) return;
      if (action === 'delete') setDeleteId(null);
      setNotice(action === 'default'
        ? `下次口述使用“${mode.name}”。已有记录保持不变。`
        : `已删除“${mode.name}”，已有记录保持不变。`);
      requestAnimationFrame(() => {
        if (mounted.current) focusInView(action === 'default' ? editButtons.current.get(mode.id) ?? heading.current : heading.current);
      });
    } catch (error) {
      if (mounted.current) setActionError(errorMessage(error, action === 'default' ? '设置默认场合失败，请重试。' : '删除失败，场合已保留，请重试。'));
    } finally {
      actionPending.current = false;
      if (mounted.current) setPendingAction(null);
    }
  }
  if (editor) return <ModeEditor key={editor.draft.id} initial={editor.draft} isNew={editor.isNew} records={records} onSave={save} onClose={closeEditor} onDirtyChange={onDirtyChange} leaving={leaving} onKeepEditing={onKeepEditing} onDiscardAndLeave={onDiscardAndLeave} />;
  return <div className="mode-library">
    <Button variant="ghost" color="gray" className="mode-back" onClick={onBack}><ArrowLeftIcon />返回{origin}</Button>
    <div className="mode-page-header"><div className="page-heading"><h1 ref={heading} tabIndex={-1}>使用场合</h1><p>选择这次表达要用在什么场合，或保存自己的场合与表达偏好。</p></div><Button disabled={pendingAction !== null} onClick={() => create()}><PlusIcon />新建自定义场合</Button></div>
    <p className="mode-library-status" role="status" aria-live="polite">{notice}</p>
    {actionError && <p className="mode-field-error" role="alert">{actionError}</p>}
    {!available && <p className="mode-session-note" role="status">{desktopRequired}</p>}
    <section className="mode-personal-section surface" aria-labelledby="my-modes-title">
      <div className="mode-library-section-heading"><h2 id="my-modes-title">自定义场合</h2><span>{modes.length} 个</span></div>
      {modes.length === 0 ? <div className="mode-library-empty"><div className="mode-empty-icon"><Pencil2Icon /></div><div><p>把常用的整理要求保存下来。</p><Button variant="soft" color="gray" onClick={() => create(2)}>从“项目记录”示例开始<ArrowRightIcon /></Button></div></div>
        : <div className="personal-mode-list">{modes.map(mode => <article key={mode.id} className="personal-mode-row">
          <div className="personal-mode-icon"><Pencil2Icon /></div>
          <div className="personal-mode-copy"><div className="personal-mode-title"><h3>{mode.name}</h3>{defaultMode === mode.id && <Badge>默认</Badge>}</div><p>{mode.instructions}</p><span>{mode.base === '轻整理' ? '自然段 · 保留叙述顺序' : '主题分点 · 合并补充说明'}</span></div>
          <div className="personal-mode-actions"><Button ref={element => { if (element) editButtons.current.set(mode.id, element); else editButtons.current.delete(mode.id); }} variant="soft" color="gray" disabled={pendingAction !== null} onClick={() => edit(mode)}><Pencil2Icon />编辑</Button>{defaultMode !== mode.id && <Button variant="ghost" disabled={!available || pendingAction !== null} onClick={() => { void manage(mode, 'default'); }}>{pendingAction === `default:${mode.id}` ? '正在设置' : '设为默认'}</Button>}<IconButton variant="ghost" color="gray" disabled={!available || pendingAction !== null} aria-label={`删除 ${mode.name}`} onClick={() => { setDeleteId(mode.id); setActionError(''); }}><TrashIcon /></IconButton></div>
          {deleteId === mode.id && <div className="mode-delete-notice" role="alert"><p>删除“{mode.name}”？已有记录保留。{defaultMode === mode.id && '默认场合将恢复为自动。'}</p><div className="mode-inline-actions"><Button variant="ghost" color="gray" disabled={pendingAction !== null} onClick={() => setDeleteId(null)}>取消</Button><Button variant="soft" color="red" disabled={!available || pendingAction !== null} onClick={() => { void manage(mode, 'delete'); }}>{pendingAction === `delete:${mode.id}` ? '正在删除' : '删除场合'}</Button></div></div>}
        </article>)}</div>}
    </section>
    <section className="mode-builtins surface" aria-labelledby="builtin-modes-title"><div className="mode-library-section-heading"><h2 id="builtin-modes-title">常用场合</h2></div>
      <div className="builtin-mode-grid">{builtinModes.map(mode => <div className="builtin-mode-tile" key={mode.id}><ModeIcon mode={mode.id} /><div><strong>{mode.id}</strong><p>{mode.detail}</p><div className="mode-inline-actions">{defaultMode===mode.id?<Badge>默认</Badge>:<Button size="1" variant="soft" disabled={!available||pendingAction!==null} onClick={()=>void setBuiltinDefault(mode.id)}>设为默认</Button>}{mode.id !== '自动' && <Button size="1" variant="ghost" disabled={pendingAction !== null} onClick={() => createForScenario(mode.id)}>自定义表达<ArrowRightIcon /></Button>}</div></div></div>)}</div>
    </section>
  </div>;
}
