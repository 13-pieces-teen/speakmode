import {useCallback, useEffect, useMemo, useRef, useState} from 'react';
import {Button, IconButton, TextArea, TextField} from '@radix-ui/themes';
import {CheckIcon, Cross2Icon, MagnifyingGlassIcon, Pencil2Icon, PlusIcon, ReaderIcon, ReloadIcon, TrashIcon} from '@radix-ui/react-icons';
import {useDesktop} from '../services/DesktopProvider';
import {errorMessage} from '../services/bridge';
import type {DictionaryEntry, DictionarySnapshot} from '../services/contracts';
import {dictionaryConflict, dictionaryContextBytes, dictionaryDraftCapacity, dictionaryLimits, dictionarySearchKey, dictionaryTermError, isDictionarySnapshot, parseDictionaryLines} from '../dictionary';
import {useNavigationGuard} from '../components/NavigationGuard';
import {focusInView} from '../components/focusInView';
import './dictionary.css';

type Editor = {kind: 'single' | 'bulk'; id?: string; text: string; initialText: string; version: number; conflict: boolean};
type DeleteIntent = {entry: DictionaryEntry; version: number; conflict: boolean};

export function DictionaryPage() {
  const {available, dictionary, command, acceptDictionary} = useDesktop();
  const navigation = useNavigationGuard();
  const [query, setQuery] = useState('');
  const [editor, setEditor] = useState<Editor | null>(null);
  const [deleteIntent, setDeleteIntent] = useState<DeleteIntent | null>(null);
  const [loading, setLoading] = useState(available);
  const [busy, setBusy] = useState(false);
  const [readError, setReadError] = useState('');
  const [actionError, setActionError] = useState('');
  const [notice, setNotice] = useState('');
  const [triedSave, setTriedSave] = useState(false);
  const mounted = useRef(false), readRun = useRef(0), actionPending = useRef(false);
  const input = useRef<HTMLInputElement>(null), bulkInput = useRef<HTMLTextAreaElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const listHeading = useRef<HTMLHeadingElement>(null), searchInput = useRef<HTMLInputElement>(null);
  const addButton = useRef<HTMLButtonElement>(null), bulkButton = useRef<HTMLButtonElement>(null);
  const editButtons = useRef(new Map<string, HTMLButtonElement>()), deleteButtons = useRef(new Map<string, HTMLButtonElement>());
  const keepButton = useRef<HTMLButtonElement>(null), editorOrigin = useRef<HTMLButtonElement | null>(null);
  const snapshotRef = useRef(dictionary);
  snapshotRef.current = dictionary;
  const dirty = Boolean(editor && editor.text !== editor.initialText);
  const entries = dictionary?.entries ?? [];
  const usedBytes = dictionaryContextBytes(entries.map(entry => entry.term));
  const filtered = useMemo(() => entries.filter(entry => dictionarySearchKey(entry.term).includes(dictionarySearchKey(query.trim()))), [entries, query]);
  const batch = parseDictionaryLines(editor?.text ?? '');
  const validation = editor?.kind === 'bulk' ? batch.error : dictionaryTermError(editor?.text ?? '');
  const draftCapacity = editor && !validation ? dictionaryDraftCapacity(entries, editor.kind === 'bulk' ? {kind: 'bulk', terms: batch.terms} : {kind: 'single', id: editor.id, text: editor.text}) : null;
  const editorStale = Boolean(editor && dictionary && editor.version !== dictionary.version);

  const read = useCallback(async (): Promise<DictionarySnapshot | null> => {
    if (!available) return null;
    const run = ++readRun.current;
    setLoading(true); setReadError('');
    try {
      const next = await command<DictionarySnapshot>('dictionary.list');
      if (!isDictionarySnapshot(next)) throw new Error('未收到有效的词库，请重新读取。');
      if (mounted.current && run === readRun.current) acceptDictionary(next);
      return next;
    } catch (error) {
      if (mounted.current && run === readRun.current) setReadError(errorMessage(error));
      return null;
    } finally {if (mounted.current && run === readRun.current) setLoading(false);}
  }, [available, command, acceptDictionary]);

  useEffect(() => {
    mounted.current = true;
    void read();
    return () => {mounted.current = false; readRun.current++; navigation.register(null);};
  }, [read, navigation.register]);
  useEffect(() => {
    if (editor) requestAnimationFrame(() => {if (mounted.current) focusInView(editor.kind === 'bulk' ? bulkInput.current : input.current);});
  }, [editor?.kind, editor?.id]);

  const save = useCallback(async () => {
    if (!available || actionPending.current || !editor) return;
    setTriedSave(true);
    const parsed = parseDictionaryLines(editor.text);
    const invalid = editor.kind === 'bulk' ? parsed.error : dictionaryTermError(editor.text);
    if (invalid) {focusInView(editor.kind === 'bulk' ? bulkInput.current : input.current); throw new Error(invalid);}
    actionPending.current = true; setBusy(true); setActionError(''); setNotice('');
    const beforeCount = snapshotRef.current?.entries.length ?? 0;
    try {
      const next = await command<DictionarySnapshot>(editor.kind === 'bulk' ? 'dictionary.import' : 'dictionary.save', {
        ...(editor.kind === 'bulk' ? {terms: parsed.terms} : {draft: {id: editor.id ?? null, term: editor.text.trim()}}),
        expectedVersion: editor.version,
      });
      if (!isDictionarySnapshot(next)) throw new Error('保存结果未能确认，请重新读取词库后核对。');
      if (!mounted.current) return;
      acceptDictionary(next); setEditor(null); setTriedSave(false);
      const added = Math.max(0, next.entries.length - beforeCount);
      setNotice(editor.kind === 'bulk' ? `已添加 ${added} 个词条，跳过 ${parsed.terms.length - added} 个重复词条。下次录音生效。` : next.version === editor.version ? editor.id ? '词条未改变，无需更新。' : '词条已存在，未重复添加。' : editor.id ? '词条已更新，下次录音生效。' : '词条已添加，下次录音生效。');
      requestAnimationFrame(() => {if (mounted.current) focusInView(heading.current);});
    } catch (error) {
      if (dictionaryConflict(error)) {
        const refreshed = await read();
        if (mounted.current) {
          setEditor(current => current ? {...current, version: refreshed?.version ?? current.version, conflict: true} : current);
          setActionError(refreshed ? '词库已变化，已重新读取。草稿仍保留，请核对下方词条后重试。' : '词库已变化，草稿仍保留。请先重新读取，再核对后重试。');
        }
      } else if (mounted.current) setActionError(errorMessage(error));
      throw error;
    } finally {actionPending.current = false; if (mounted.current) setBusy(false);}
  }, [available, editor, command, acceptDictionary, read]);

  useEffect(() => {
    navigation.register({dirty: dirty || busy, pending: busy, save, discard: () => setEditor(null)});
    return () => navigation.register(null);
  }, [navigation.register, dirty, busy, save]);

  function focusAfterRender(resolve: () => HTMLElement | null | undefined) {
    requestAnimationFrame(() => {
      if (!mounted.current) return;
      const target = resolve();
      focusInView(target?.isConnected ? target : listHeading.current);
    });
  }
  function openEditor(kind: Editor['kind'], entry?: DictionaryEntry, trigger?: HTMLButtonElement) {
    if (!dictionary || busy) return;
    navigation.request(() => {
      editorOrigin.current = trigger ?? (entry ? editButtons.current.get(entry.id) : kind === 'bulk' ? bulkButton.current : addButton.current) ?? null;
      setEditor({kind, id: entry?.id, text: entry?.term ?? '', initialText: entry?.term ?? '', version: dictionary.version, conflict: false});
      setDeleteIntent(null); setActionError(''); setNotice(''); setTriedSave(false);
    });
  }
  function closeEditor() {navigation.request(() => {setEditor(null); setActionError(''); setTriedSave(false); focusAfterRender(() => editorOrigin.current);});}
  function closeDelete() {
    const id = deleteIntent?.entry.id;
    setDeleteIntent(null); setActionError('');
    focusAfterRender(() => id ? deleteButtons.current.get(id) : listHeading.current);
  }
  async function reload() {
    const refreshed = await read();
    if (!mounted.current || !refreshed) return;
    setEditor(current => current?.conflict ? {...current, version: refreshed.version} : current);
  }
  async function remove() {
    if (!available || actionPending.current || !deleteIntent) return;
    const deletedIndex = filtered.findIndex(entry => entry.id === deleteIntent.entry.id);
    const adjacentIds = [...filtered.slice(deletedIndex + 1), ...filtered.slice(0, deletedIndex).reverse()].map(entry => entry.id);
    actionPending.current = true; setBusy(true); setActionError(''); setNotice('');
    try {
      const next = await command<DictionarySnapshot>('dictionary.delete', {entryId: deleteIntent.entry.id, expectedVersion: deleteIntent.version});
      if (!isDictionarySnapshot(next)) throw new Error('删除结果未能确认，请重新读取词库后核对。');
      if (!mounted.current) return;
      acceptDictionary(next); setDeleteIntent(null); setNotice('词条已删除，下次录音不再使用。');
      focusAfterRender(() => adjacentIds.map(id => editButtons.current.get(id)).find(Boolean) ?? listHeading.current);
    } catch (error) {
      if (dictionaryConflict(error)) {
        const refreshed = await read();
        if (mounted.current) {
          const latest = refreshed?.entries.find(entry => entry.id === deleteIntent.entry.id);
          setDeleteIntent(latest && refreshed ? {entry: latest, version: refreshed.version, conflict: true} : null);
          setActionError(refreshed ? latest ? '词库已变化，已重新读取。请核对当前词条后再删除。' : '此词条已不在当前词库中。' : '词库已变化，请先重新读取后再删除。');
        }
      } else if (mounted.current) setActionError(errorMessage(error));
    } finally {actionPending.current = false; if (mounted.current) setBusy(false);}
  }

  return <div className="dictionary-page">
    <header className="dictionary-heading"><div className="page-heading"><h1 ref={heading} tabIndex={-1}>个人词典</h1><p>告诉 SpeakMode 常用的名字、术语与专有词。</p></div><div className="dictionary-actions"><Button ref={bulkButton} variant="soft" color="gray" disabled={!available || !dictionary || busy} onClick={event => openEditor('bulk', undefined, event?.currentTarget)}>批量添加</Button><Button ref={addButton} disabled={!available || !dictionary || busy} onClick={event => openEditor('single', undefined, event?.currentTarget)}><PlusIcon/>添加词条</Button></div></header>
    <p className="dictionary-notice" role="status" aria-live="polite">{notice}</p>
    {!available && <p className="dictionary-help" role="status">在桌面应用中维护自己的词库，此浏览器页面不生成示例词条。</p>}
    {readError && <div className="dictionary-error" role="alert"><p>{readError}</p><Button variant="soft" color="gray" disabled={loading || busy} onClick={() => void reload()}>重新读取</Button></div>}
    {actionError && <p className="dictionary-error" role="alert">{actionError}</p>}
    {editor && <section className="dictionary-editor surface" aria-labelledby="dictionary-editor-title" aria-busy={busy}>
      <div className="dictionary-section-heading"><h2 id="dictionary-editor-title">{editor.kind === 'bulk' ? '批量添加词条' : editor.id ? '编辑词条' : '添加词条'}</h2><IconButton variant="ghost" color="gray" disabled={busy} aria-label="关闭词条编辑" onClick={closeEditor}><Cross2Icon/></IconButton></div>
      <form noValidate onSubmit={event => {event.preventDefault(); void save().catch(() => {});}}>
        <label htmlFor="dictionary-term">{editor.kind === 'bulk' ? '每行一个词条' : '词条'}</label>
        {editor.kind === 'bulk' ? <TextArea ref={bulkInput} id="dictionary-term" value={editor.text} disabled={busy} placeholder={'SpeakMode\n人机交互\nPolyU'} rows={5} aria-invalid={triedSave && Boolean(validation)} aria-describedby="dictionary-field-help dictionary-field-error dictionary-capacity-help" onChange={event => {setEditor({...editor, text: event.target.value}); setTriedSave(false); setActionError('');}}/> : <TextField.Root ref={input} id="dictionary-term" value={editor.text} disabled={busy} placeholder="例如：SpeakMode" aria-invalid={triedSave && Boolean(validation)} aria-describedby="dictionary-field-help dictionary-field-error dictionary-capacity-help" onChange={event => {setEditor({...editor, text: event.target.value}); setTriedSave(false); setActionError('');}}/>}
        <div className="dictionary-field-meta"><p id="dictionary-field-help">{editor.kind === 'bulk' ? '空行会忽略，重复词条会自动跳过。整批核对通过后才保存。' : '保留你输入的拼写与大小写，最多 80 个字符。'}</p><span>{editor.kind === 'bulk' ? `${batch.terms.length} / 200 行` : `${Array.from(editor.text.trim()).length} / 80`}</span></div>
        {draftCapacity && <div className={`dictionary-draft-capacity${draftCapacity.remainingBytes < 0 || draftCapacity.entries > dictionaryLimits.entries ? ' is-warning' : ''}`} id="dictionary-capacity-help" role="status" aria-live="polite" aria-atomic="true"><div className="dictionary-capacity-label"><span>草稿容量上限 {draftCapacity.bytes} / {dictionaryLimits.contextBytes} 字节</span><span>{draftCapacity.remainingBytes < 0 ? `可能超出 ${-draftCapacity.remainingBytes} 字节` : `至少剩余 ${draftCapacity.remainingBytes} 字节`}</span></div><p>{editor.kind === 'bulk' ? '已计入现有词库。重复词尚未合并，保存后的实际占用可能更少。' : editor.id ? '已扣除原词条占用；保存时还会核对重复词、容量与版本。' : '已计入现有词库；保存时还会核对重复词、容量与版本。'}{draftCapacity.entries > dictionaryLimits.entries && ` 合并前 ${draftCapacity.entries} 条，最多可保存 ${dictionaryLimits.entries} 条。`}{draftCapacity.remainingBytes < 0 && ' 可缩短词条，或继续保存交由应用核对；超限时不会部分保存。'}</p></div>}
        {triedSave && validation && <p id="dictionary-field-error" className="dictionary-field-error" role="alert">{validation}</p>}
        {editorStale && <p className="dictionary-field-error" role="alert">词库有新版本。保存时将重新核对，不会覆盖其他修改。</p>}
        <div className="dictionary-editor-footer"><span>保存后，从下一次录音开始使用。</span><div className="dictionary-actions"><Button type="button" variant="soft" color="gray" disabled={busy} onClick={closeEditor}>取消</Button><Button type="submit" loading={busy} disabled={!available || !editor.text.trim()}><CheckIcon/>{editor.conflict ? '核对后重试' : editor.kind === 'bulk' ? '添加词条' : '保存词条'}</Button></div></div>
      </form>
    </section>}
    <section className="dictionary-library surface" aria-labelledby="dictionary-list-title" aria-busy={loading || busy}>
      <div className="dictionary-list-toolbar"><div className="dictionary-section-heading"><h2 ref={listHeading} tabIndex={-1} id="dictionary-list-title">我的词库</h2><span>{dictionary ? entries.length : '—'} / {dictionaryLimits.entries}</span></div><div className="dictionary-search"><TextField.Root ref={searchInput} aria-label="搜索词条" value={query} placeholder="搜索词条" onChange={event => setQuery(event.target.value)}><TextField.Slot><MagnifyingGlassIcon/></TextField.Slot>{query && <TextField.Slot side="right"><IconButton size="1" variant="ghost" color="gray" aria-label="清除搜索" onClick={() => {setQuery(''); focusAfterRender(() => searchInput.current);}}><Cross2Icon/></IconButton></TextField.Slot>}</TextField.Root><IconButton variant="ghost" color="gray" aria-label="重新读取词库" disabled={!available || loading || busy} onClick={() => void reload()}><ReloadIcon className={loading ? 'dictionary-spinning' : ''}/></IconButton></div></div>
      {dictionary && <div className="dictionary-capacity"><div className="dictionary-capacity-label"><span>已用 {usedBytes} / {dictionaryLimits.contextBytes} 字节</span><span>剩余 {Math.max(0, dictionaryLimits.contextBytes - usedBytes)} 字节</span></div><meter aria-label="词库容量" min={0} max={dictionaryLimits.contextBytes} value={usedBytes} low={3200} high={3600} optimum={0}/></div>}
      {loading && !dictionary ? <div className="dictionary-empty" role="status"><p>正在读取词库…</p></div> : filtered.length === 0 ? <div className="dictionary-empty" role="status"><span className="dictionary-empty-icon"><ReaderIcon/></span><h3>{query ? '没有找到这个词条' : '让常用词更容易被听见'}</h3><p>{query ? '试试其他拼写，或清除搜索查看全部。' : '添加经常被识别错的人名、产品名或专业术语。'}</p>{!query && <Button variant="soft" disabled={!available || !dictionary || busy} onClick={event => openEditor('single', undefined, event?.currentTarget)}><PlusIcon/>添加第一个词条</Button>}</div> : <ul className="dictionary-list">{filtered.map(entry => <li key={entry.id} className="dictionary-row"><div className="dictionary-word"><ReaderIcon aria-hidden="true"/><span>{entry.term}</span></div><div className="dictionary-actions"><IconButton ref={element => {if (element) editButtons.current.set(entry.id, element); else editButtons.current.delete(entry.id);}} variant="ghost" color="gray" disabled={!available || busy} aria-label={`编辑词条 ${entry.term}`} onClick={event => openEditor('single', entry, event?.currentTarget)}><Pencil2Icon/></IconButton><IconButton ref={element => {if (element) deleteButtons.current.set(entry.id, element); else deleteButtons.current.delete(entry.id);}} variant="ghost" color="gray" disabled={!available || busy} aria-label={`删除词条 ${entry.term}`} onClick={() => {if (!dictionary) return; navigation.request(() => {setEditor(null); setDeleteIntent({entry, version: dictionary.version, conflict: false}); setActionError(''); focusAfterRender(() => keepButton.current);});}}><TrashIcon/></IconButton></div>{deleteIntent?.entry.id === entry.id && <div className="dictionary-delete-confirm" role="alert" onKeyDown={event => {if (event.key === 'Escape' && !busy) {event.preventDefault(); event.stopPropagation(); closeDelete();}}}><p>删除“<strong>{deleteIntent.entry.term}</strong>”？{deleteIntent.conflict && '这是重新读取后的当前词条。'}</p><div className="dictionary-actions"><Button ref={keepButton} variant="soft" color="gray" disabled={busy} onClick={closeDelete}>保留</Button><Button color="red" variant="soft" loading={busy} onClick={() => void remove()}>{deleteIntent.conflict ? '核对后删除' : '删除词条'}</Button></div></div>}</li>)}</ul>}
      <footer className="dictionary-library-footer"><p>词库作为识别提示提交给当前云端服务，有助于专有词识别；效果仍需以实际口述为准。</p><details><summary>使用说明</summary><p>词条手动维护，不会自动从历史记录中学习。批量添加时，相近的全半角及大小写重复词条会合并，保留先添加的拼写。最多 200 条，每条 80 字符；SpeakMode 的词库预算为 4000 UTF-8 字节，包含固定提示、引号与分隔符。中文、英文和符号占用不同，不能按字数换算；所选语音模型还会单独校验请求容量。修改不影响正在进行的录音，也不替换已有识别稿。</p></details></footer>
    </section>
  </div>;
}
