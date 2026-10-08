import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { Button, TextArea } from '@radix-ui/themes';
import { CheckIcon, CopyIcon } from '@radix-ui/react-icons';
import { serializeRecord, type Dictation } from '../model';
import { useDesktop } from '../services/DesktopProvider';

/** Feedback and clipboard recovery occupy document space, never cover content. */
export function CopyAction({ record, label = '复制', children }: {
  record: Dictation; label?: string; children?: ReactNode;
}) {
  const desktop = useDesktop();
  const [copied, setCopied] = useState(false);
  const [manual, setManual] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const textRef = useRef<HTMLTextAreaElement>(null);
  const generation = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const explanationId = useId();
  useEffect(() => {
    generation.current += 1;
    setCopied(false);
    setManual(false);
    return () => { generation.current += 1; clearTimeout(timer.current); };
  }, [record.id, record.revision]);
  useEffect(() => {
    if (!manual) return;
    textRef.current?.focus({ preventScroll: true });
    textRef.current?.select();
    textRef.current?.scrollIntoView({ block: 'nearest', behavior: 'instant' });
  }, [manual]);
  async function copy() {
    const current = generation.current;
    clearTimeout(timer.current);
    try {
      await desktop.command('result.copy', { resultId: record.id, sessionId: record.id, outputRevisionId: record.revision || null, sourceRevisionId: record.sourceRevisionId, sourceOnly: !record.detail?.output });
      if (current !== generation.current) return;
      setManual(false);
      setCopied(true);
      timer.current = setTimeout(() => setCopied(false), 3000);
    } catch {
      if (current !== generation.current) return;
      setCopied(false);
      setManual(true);
    }
  }
  function closeManual() {
    setManual(false);
    buttonRef.current?.focus({ preventScroll: true });
    buttonRef.current?.scrollIntoView({ block: 'nearest', behavior: 'instant' });
  }
  return <div className="copy-action">
    <div className="copy-actions-row">
      <Button ref={buttonRef} onClick={copy} disabled={!desktop.available || record.previewOnly} className="copy-primary" aria-label={label}>
        {copied ? <CheckIcon /> : <CopyIcon />}<span>{copied ? '已复制' : label}</span>
      </Button>
      {children}
      <span className="sr-only" role="status" aria-live="polite">{copied ? '已复制当前结果。' : ''}</span>
    </div>
    {manual && <section className="copy-recovery" aria-label="手动复制结果">
      <p id={explanationId}>自动复制未完成。文字已选中，按 Ctrl + C 手动复制。</p>
      <TextArea ref={textRef} aria-label="需要复制的结果" aria-describedby={explanationId} value={serializeRecord(record)} readOnly rows={6} />
      <div className="copy-recovery-actions"><Button variant="soft" color="gray" onClick={closeManual}>收起</Button></div>
    </section>}
  </div>;
}


