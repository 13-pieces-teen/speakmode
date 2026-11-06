import { useEffect, useId, useRef, useState } from 'react';
import { ArrowRightIcon, ChevronDownIcon } from '@radix-ui/react-icons';
import { DropdownMenu } from '@radix-ui/themes';
import type { Mode } from '../model';
import { scenarioOptions } from '../modes';
import { useModes } from '../hooks/useModes';
import { errorMessage } from '../services/bridge';
import { ModeIcon } from './ui';
import './HomeModePicker.css';

interface HomeModePickerProps {
  value: Mode;
  onChange: (mode: Mode) => Promise<void>;
  onManage: () => void;
  onPersonalization?: () => void;
}

interface SaveError {
  mode: Mode;
  name: string;
  message: string;
}

/** A compact entry point; only the native preference refresh changes selection. */
export function HomeModePicker({ value, onChange, onManage, onPersonalization }: HomeModePickerProps) {
  const catalog = useModes();
  const options = scenarioOptions(value, catalog.modes);
  const currentName = catalog.label(value);
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState<Mode | null>(null);
  const [error, setError] = useState<SaveError | null>(null);
  const [status, setStatus] = useState('');
  const pendingRef = useRef(false);
  const mounted = useRef(false);
  const request = useRef(0);
  const statusId = useId();
  const busy = pending !== null;

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; request.current++; };
  }, []);

  async function choose(next: Mode) {
    if (!catalog.sessionAvailable || pendingRef.current) return;
    if (next === value) { setOpen(false); setError(null); return; }
    const name = catalog.label(next);
    const run = ++request.current;
    pendingRef.current = true;
    setPending(next);
    setError(null);
    setStatus(`正在保存“${name}”…`);
    try {
      await onChange(next);
      if (!mounted.current || run !== request.current) return;
      setStatus(`已设为“${name}”，用于下一次口述。`);
      setOpen(false);
    } catch (cause) {
      if (!mounted.current || run !== request.current) return;
      setStatus('');
      setError({ mode: next, name, message: errorMessage(cause) });
    } finally {
      if (mounted.current && run === request.current) {
        pendingRef.current = false;
        setPending(null);
      }
    }
  }

  return <div className="home-mode-picker" data-pending={busy || undefined}>
    <DropdownMenu.Root open={open} onOpenChange={next => {
      // Escape/outside click remain available while saving, but cannot start a second operation.
      if (!next || !pendingRef.current) setOpen(next);
    }}>
      <DropdownMenu.Trigger>
        <button type="button" className="home-mode-trigger" disabled={!catalog.sessionAvailable}
          aria-disabled={!catalog.sessionAvailable || busy} aria-describedby={statusId}
          aria-label={`场合：${currentName}，更改用于下一次口述`}>
          <span className="home-mode-trigger-label">场合</span>
          <span className="home-mode-trigger-value"><ModeIcon mode={value} /><strong>{currentName}</strong></span>
          <ChevronDownIcon className="home-mode-chevron" aria-hidden="true" />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Content className="home-mode-menu" align="start" size="2" variant="soft"
        aria-label="选择下次口述场合" aria-busy={busy}>
        <DropdownMenu.RadioGroup value={value} onValueChange={next => void choose(next as Mode)} aria-label="场合">
          {options.map(option => <DropdownMenu.RadioItem className="home-mode-option" key={option.id}
            value={option.id} textValue={option.name} disabled={!catalog.sessionAvailable || busy}
            onSelect={event => event.preventDefault()}>
            <span className="home-mode-option-icon"><ModeIcon mode={option.id} /></span>
            <span className="home-mode-option-copy"><strong>{option.name}</strong><span>{option.caption}</span></span>
          </DropdownMenu.RadioItem>)}
        </DropdownMenu.RadioGroup>
        <DropdownMenu.Separator className="home-mode-separator" />
        {busy && <DropdownMenu.Label className="home-mode-note">正在保存场合…</DropdownMenu.Label>}
        {error && <div className="home-mode-error" role="alert"><span>未能切换到“{error.name}”：{error.message}</span>
          <DropdownMenu.Item className="home-mode-retry" disabled={busy || !catalog.sessionAvailable}
            onSelect={event => { event.preventDefault(); void choose(error.mode); }}>重新尝试</DropdownMenu.Item>
        </div>}
        <DropdownMenu.Label className="home-mode-note">用于下一次口述</DropdownMenu.Label>
        <DropdownMenu.Item className="home-mode-manage" disabled={busy} onSelect={onManage}>
          管理场合<ArrowRightIcon aria-hidden="true" />
        </DropdownMenu.Item>
        {onPersonalization&&<DropdownMenu.Item className="home-mode-manage" disabled={busy} onSelect={onPersonalization}>个性化设置<ArrowRightIcon aria-hidden="true"/></DropdownMenu.Item>}
      </DropdownMenu.Content>
    </DropdownMenu.Root>
    <span className="home-mode-status" id={statusId} role="status" aria-atomic="true">{status}</span>
    {!open && busy && <p className="home-mode-inline-status">正在保存场合…</p>}
    {!open && error && <div className="home-mode-error home-mode-inline-error" role="alert">
      <span>未能切换到“{error.name}”：{error.message}</span>
      <button type="button" disabled={busy || !catalog.sessionAvailable} onClick={() => void choose(error.mode)}>重新尝试</button>
    </div>}
  </div>;
}
