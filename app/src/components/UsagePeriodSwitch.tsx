// Lucian UX/UI Prompt #047 · MIT License (c) 2026 Seokwoooo · https://lucianlabs.dev/uxui-prompt/#047
import { useLayoutEffect, useRef, useState, type KeyboardEvent, type MouseEvent } from 'react';
import type { UsagePeriod } from '../hooks/useUsageOverview';
import './lucian-period.css';

const periods: { value: UsagePeriod; label: string }[] = [
  { value: 'today', label: '今日' },
  { value: 'total', label: '累计' },
];

function placeKnob(control: HTMLDivElement) {
  const selected = control.querySelector<HTMLButtonElement>('button[aria-pressed="true"]');
  if (!selected) return;
  control.style.setProperty('--l', `${selected.offsetLeft}px`);
  control.style.setProperty('--r', `${control.clientWidth - selected.offsetLeft - selected.offsetWidth}px`);
}

/** Port of the period selector only. Ledger values are rendered by UsageOverview. */
export function UsagePeriodSwitch({ value, onChange }: { value: UsagePeriod; onChange: (value: UsagePeriod) => void }) {
  const root = useRef<HTMLDivElement>(null);
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  const [direction, setDirection] = useState(0);
  const [instant, setInstant] = useState(true);
  const index = periods.findIndex(period => period.value === value);

  useLayoutEffect(() => {
    const control = root.current;
    if (control) placeKnob(control);
  }, [index]);

  useLayoutEffect(() => {
    const control = root.current;
    if (!control) return;
    const observer = new ResizeObserver(() => { setInstant(true); placeKnob(control); });
    observer.observe(control);
    return () => observer.disconnect();
  }, []);

  function choose(next: number, immediate: boolean) {
    if (next === index) return;
    setInstant(immediate);
    setDirection(next > index ? 1 : -1);
    onChange(periods[next].value);
  }

  function click(event: MouseEvent<HTMLButtonElement>, next: number) {
    choose(next, event.detail === 0);
  }

  function key(event: KeyboardEvent<HTMLButtonElement>, current: number) {
    const delta = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
    if (!delta) return;
    event.preventDefault();
    const next = (current + delta + periods.length) % periods.length;
    choose(next, true);
    buttons.current[next]?.focus();
  }

  return <div ref={root} className={`usage-range ux u047 u047-seg${direction > 0 ? ' go-r' : direction < 0 ? ' go-l' : ''}`}
    role="group" aria-label="统计范围" data-motion={instant ? 'instant' : 'normal'}>
    <span className="u047-knob" aria-hidden="true" />
    {periods.map((period, position) => <button key={period.value} ref={button => { buttons.current[position] = button; }}
      type="button" className="u047-opt" aria-pressed={value === period.value} tabIndex={value === period.value ? 0 : -1}
      onClick={event => click(event, position)} onKeyDown={event => key(event, position)}>{period.label}</button>)}
  </div>;
}
