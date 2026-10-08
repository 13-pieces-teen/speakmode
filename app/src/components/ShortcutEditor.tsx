import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react';
import { Button, Kbd } from '@radix-ui/themes';
import { CheckIcon, KeyboardIcon, Pencil1Icon, ResetIcon } from '@radix-ui/react-icons';
import { useDesktop } from '../services/DesktopProvider';
import { errorMessage } from '../services/bridge';

const DEFAULT_SHORTCUT = '右 Alt';
const FUNCTION_KEYS = new Set(['F2', 'F3', 'F6', 'F7', 'F8', 'F9', 'F10', 'F11', 'F12']);
const RESERVED_CTRL_LETTERS = new Set(['C', 'V', 'X', 'Z', 'A', 'F', 'R', 'W', 'T', 'N', 'L']);

type ShortcutResult = { value: string; error?: never } | { value?: never; error: string };

type CaptureCommand = ReturnType<typeof useDesktop>['command'];
interface CaptureLease {
  id: string;
  begin: Promise<unknown>;
  release: Promise<void> | null;
  released: boolean;
  abandoned: boolean;
  leaving: boolean;
  cleanup: Promise<void> | null;
}

// Native capture is global. Keep cleanup alive across page unmounts, and make a newly
// mounted editor wait so an old cancellation cannot release its newer capture.
let detachedCaptureCleanup: Promise<void> | null = null;

function releaseCapture(command: CaptureCommand, lease: CaptureLease): Promise<void> {
  if (lease.released) return Promise.resolve();
  if (lease.release) return lease.release;
  lease.release = (async () => {
    // Even a failed begin may have reached native before its response was lost.
    await lease.begin.catch(() => undefined);
    await command('shortcut.capture.cancel', { captureId: lease.id });
    lease.released = true;
  })().finally(() => { lease.release = null; });
  return lease.release;
}

function abandonCapture(command: CaptureCommand, lease: CaptureLease) {
  lease.abandoned = true;
  if (lease.cleanup || lease.released) return;
  const previous = detachedCaptureCleanup;
  const cleanup = (async () => {
    if (previous) await previous;
    let attempt = 0;
    while (!lease.released) {
      try { await releaseCapture(command, lease); }
      catch {
        // There is no mounted panel in which to retry. Retain ownership and retry
        // until native confirms release; subsequent editors wait on this promise.
        await new Promise(resolve => setTimeout(resolve, Math.min(100 * 2 ** Math.min(attempt++, 5), 2000)));
      }
    }
  })();
  lease.cleanup = cleanup;
  detachedCaptureCleanup = cleanup;
  void cleanup.then(() => {
    if (detachedCaptureCleanup === cleanup) detachedCaptureCleanup = null;
  });
}

function readShortcut(event: KeyboardEvent<HTMLButtonElement>): ShortcutResult {
  const { key, code, ctrlKey, altKey, shiftKey, metaKey } = event;
  if (metaKey || key === 'Meta' || code === 'MetaLeft' || code === 'MetaRight') {
    return { error: '不能使用 Win 键，请选择其他快捷键。' };
  }
  if ((ctrlKey && altKey) || event.getModifierState('AltGraph') || key === 'AltGraph') {
    return { error: '暂不支持 Ctrl + Alt 组合，以免与 AltGr 输入冲突。' };
  }
  if (event.nativeEvent.isComposing || key === 'Process' || key === 'Dead' || key === 'Unidentified') {
    return { error: '未能识别这个按键，请结束输入法组合后重新按键。' };
  }
  if (altKey && (key === 'F4' || code === 'F4' || key === 'Tab' || code === 'Tab')) {
    return { error: 'Alt + F4 和 Alt + Tab 是系统快捷键，不能用于录音。' };
  }
  if (code === 'AltRight' && !ctrlKey && !shiftKey) {
    return { value: DEFAULT_SHORTCUT };
  }
  if (!ctrlKey && !altKey && !shiftKey && FUNCTION_KEYS.has(code || key)) {
    return { value: code || key };
  }
  if (key === 'Control' || key === 'Shift' || key === 'Alt') {
    return { error: '除右 Alt 外，修饰键不能单独使用；请按 Ctrl 或 Alt 加字母或空格，也可加 Shift。' };
  }

  // Store a stable physical letter key even when Shift or the keyboard layout changes its text.
  const letter = /^Key[A-Z]$/.test(code) ? code.slice(3) : /^[a-z]$/i.test(key) ? key.toUpperCase() : null;
  const isSpace = code === 'Space' || key === ' ';
  if (!letter && !isSpace) {
    return { error: '请使用右 Alt、F2/F3/F6–F12，或 Ctrl / Alt + 字母或空格，也可加 Shift。' };
  }
  if (!ctrlKey && !altKey) {
    return { error: shiftKey
      ? 'Shift 加字母或空格会影响正常输入，请同时按 Ctrl 或 Alt。'
      : '字母和空格不能单独使用，请同时按 Ctrl 或 Alt，也可加 Shift。' };
  }
  // Also reject Shift variants of common Ctrl shortcuts (for example Ctrl + Shift + T).
  if (ctrlKey && letter && RESERVED_CTRL_LETTERS.has(letter)) {
    return { error: `Ctrl + ${letter} 是常用快捷键，请换一组按键（加 Shift 也不采用）。` };
  }
  const modifiers = [ctrlKey && 'Ctrl', altKey && 'Alt', shiftKey && 'Shift'].filter(Boolean);
  return { value: [...modifiers, letter ?? 'Space'].join(' + ') };
}

export interface ShortcutEditorProps {
  value: string;
  onChange: (value: string, captureId?: string) => Promise<void>;
  disabled?: boolean;
}

/** Captures a candidate while native recording toggles are suspended. */
export function ShortcutEditor({ value, onChange, disabled = false }: ShortcutEditorProps) {
  const desktop = useDesktop();
  const command = desktop.command;
  const [pending, setPending] = useState(false);
  const operation = useRef(false);
  const mounted = useRef(true);
  const captureLease = useRef<CaptureLease | null>(null);
  const unavailable = disabled || !desktop.available;
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (captureLease.current) abandonCapture(command, captureLease.current);
    };
  }, [command]);
  const [editing, setEditing] = useState(false);
  const [captureReady, setCaptureReady] = useState(false);
  const [committed, setCommitted] = useState(false);
  const [candidate, setCandidate] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const changeRef = useRef<HTMLButtonElement>(null);
  const captureRef = useRef<HTMLButtonElement>(null);
  const returnFocus = useRef(false);
  const panelId = useId();
  const helpId = useId();
  const boundaryId = useId();
  const errorId = useId();

  useEffect(() => {
    function leaveWindow(event?: Event) {
      // An element losing focus to Confirm/Cancel must not release capture.
      if (event?.type === 'blur' && event.target !== window) return;
      const lease = captureLease.current;
      if (!lease || lease.leaving) return;
      lease.leaving = true;
      returnFocus.current = false;
      setCaptureReady(false);
      setEditing(true);
      setPending(true);
      setError('');
      setNotice('');
      // releaseCapture waits for this lease's begin, even if blur arrived while
      // begin was pending. Keep a failed release visible for an explicit retry.
      void closeEditor('已退出按键捕获。', lease, false).catch(() => {
        if (mounted.current && captureLease.current === lease) {
          setError('未能确认已退出按键捕获，录音快捷键暂时停用。请重试退出。');
        }
      }).finally(() => {
        if (mounted.current && (!captureLease.current || captureLease.current === lease)) {
          setPending(operation.current);
        }
      });
    }
    const visibilityChanged = () => {
      if (document.visibilityState === 'hidden') leaveWindow();
    };
    window.addEventListener('blur', leaveWindow);
    document.addEventListener('visibilitychange', visibilityChanged);
    return () => {
      window.removeEventListener('blur', leaveWindow);
      document.removeEventListener('visibilitychange', visibilityChanged);
    };
  }, [command]);

  useEffect(() => {
    if (document.visibilityState === 'hidden' || !document.hasFocus()) return;
    if (editing && !pending) {
      captureRef.current?.focus();
    } else if (!editing && !pending && !unavailable && returnFocus.current) {
      returnFocus.current = false;
      changeRef.current?.focus();
    }
  }, [editing, pending, unavailable]);

  async function startEditing() {
    if (operation.current || unavailable) return;
    operation.current = true;
    setPending(true);
    setError('');
    setNotice('');
    let lease: CaptureLease | null = null;
    try {
      while (detachedCaptureCleanup) await detachedCaptureCleanup;
      if (!mounted.current || document.visibilityState === 'hidden' || !document.hasFocus()) return;
      lease = { id: crypto.randomUUID(), begin: Promise.resolve(), release: null, released: false, abandoned: false, leaving: false, cleanup: null };
      captureLease.current = lease;
      lease.begin = command('shortcut.capture.begin', { captureId: lease.id });
      await lease.begin;
      if (!mounted.current || lease.abandoned) { abandonCapture(command, lease); return; }
      if (lease.leaving || captureLease.current !== lease) return;
      setCandidate(null);
      setCommitted(false);
      setCaptureReady(true);
      setEditing(true);
    } catch (e) {
      if (!mounted.current || lease?.abandoned) {
        if (lease) abandonCapture(command, lease);
        return;
      }
      if (lease?.leaving || captureLease.current !== lease) return;
      try {
        if (lease) await releaseCapture(command, lease);
        if (captureLease.current !== lease) return;
        captureLease.current = null;
        if (mounted.current) setError(errorMessage(e));
      } catch {
        if (mounted.current) {
          setEditing(true);
          setCaptureReady(false);
          setError('未能确认已退出按键捕获。请重试取消后，再更改快捷键。');
        }
      }
    } finally {
      operation.current = false;
      if (mounted.current) setPending(Boolean(captureLease.current?.release));
    }
  }

  async function closeEditor(message: string, lease = captureLease.current, restoreFocus = true) {
    if (lease) await releaseCapture(command, lease);
    // An old release ACK must never clear a newer capture or its UI.
    if (captureLease.current !== lease) return;
    captureLease.current = null;
    if (!mounted.current) return;
    returnFocus.current = restoreFocus && !lease?.leaving;
    setEditing(false);
    setCaptureReady(false);
    setCandidate(null);
    setCommitted(false);
    setError('');
    setNotice(message);
  }

  async function cancelEditing() {
    if (operation.current) return;
    operation.current = true;
    setPending(true);
    setError('');
    try {
      await closeEditor(committed ? '已退出按键捕获，录音快捷键已更新。' : '已取消更改，当前快捷键保持不变。');
    } catch {
      if (mounted.current) setError('退出按键捕获未成功，录音快捷键暂时停用。请重试取消。');
    } finally {
      operation.current = false;
      if (mounted.current) setPending(Boolean(captureLease.current?.release));
    }
  }

  function capture(event: KeyboardEvent<HTMLButtonElement>) {
    // Keep Tab / Shift + Tab available to reach the confirmation and cancel controls.
    if (event.key === 'Tab' && !event.ctrlKey && !event.altKey && !event.metaKey) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.repeat || operation.current) return;
    if (event.key === 'Escape') {
      void cancelEditing();
      return;
    }
    if (!captureReady || committed || captureLease.current?.leaving) return;
    const result = readShortcut(event);
    if (result.error !== undefined) {
      setCandidate(null);
      setError(result.error);
      setNotice('');
      return;
    }
    setCandidate(result.value);
    setError('');
    setNotice(`候选快捷键：${result.value}。确认后才会更新设置。`);
  }

  async function applyCandidate() {
    const lease = captureLease.current;
    if (!candidate || !lease || lease.leaving || operation.current || !captureReady || committed || unavailable) return;
    operation.current = true;
    setPending(true); setError('');
    let saved = false;
    try {
      await onChange(candidate, lease.id);
      saved = true;
      if (mounted.current && captureLease.current === lease) {
        setCommitted(true); setNotice('快捷键已保存，正在退出按键捕获…');
      } else if (mounted.current && !captureLease.current && lease.leaving && lease.released) {
        setNotice('录音快捷键已更新，已退出按键捕获。');
      }
      await closeEditor('录音快捷键已更新。', lease);
    } catch (e) {
      if (mounted.current && (!captureLease.current || captureLease.current === lease)) {
        setNotice('');
        setError(saved ? '快捷键已保存，但退出按键捕获未成功。请重试退出，恢复录音快捷键。' : errorMessage(e));
      }
    } finally {
      operation.current = false;
      if (mounted.current) setPending(Boolean(captureLease.current?.release));
    }
  }
  async function restoreDefault() {
    if (operation.current || unavailable || captureLease.current) return;
    operation.current = true;
    setPending(true); setError(''); setNotice('');
    let lease: CaptureLease | null = null;
    let saved = false;
    let failure = '';
    try {
      while (detachedCaptureCleanup) await detachedCaptureCleanup;
      if (!mounted.current || document.visibilityState === 'hidden' || !document.hasFocus()) return;
      lease = { id: crypto.randomUUID(), begin: Promise.resolve(), release: null, released: false, abandoned: false, leaving: false, cleanup: null };
      captureLease.current = lease;
      setCaptureReady(false); setCommitted(false); setCandidate(null);
      lease.begin = command('shortcut.capture.begin', { captureId: lease.id });
      await lease.begin;
      if (!mounted.current || lease.abandoned || lease.leaving || captureLease.current !== lease) return;
      await onChange(DEFAULT_SHORTCUT, lease.id);
      saved = true;
      if (mounted.current && captureLease.current === lease) setCommitted(true);
    } catch (e) {
      failure = errorMessage(e);
    } finally {
      // Restoring is a short capture transaction, not a preferences reset. Share
      // its release with blur/unmount cleanup and await the native cancel ACK.
      if (lease) {
        try {
          await closeEditor('', lease);
          if (mounted.current && !captureLease.current) {
            if (saved) setNotice('录音快捷键已恢复为右 Alt。');
            else if (failure) setError(failure);
            else if (lease.leaving) setNotice('已取消恢复，当前快捷键保持不变。');
          }
        } catch {
          if (mounted.current && captureLease.current === lease) {
            setEditing(true);
            setCaptureReady(false);
            setCommitted(saved);
            setNotice('');
            setError(saved
              ? '录音快捷键已恢复为右 Alt，但退出按键捕获未成功。请重试退出，恢复录音快捷键。'
              : `${failure ? `${failure}；` : ''}未能确认已退出按键捕获，录音快捷键暂时停用。请重试退出。`);
          }
        }
      } else if (mounted.current && failure) {
        setError(failure);
      }
      operation.current = false;
      if (mounted.current) setPending(Boolean(captureLease.current?.release));
    }
  }
  return <div className="shortcut-editor">
    <div className="shortcut-current">
      <Button ref={changeRef} type="button" size="2" variant="surface" color="gray"
        className="shortcut-change" aria-expanded={editing} aria-controls={panelId}
        aria-label={`更改录音快捷键，当前为 ${value}`}
        disabled={editing || pending || unavailable} onClick={() => void startEditing()}>
        <Kbd className="shortcut-value" aria-label={`当前快捷键：${value}`}>{value}</Kbd>
        <Pencil1Icon aria-hidden="true" />
      </Button>
      {!editing && value !== DEFAULT_SHORTCUT && <Button type="button" size="2"
        variant="ghost" color="gray" className="shortcut-reset" disabled={pending || unavailable} onClick={() => void restoreDefault()}>
        <ResetIcon aria-hidden="true" />恢复右 Alt
      </Button>}
    </div>
    {editing && <section className="shortcut-panel" id={panelId} aria-label="更改录音快捷键">
      <p className="shortcut-help" id={helpId}>
        按下右 Alt、F2/F3/F6–F12，或 Ctrl / Alt + 字母或空格，也可加 Shift。
        在捕获区域按 Esc 取消，按 Tab 前往确认或取消按钮。
      </p>
      <Button ref={captureRef} type="button" size="2" variant="outline"
        className="shortcut-capture" aria-label="捕获新快捷键"
        aria-describedby={`${helpId} ${boundaryId}${error ? ` ${errorId}` : ''}`}
        disabled={pending || !captureReady || committed} onKeyDown={capture}>
        <KeyboardIcon aria-hidden="true" />
        {candidate ? <Kbd className="shortcut-candidate">{candidate}</Kbd> : <span>在此按下快捷键</span>}
      </Button>
      <p className="shortcut-boundary" id={boundaryId}>确认后由桌面服务注册；失败时保留当前快捷键。</p>
      {error && <p className="shortcut-error" id={errorId} role="alert">{error}</p>}
      <div className="shortcut-actions">
        <Button type="button" size="2" variant="soft" color="gray" className="shortcut-cancel"
          disabled={pending} onClick={() => void cancelEditing()}>{committed || !captureReady ? '重试退出' : '取消'}</Button>
        <Button type="button" size="2" className="shortcut-apply" disabled={!candidate || pending || unavailable || !captureReady || committed}
          onClick={() => void applyCandidate()}><CheckIcon aria-hidden="true" />使用此快捷键</Button>
      </div>
    </section>}
    {!editing && error && <p className="shortcut-error" role="alert">{error}</p>}
    <p className="shortcut-status" role="status" aria-live="polite" aria-atomic="true">{notice}</p>
  </div>;
}



