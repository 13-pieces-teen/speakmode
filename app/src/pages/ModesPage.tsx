import { useEffect, useRef, useState } from 'react';
import { Badge, Button, IconButton, RadioCards, Select, TextArea, TextField } from '@radix-ui/themes';
import { ArrowLeftIcon, ArrowRightIcon, CheckIcon, CheckCircledIcon, FileTextIcon, Pencil2Icon, PlusIcon, TrashIcon, UpdateIcon } from '@radix-ui/react-icons';
import type { Dictation, ResolvedMode } from '../model';
import { builtinModes, instructionTemplates, projectSample, type PersonalMode } from '../modes';
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
const desktopRequired = '请在 SpeakMode 桌面应用中保存、管理和试用 Mode。当前页面可编辑草稿与输入样例，但不会联网或保存。';

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
  const [preview, setPreview] = useState<(ModePreviewResult & { fingerprint: string }) | null>(null);
  const [busy, setBusy] = useState(false);
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
  const fingerprint = JSON.stringify([draft.name, draft.base, draft.instructions, sample]);
  const stale = preview !== null && preview.fingerprint !== fingerprint;
  const currentMode = modes.find(mode => mode.id === draft.id);
  const changedElsewhere = !isNew && (!currentMode || currentMode.version !== initial.expectedRevision);
  const nameLength = scalarCount(draft.name.trim());
  const rulesLength = scalarCount(draft.instructions.trim());
  const nameError = nameLength === 0 ? '给这个 Mode 起一个名字。' : nameLength < 0 || nameLength > 40 ? '名称需为 1–40 个有效 Unicode 字符。' : builtinModes.some(mode => mode.id === draft.name.trim()) || modes.some(mode => mode.id !== draft.id && mode.name.toLocaleLowerCase() === draft.name.trim().toLocaleLowerCase()) ? '已经有同名 Mode，换个名字便于区分。' : '';
  const instructionError = rulesLength === 0 ? '写下你希望怎样整理这段表达。' : rulesLength < 0 || rulesLength > 4000 ? '整理要求需为 1–4000 个有效 Unicode 字符。' : '';
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
  useEffect(() => { if (!available) invalidatePreview(); }, [available]);

  function invalidatePreview(cancelling = false) {
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
  }
  function changeDraft(next: Draft) {
    invalidatePreview();
    setDraft(next);
    setSaveError('');
  }

  function setBase(base: ResolvedMode) {
    changeDraft({ ...draft, base,
      instructions: instructionTemplates.slice(0, 2).some(template => template.text === draft.instructions)
        ? instructionTemplates[base === '轻整理' ? 0 : 1].text : draft.instructions,
    });
  }
  function chooseSample(id: string) {
    invalidatePreview();
    setSampleId(id);
    if (id !== 'custom') setSample(id === 'project' ? projectSample : records.find(record => record.id === id)?.source.join('\n') ?? '');
    setSampleError('');
  }
  async function runPreview() {
    if (!available || busy || saving) return;
    if (!sample.trim()) { setSampleError('先输入一段想试用的文本。'); return; }
    if (nameError || instructionError) { setTriedSubmit(true); focusInView(nameError ? nameRef.current : instructionsRef.current); return; }
    const generation = ++runGeneration.current;
    const runId = crypto.randomUUID();
    activePreview.current = {runId, generation};
    setSampleError('');
    setPreviewError('');
    setPreviewStatus('');
    setBusy(true);
    try {
      const result = await command<ModePreviewResult>('mode.preview', {
        draft: { name: draft.name.trim(), baseMode: draft.base === '轻整理' ? 'light' : 'structure', rules: draft.instructions.trim() },
        sample: sample.trim(),
        runId,
        requestId: crypto.randomUUID(),
      });
      if (!mounted.current || generation !== runGeneration.current || activePreview.current?.runId !== runId) return;
      if (!isPreviewResult(result)) throw new Error('未收到有效的整理结果，请重新试用。');
      setPreview({ ...result, fingerprint });
      setPreviewStatus('预览已更新');
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
    <Button className="mode-back" variant="ghost" color="gray" disabled={saving} onClick={() => dirty ? setClosePending(true) : onClose()}><ArrowLeftIcon />返回 Mode 管理</Button>
    <div className="mode-page-header">
      <PageHeading title={isNew ? '新建 Mode' : '编辑 Mode'} description="保存常用的整理规则，录音时直接选用。" />
      <Button type="submit" form="mode-editor-form" disabled={!available || saving}>{saving ? <UpdateIcon className="mode-busy-icon" /> : <CheckIcon />}{saving ? '正在保存' : '保存 Mode'}</Button>
    </div>
    {!available && <p className="mode-session-note" role="status">{desktopRequired}</p>}
    {saveError && <p className="mode-field-error" role="alert">{saveError} 草稿仍保留；若版本已变化，请返回管理重新打开后核对。</p>}
    {changedElsewhere && <p className="mode-field-error" role="alert">{currentMode ? '此 Mode 已有新版本。' : '此 Mode 已被删除。'}当前草稿仍保留，保存不会覆盖新版或重新创建；请返回管理重新打开后核对。</p>}
    {(leaving || closePending) && <LeaveNotice onKeep={() => { setClosePending(false); onKeepEditing(); }} onDiscard={leaving ? onDiscardAndLeave : onClose} />}
    <form id="mode-editor-form" noValidate onSubmit={event => { event.preventDefault(); void save(); }} className="mode-editor-grid" aria-busy={saving}>
      <section className="mode-definition surface" aria-labelledby="mode-definition-title">
        <div className="mode-section-heading"><Pencil2Icon /><h2 id="mode-definition-title">整理规则</h2></div>
        <div className="mode-field">
          <label htmlFor="personal-mode-name">名称</label>
          <TextField.Root ref={nameRef} id="personal-mode-name" value={draft.name} disabled={saving} onChange={event => changeDraft({ ...draft, name: event.target.value })} placeholder="例如：项目记录、发给同学" aria-invalid={triedSubmit && Boolean(nameError)} aria-describedby={triedSubmit && nameError ? 'mode-name-hint mode-name-error' : 'mode-name-hint'} />
          <p className="mode-field-hint" id="mode-name-hint">名称 1–40 个字符。</p>
          {triedSubmit && nameError && <p className="mode-field-error" id="mode-name-error" role="alert">{nameError}</p>}
        </div>
        <div className="mode-field">
          <span id="mode-base-label" className="mode-field-label">基础整理方式</span>
          <RadioCards.Root className="mode-base-options" aria-labelledby="mode-base-label" value={draft.base} onValueChange={value => setBase(value as ResolvedMode)} columns="2" size="1">
            {(['轻整理', '结构'] as const).map(base => <RadioCards.Item key={base} value={base} disabled={saving}><ModeIcon mode={base} /><span>{base}</span>{draft.base === base && <CheckIcon />}</RadioCards.Item>)}
          </RadioCards.Root>
          <p className="mode-field-hint">{draft.base === '轻整理' ? '保留说话顺序。' : '按要点分组。'}</p>
        </div>
        <div className="mode-field">
          <ExpressionPreferences value={draft.instructions} disabled={saving} onChange={instructions => changeDraft({ ...draft, instructions })} />
          <label htmlFor="personal-mode-instructions">整理要求</label>
          <TextArea ref={instructionsRef} id="personal-mode-instructions" className="mode-instructions" value={draft.instructions} disabled={saving} onChange={event => changeDraft({ ...draft, instructions: event.target.value })} placeholder="说清楚希望的格式、语气，以及需要保留什么。" aria-invalid={triedSubmit && Boolean(instructionError)} aria-describedby="mode-instruction-hint mode-instruction-error" />
          <p className="mode-field-hint" id="mode-instruction-hint">用自己的话描述格式和语气，1–4000 个字符。</p>
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
        <TextArea id="mode-sample" className="mode-sample" value={sample} disabled={saving} maxLength={4000} onChange={event => { invalidatePreview(); setSample(event.target.value); setSampleId('custom'); setSampleError(''); }} aria-invalid={Boolean(sampleError)} aria-describedby="mode-sample-error mode-preview-boundary" />
        <p className="mode-field-error" id="mode-sample-error" role="alert">{sampleError}</p>
        <div className="mode-try-action"><span id="mode-preview-boundary">上方是输入样例，试用后才会生成结果。</span><div className="mode-inline-actions">{busy && <Button type="button" variant="ghost" color="gray" onClick={cancelPreview}>取消试用</Button>}<Button type="button" variant="soft" disabled={!available || busy || saving} onClick={() => { void runPreview(); }}>{busy ? <UpdateIcon className="mode-busy-icon" /> : <ArrowRightIcon />}{busy ? '正在试用' : '试用这一段'}</Button></div></div>
        {previewError && <p className="mode-field-error" role="alert">{previewError}</p>}
        <div className="mode-preview" data-stale={stale} aria-busy={busy}>
          <div className="mode-preview-header"><h3 ref={previewHeading} tabIndex={-1}>整理预览</h3><span role="status" aria-live="polite">{busy ? '正在生成预览' : previewStatus || (stale ? '规则或文本已修改，待更新' : previewError ? '试用未完成' : preview ? '预览已更新' : '尚未试用')}</span></div>
          {preview ? <OutputBlocks texts={preview.blocks.map(block => block.text)} className="mode-preview-text" />
            : <div className="mode-preview-empty"><p>点击“试用这一段”查看结果。</p></div>}
        </div>
        <details className="mode-preview-help"><summary>预览说明</summary><p>模板提供整理规则和输入样例；点击试用后，使用当前模型生成结果。试用不新增历史、不计入口述统计，也不修改已有记录。取消、修改规则/样例或离开时会按本次请求身份通知服务取消，并忽略迟到结果。</p></details>
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
    setNotice(`已保存“${saved.name}”。可在 Mode 菜单选用，已有记录保持不变。`);
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
      if (mounted.current) setActionError(errorMessage(error, action === 'default' ? '设置默认 Mode 失败，请重试。' : '删除失败，Mode 已保留，请重试。'));
    } finally {
      actionPending.current = false;
      if (mounted.current) setPendingAction(null);
    }
  }
  if (editor) return <ModeEditor key={editor.draft.id} initial={editor.draft} isNew={editor.isNew} records={records} onSave={save} onClose={closeEditor} onDirtyChange={onDirtyChange} leaving={leaving} onKeepEditing={onKeepEditing} onDiscardAndLeave={onDiscardAndLeave} />;
  return <div className="mode-library">
    <Button variant="ghost" color="gray" className="mode-back" onClick={onBack}><ArrowLeftIcon />返回{origin}</Button>
    <div className="mode-page-header"><div className="page-heading"><h1 ref={heading} tabIndex={-1}>Mode 管理</h1><p>选择内置模式，或保存自己的整理规则。</p></div><Button disabled={pendingAction !== null} onClick={() => create()}><PlusIcon />新建 Mode</Button></div>
    <p className="mode-library-status" role="status" aria-live="polite">{notice}</p>
    {actionError && <p className="mode-field-error" role="alert">{actionError}</p>}
    {!available && <p className="mode-session-note" role="status">{desktopRequired}</p>}
    <section className="mode-personal-section surface" aria-labelledby="my-modes-title">
      <div className="mode-library-section-heading"><h2 id="my-modes-title">我的 Mode</h2><span>{modes.length} 个</span></div>
      {modes.length === 0 ? <div className="mode-library-empty"><div className="mode-empty-icon"><Pencil2Icon /></div><div><p>把常用的整理要求保存下来。</p><Button variant="soft" color="gray" onClick={() => create(2)}>从“项目记录”示例开始<ArrowRightIcon /></Button></div></div>
        : <div className="personal-mode-list">{modes.map(mode => <article key={mode.id} className="personal-mode-row">
          <div className="personal-mode-icon"><Pencil2Icon /></div>
          <div className="personal-mode-copy"><div className="personal-mode-title"><h3>{mode.name}</h3>{defaultMode === mode.id && <Badge>默认</Badge>}</div><p>{mode.instructions}</p><span>基于{mode.base}</span></div>
          <div className="personal-mode-actions"><Button ref={element => { if (element) editButtons.current.set(mode.id, element); else editButtons.current.delete(mode.id); }} variant="soft" color="gray" disabled={pendingAction !== null} onClick={() => edit(mode)}><Pencil2Icon />编辑</Button>{defaultMode !== mode.id && <Button variant="ghost" disabled={!available || pendingAction !== null} onClick={() => { void manage(mode, 'default'); }}>{pendingAction === `default:${mode.id}` ? '正在设置' : '设为默认'}</Button>}<IconButton variant="ghost" color="gray" disabled={!available || pendingAction !== null} aria-label={`删除 ${mode.name}`} onClick={() => { setDeleteId(mode.id); setActionError(''); }}><TrashIcon /></IconButton></div>
          {deleteId === mode.id && <div className="mode-delete-notice" role="alert"><p>删除“{mode.name}”？已有记录保留。{defaultMode === mode.id && '默认 Mode 将恢复为自动。'}</p><div className="mode-inline-actions"><Button variant="ghost" color="gray" disabled={pendingAction !== null} onClick={() => setDeleteId(null)}>取消</Button><Button variant="soft" color="red" disabled={!available || pendingAction !== null} onClick={() => { void manage(mode, 'delete'); }}>{pendingAction === `delete:${mode.id}` ? '正在删除' : '删除 Mode'}</Button></div></div>}
        </article>)}</div>}
    </section>
    <section className="mode-builtins surface" aria-labelledby="builtin-modes-title"><div className="mode-library-section-heading"><h2 id="builtin-modes-title">内置模式</h2></div>
      <div className="builtin-mode-grid">{builtinModes.map(mode => <div className="builtin-mode-tile" key={mode.id}><ModeIcon mode={mode.id} /><div><strong>{mode.id}</strong><p>{mode.caption}</p>{mode.id !== '自动' && <Button size="1" variant="ghost" disabled={pendingAction !== null} onClick={() => create(mode.id === '结构' ? 1 : 0)}>以此创建<ArrowRightIcon /></Button>}</div></div>)}</div>
    </section>
  </div>;
}
