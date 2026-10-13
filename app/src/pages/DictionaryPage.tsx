import {useCallback, useEffect, useMemo, useRef, useState} from 'react';
import {AlertDialog, Button, Dialog, DropdownMenu, IconButton, TextArea, TextField} from '@radix-ui/themes';
import {Cross2Icon, DotsHorizontalIcon, DownloadIcon, MagnifyingGlassIcon, Pencil2Icon, PlusIcon, ReaderIcon, TrashIcon} from '@radix-ui/react-icons';
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
  const [importingTypeless, setImportingTypeless] = useState(false);
  const [readError, setReadError] = useState('');
  const [actionError, setActionError] = useState('');
  const [notice, setNotice] = useState('');
  const [triedSave, setTriedSave] = useState(false);
  const mounted = useRef(false), readRun = useRef(0), actionPending = useRef(false);
  const input = useRef<HTMLInputElement>(null), bulkInput = useRef<HTMLTextAreaElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const listHeading = useRef<HTMLHeadingElement>(null), searchInput = useRef<HTMLInputElement>(null);
  const addButton = useRef<HTMLButtonElement>(null);
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
      snapshotRef.current = next;
      acceptDictionary(next); setEditor(null); setTriedSave(false);
      const added = Math.max(0, next.entries.length - beforeCount);
      setNotice(editor.kind === 'bulk' ? `已添加 ${added} 个词条，跳过 ${parsed.terms.length - added} 个重复词条。下次录音生效。` : next.version === editor.version ? editor.id ? '词条未改变，无需更新。' : '词条已存在，未重复添加。' : editor.id ? '词条已更新，下次录音生效。' : '词条已添加，下次录音生效。');
      requestAnimationFrame(() => {if (mounted.current) focusInView(editorOrigin.current?.isConnected ? editorOrigin.current : heading.current);});
    } catch (error) {
      if (dictionaryConflict(error)) {
        const refreshed = await read();
        if (mounted.current) {
          setEditor(current => current ? {...current, version: refreshed?.version ?? current.version, conflict: true} : current);
          setActionError(refreshed ? '词库已变化，已重新读取。草稿仍保留，请核对后重试。' : '词库已变化，草稿仍保留。请先重新读取，再核对后重试。');
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
      const current = snapshotRef.current;
      if (!current) return;
      const currentEntry = entry ? current.entries.find(item => item.id === entry.id) : undefined;
      if (entry && !currentEntry) {setNotice('此词条已不在当前词库中。'); return;}
      if (!editor) editorOrigin.current = trigger ?? (entry ? editButtons.current.get(entry.id) : addButton.current) ?? null;
      setEditor({kind, id: currentEntry?.id, text: currentEntry?.term ?? '', initialText: currentEntry?.term ?? '', version: current.version, conflict: false});
      setDeleteIntent(null); setActionError(''); setNotice(''); setTriedSave(false);
    });
  }
  function closeEditor() {if (!busy) navigation.request(() => {setEditor(null); setActionError(''); setTriedSave(false); focusAfterRender(() => editorOrigin.current);});}
  function closeDelete() {
    if (busy) return;
    const id = deleteIntent?.entry.id;
    setDeleteIntent(null); setActionError('');
    focusAfterRender(() => id ? deleteButtons.current.get(id) : listHeading.current);
  }
  async function reload() {
    const refreshed = await read();
    if (!mounted.current || !refreshed) return;
    setEditor(current => current?.conflict ? {...current, version: refreshed.version} : current);
  }
  async function importTypeless() {
    const before = snapshotRef.current;
    if (!available || actionPending.current || !before || editor || deleteIntent) return;
    actionPending.current = true;
    setBusy(true); setImportingTypeless(true); setActionError(''); setNotice('');
    try {
      const next = await command<DictionarySnapshot>('dictionary.importTypeless', {expectedVersion: before.version});
      if (!isDictionarySnapshot(next)) throw new Error('导入结果未能确认，请重新读取词库后核对。');
      if (!mounted.current) return;
      snapshotRef.current = next;
      acceptDictionary(next);
      const added = Math.max(0, next.entries.length - before.entries.length);
      setNotice(added ? `已从 Typeless 导入 ${added} 个新词，重复词已跳过。下次录音生效。` : 'Typeless 中的词汇已在词典里，无需重复导入。');
    } catch (error) {
      if (dictionaryConflict(error)) {
        const refreshed = await read();
        if (mounted.current) setActionError(refreshed ? '词库已更新，请再次点击导入。已有词汇会自动跳过。' : '词库已变化，请重新读取后再导入。');
      } else if (mounted.current) setActionError(errorMessage(error));
    } finally {
      actionPending.current = false;
      if (mounted.current) {setBusy(false); setImportingTypeless(false);}
    }
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
          if (!latest) focusAfterRender(() => adjacentIds.map(id => editButtons.current.get(id)).find(Boolean) ?? listHeading.current);
          setActionError(refreshed ? latest ? '词库已变化，已重新读取。请核对当前词条后再删除。' : '此词条已不在当前词库中。' : '词库已变化，请先重新读取后再删除。');
        }
      } else if (mounted.current) setActionError(errorMessage(error));
    } finally {actionPending.current = false; if (mounted.current) setBusy(false);}
  }

  const capacityWarning = Boolean(draftCapacity && (draftCapacity.remainingBytes < 0 || draftCapacity.entries > dictionaryLimits.entries));
  const capacityNotice = dictionary && (usedBytes >= 3200 || entries.length >= 180);
  const editorDescription = editor?.kind === 'bulk' ? '每行一个词，重复词会自动跳过。' : editor?.id ? '保留你希望识别出的拼写。' : '添加常用的人名、产品名或专业术语。';
  const readFailure = readError && <div className="dictionary-error" role="alert"><p>{readError}</p><Button variant="soft" color="gray" disabled={loading || busy} onClick={() => void reload()}>重新读取</Button></div>;

  return <div className="dictionary-page">
    <header className="dictionary-heading">
      <div className="page-heading"><h1 ref={heading} tabIndex={-1}>个人词典</h1><p>记住常用词，让表达更准确。</p></div>
      <div className="dictionary-heading-actions">
        <Button className="dictionary-import" variant="soft" color="gray" loading={importingTypeless} aria-label={importingTypeless ? '正在导入 Typeless 词汇' : '导入 Typeless'} aria-busy={importingTypeless} title="读取本机缓存的 Typeless 词汇；如刚修改过词典，请先在 Typeless 中打开词典页。" disabled={!available || !dictionary || busy || Boolean(editor || deleteIntent)} onClick={() => void importTypeless()}><DownloadIcon/>{importingTypeless ? '正在导入…' : '导入 Typeless'}</Button>
        <Button ref={addButton} className="dictionary-add" disabled={!available || !dictionary || busy} onClick={event => openEditor('single', undefined, event?.currentTarget)}><PlusIcon/>新词</Button>
      </div>
    </header>
    <p className="dictionary-notice" role="status" aria-live="polite">{notice}</p>
    {!available && <p className="dictionary-help" role="status">在桌面应用中添加和管理词汇。</p>}
    {!editor && !deleteIntent && readFailure}
    {!editor && !deleteIntent && actionError && <p className="dictionary-error" role="alert">{actionError}</p>}

    <section className="dictionary-library" aria-labelledby="dictionary-list-title" aria-busy={loading || busy}>
      <div className="dictionary-list-toolbar">
        <h2 ref={listHeading} tabIndex={-1} id="dictionary-list-title">{query ? '搜索结果' : '全部词条'}<span>{dictionary ? query ? filtered.length : entries.length : '—'}</span></h2>
        <TextField.Root className="dictionary-search" ref={searchInput} aria-label="搜索词条" value={query} placeholder="搜索词条" onChange={event => setQuery(event.target.value)}>
          <TextField.Slot><MagnifyingGlassIcon/></TextField.Slot>
          {query && <TextField.Slot side="right"><IconButton size="1" variant="ghost" color="gray" aria-label="清除搜索" onClick={() => {setQuery(''); focusAfterRender(() => searchInput.current);}}><Cross2Icon/></IconButton></TextField.Slot>}
        </TextField.Root>
      </div>
      {loading && !dictionary ? <div className="dictionary-empty" role="status"><p>正在读取词库…</p></div>
        : !dictionary ? <div className="dictionary-empty" role="status"><ReaderIcon/><h3>{available ? '暂时无法显示词库' : '词汇会保存在桌面应用中'}</h3><p>{available ? '重新读取后，你的词条会出现在这里。' : '打开 SpeakMode，即可查看和添加常用词。'}</p></div>
        : filtered.length === 0 ? <div className="dictionary-empty" role="status"><span className="dictionary-empty-icon"><ReaderIcon/></span><h3>{query ? '没有找到这个词条' : '从一个常用词开始'}</h3><p>{query ? '换个关键词试试，或清除搜索。' : '人名、产品名、专业术语，都可以放在这里。'}</p>{query ? <Button variant="soft" color="gray" onClick={() => {setQuery(''); focusAfterRender(() => searchInput.current);}}>清除搜索</Button> : <Button variant="soft" disabled={!available || busy} onClick={event => openEditor('single', undefined, event?.currentTarget)}><PlusIcon/>添加第一个词条</Button>}</div>
        : <ul className="dictionary-list">{filtered.map(entry => <li key={entry.id} className="dictionary-row">
          <button className="dictionary-word" ref={element => {if (element) editButtons.current.set(entry.id, element); else editButtons.current.delete(entry.id);}} disabled={!available || busy} aria-label={'编辑词条 ' + entry.term} title={entry.term} onClick={event => openEditor('single', entry, event?.currentTarget)}>
            <span className="dictionary-word-mark" aria-hidden="true"><ReaderIcon/></span><span className="dictionary-word-text">{entry.term}</span>
          </button>
          <DropdownMenu.Root>
            <DropdownMenu.Trigger><IconButton className="dictionary-word-menu" ref={element => {if (element) deleteButtons.current.set(entry.id, element); else deleteButtons.current.delete(entry.id);}} variant="ghost" color="gray" disabled={!available || busy} aria-label={'词条操作 ' + entry.term}><DotsHorizontalIcon/></IconButton></DropdownMenu.Trigger>
            <DropdownMenu.Content align="end" size="2">
              <DropdownMenu.Item onSelect={() => openEditor('single', entry, deleteButtons.current.get(entry.id))}><Pencil2Icon/>编辑词条</DropdownMenu.Item>
              <DropdownMenu.Separator/>
              <DropdownMenu.Item color="red" onSelect={() => {if (!dictionary || busy) return; setDeleteIntent({entry, version: dictionary.version, conflict: false}); setActionError('');}}><TrashIcon/>删除词条</DropdownMenu.Item>
            </DropdownMenu.Content>
          </DropdownMenu.Root>
        </li>)}</ul>}
      {dictionary && <footer className="dictionary-library-footer">
        <p>{capacityNotice ? '词库接近上限，可以整理不常用的词条。' : '修改会在下一次录音时生效。'}</p>
        <details className="dictionary-library-details"><summary>词库说明</summary><div className="dictionary-library-notes">
          <p>词条作为提示提交给当前语音识别服务，不会强制替换结果，也不会自动从历史中添加。</p>
          <p>从 Typeless 导入会读取本机最近缓存的词汇，原词库保持不变。如刚修改过 Typeless 词典，请先打开它的词典页更新，再回来导入。</p>
          <div className="dictionary-capacity"><div className="dictionary-capacity-label"><span>已用 {usedBytes} / {dictionaryLimits.contextBytes} 字节</span><span>剩余 {Math.max(0, dictionaryLimits.contextBytes - usedBytes)} 字节</span></div><meter aria-label="词库容量" min={0} max={dictionaryLimits.contextBytes} value={usedBytes}/></div>
          <p>{entries.length} / {dictionaryLimits.entries} 个词条，每条最多 80 字符。容量包含词汇和必要的提示格式；修改不会影响正在进行的录音。</p>
        </div></details>
      </footer>}
    </section>

    <Dialog.Root open={Boolean(editor)} onOpenChange={open => {if (!open) closeEditor();}}>
      <Dialog.Content className="dictionary-editor" maxWidth="520px" aria-busy={busy} onOpenAutoFocus={event => {event.preventDefault(); focusAfterRender(() => editor?.kind === 'bulk' ? bulkInput.current : input.current);}} onCloseAutoFocus={event => event.preventDefault()} onInteractOutside={event => {if (busy) event.preventDefault();}} onEscapeKeyDown={event => {if (busy) event.preventDefault();}}>
        {editor && <>
          <Dialog.Title>{editor.kind === 'bulk' ? '批量添加' : editor.id ? '编辑词条' : '添加到词典'}</Dialog.Title>
          <Dialog.Description className="dictionary-dialog-description">{editorDescription}</Dialog.Description>
          <form noValidate onSubmit={event => {event.preventDefault(); void save().catch(() => {});}}>
            <label className="dictionary-field-label" htmlFor="dictionary-term">{editor.kind === 'bulk' ? '每行一个词条' : '词条'}</label>
            {editor.kind === 'bulk' ? <TextArea ref={bulkInput} id="dictionary-term" value={editor.text} disabled={busy} placeholder={'SpeakMode\n人机交互\nPolyU'} rows={5} aria-invalid={triedSave && Boolean(validation)} aria-describedby={'dictionary-field-help' + (triedSave && validation ? ' dictionary-field-error' : '')} onChange={event => {setEditor({...editor, text: event.target.value}); setTriedSave(false); setActionError('');}}/> : <TextField.Root ref={input} id="dictionary-term" value={editor.text} disabled={busy} placeholder="输入新词，例如 SpeakMode" aria-invalid={triedSave && Boolean(validation)} aria-describedby={'dictionary-field-help' + (triedSave && validation ? ' dictionary-field-error' : '')} onChange={event => {setEditor({...editor, text: event.target.value}); setTriedSave(false); setActionError('');}}/>}
            <div className="dictionary-field-meta"><p id="dictionary-field-help">{editor.kind === 'bulk' ? '空行忽略，整批保存。' : '保留原有拼写与大小写。'}</p><span>{editor.kind === 'bulk' ? batch.terms.length + ' / 200 行' : Array.from(editor.text.trim()).length + ' / 80'}</span></div>
            {triedSave && validation && <p id="dictionary-field-error" className="dictionary-field-error" role="alert">{validation}</p>}
            {editorStale && <p className="dictionary-field-error" role="alert">词库已更新，保存时会核对最新版本。</p>}
            {editor.conflict && editor.id && <p className="dictionary-current-value">{dictionary?.entries.find(entry => entry.id === editor.id) ? '当前已保存：' + dictionary.entries.find(entry => entry.id === editor.id)!.term : '此词条已被删除。草稿仍保留，可复制后重新添加。'}</p>}
            {readFailure}
            {actionError && <p className="dictionary-error" role="alert">{actionError}</p>}
            {draftCapacity && <details className={'dictionary-capacity-details' + (capacityWarning ? ' is-warning' : '')} open={capacityWarning || undefined}>
              <summary>{capacityWarning ? '容量可能不足' : '容量详情'}</summary>
              <div className="dictionary-draft-capacity" id="dictionary-capacity-help">
                <div className="dictionary-capacity-label"><span>草稿容量上限 {draftCapacity.bytes} / {dictionaryLimits.contextBytes} 字节</span><span>{draftCapacity.remainingBytes < 0 ? '可能超出 ' + -draftCapacity.remainingBytes + ' 字节' : '至少剩余 ' + draftCapacity.remainingBytes + ' 字节'}</span></div>
                <p>{editor.kind === 'bulk' ? '已计入现有词库。重复词尚未合并，实际占用可能更少。' : '已计入现有词库，编辑时会扣除原词条。'}{draftCapacity.entries > dictionaryLimits.entries && ' 合并前 ' + draftCapacity.entries + ' 条，最多可保存 ' + dictionaryLimits.entries + ' 条。'}{capacityWarning && ' 可以缩短词条，或保存交由应用核对；超限时不会部分保存。'}</p>
              </div>
            </details>}
            <div className="dictionary-editor-footer">
              {!editor.id && <Button type="button" className="dictionary-bulk-link" variant="ghost" color="gray" disabled={busy} onClick={() => openEditor(editor.kind === 'bulk' ? 'single' : 'bulk')}>{editor.kind === 'bulk' ? '单个添加' : '批量添加'}</Button>}
              <div className="dictionary-actions"><Button type="button" variant="soft" color="gray" disabled={busy} onClick={closeEditor}>取消</Button><Button type="submit" loading={busy} disabled={!available || !editor.text.trim()}>{editor.conflict ? '核对后重试' : editor.kind === 'bulk' ? '添加词条' : editor.id ? '保存词条' : '添加词汇'}</Button></div>
            </div>
          </form>
        </>}
      </Dialog.Content>
    </Dialog.Root>

    <AlertDialog.Root open={Boolean(deleteIntent)} onOpenChange={open => {if (!open) closeDelete();}}>
      <AlertDialog.Content className="dictionary-delete-dialog" maxWidth="420px" onOpenAutoFocus={event => {event.preventDefault(); focusAfterRender(() => keepButton.current);}} onCloseAutoFocus={event => event.preventDefault()} onEscapeKeyDown={event => {if (busy) event.preventDefault();}}>
        <AlertDialog.Title>删除这个词条？</AlertDialog.Title>
        <AlertDialog.Description className="dictionary-delete-description">“{deleteIntent?.entry.term}”将不再用于下一次识别，已有口述不受影响。{deleteIntent?.conflict && '词库已更新，请核对当前词条。'}</AlertDialog.Description>
        {readFailure}
        {actionError && <p className="dictionary-error" role="alert">{actionError}</p>}
        <div className="dictionary-actions"><Button ref={keepButton} variant="soft" color="gray" disabled={busy} onClick={closeDelete}>保留</Button><Button color="red" variant="soft" loading={busy} onClick={() => void remove()}>{deleteIntent?.conflict ? '核对后删除' : '删除词条'}</Button></div>
      </AlertDialog.Content>
    </AlertDialog.Root>
  </div>;
}
