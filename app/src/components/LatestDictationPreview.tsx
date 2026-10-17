// Lucian UX/UI Prompt #288 · MIT License (c) 2026 Seokwoooo · https://lucianlabs.dev/uxui-prompt/#288
import { useEffect, useId, useLayoutEffect, useRef, useState, type MouseEvent } from 'react';
import type { Dictation } from '../model';
import { RecordText } from './ui';
import './lucian-details.css';

/** Bounded reading with the disclosure from #288; copying still uses the full revision. */
export function LatestDictationPreview({ record }: { record: Dictation }) {
  const [expanded, setExpanded] = useState(false);
  const [overflowing, setOverflowing] = useState(false);
  const [height, setHeight] = useState<number>();
  const [instant, setInstant] = useState(true);
  const viewportRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const toggleRef = useRef<HTMLButtonElement>(null);
  const expandedRef = useRef(false);
  const stopScroll = useRef<() => void>(() => {});
  const id = useId();
  const text = record.current.join('\n');

  useLayoutEffect(() => {
    const viewport = viewportRef.current, content = contentRef.current;
    if (!viewport || !content) return;
    let alive = true;
    const measure = () => {
      if (!alive) return;
      const full = content.scrollHeight;
      const limit = Number.parseFloat(getComputedStyle(viewport).getPropertyValue('--latest-preview-height')) || full;
      setOverflowing(full > limit + 1);
      setHeight(expandedRef.current ? full : Math.min(full, limit));
    };
    const observer = new ResizeObserver(measure);
    observer.observe(content);
    measure();
    return () => { alive = false; observer.disconnect(); };
  }, [text]);

  useEffect(() => () => stopScroll.current(), []);

  function returnToReadingStart(immediate: boolean) {
    const viewport = viewportRef.current;
    const main = viewport?.closest<HTMLElement>('.app-main');
    if (!viewport || !main) return;
    if (immediate) {
      const frame = requestAnimationFrame(() => toggleRef.current?.scrollIntoView({ block: 'nearest', behavior: 'instant' }));
      stopScroll.current = () => cancelAnimationFrame(frame);
      return;
    }
    const from = main.scrollTop;
    const target = Math.max(0, from + viewport.getBoundingClientRect().top - main.getBoundingClientRect().top - 12);
    const start = performance.now();
    let frame = 0;
    let stopped = false;
    const stop = () => {
      stopped = true;
      cancelAnimationFrame(frame);
      main.removeEventListener('wheel', stop);
      main.removeEventListener('touchstart', stop);
      main.removeEventListener('pointerdown', stop);
      main.removeEventListener('keydown', stopForKey);
    };
    function stopForKey(event: KeyboardEvent) {
      if (['PageUp', 'PageDown', 'Home', 'End', ' ', 'ArrowUp', 'ArrowDown'].includes(event.key)) stop();
    }
    stopScroll.current = stop;
    main.addEventListener('wheel', stop, { passive: true });
    main.addEventListener('touchstart', stop, { passive: true });
    main.addEventListener('pointerdown', stop, { passive: true });
    main.addEventListener('keydown', stopForKey);
    const step = (now: number) => {
      if (stopped || !main.isConnected) { stop(); return; }
      const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
      const progress = reduced ? 1 : Math.min(1, (now - start) / 650);
      const eased = progress < .5 ? 2 * progress * progress : 1 - Math.pow(-2 * progress + 2, 2) / 2;
      main.scrollTop = from + (target - from) * eased;
      if (progress < 1) frame = requestAnimationFrame(step);
      else stop();
    };
    frame = requestAnimationFrame(step);
  }

  function toggle(event: MouseEvent<HTMLButtonElement>) {
    stopScroll.current();
    const viewport = viewportRef.current, content = contentRef.current;
    if (!viewport || !content) return;
    const immediate = event.detail === 0 || window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    const next = !expanded;
    const full = content.scrollHeight;
    const limit = Number.parseFloat(getComputedStyle(viewport).getPropertyValue('--latest-preview-height')) || full;
    expandedRef.current = next;
    setInstant(immediate);
    setExpanded(next);
    setHeight(next ? full : Math.min(full, limit));
    if (!next) returnToReadingStart(immediate);
  }

  return <div className={`latest-preview ux u288${expanded ? ' is-open' : ''}`} data-motion={instant ? 'instant' : 'normal'}>
    <div id={id} ref={viewportRef} className="latest-preview-viewport u288-dt" data-expanded={expanded} data-overflowing={overflowing}
      style={{ height }}>
      <div ref={contentRef} className="u288-in"><RecordText record={record} /></div>
    </div>
    {overflowing && <div className="latest-preview-footer">
      <span className="sr-only">{expanded ? '已显示完整内容' : '内容较长，当前显示部分预览'}</span>
      <button ref={toggleRef} type="button" className="u288-mr" aria-expanded={expanded} aria-controls={id} onClick={toggle}>
        <span>{expanded ? '收起全文' : '展开全文'}</span>
        <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2.5 4.5L6 8l3.5-3.5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" /></svg>
      </button>
    </div>}
  </div>;
}
