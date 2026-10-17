// Lucian UX/UI Prompt #008 · MIT License (c) 2026 Seokwoooo · https://lucianlabs.dev/uxui-prompt/#008
import './lucian-copy.css';

/** Only the original paired glyphs; CopyAction owns the real copy acknowledgement. */
export function LucianCopyMark({ copied, instant = false }: { copied: boolean; instant?: boolean }) {
  return <span className={`ux u008${copied ? ' is-copied' : ''}`} data-motion={instant ? 'instant' : 'normal'} aria-hidden="true">
    <svg className="u008-cp" viewBox="0 0 24 24" focusable="false">
      <rect x="8.5" y="8.5" width="11" height="11" rx="2.5" />
      <path d="M15.5 8.5V6.5a2 2 0 0 0-2-2h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2" />
    </svg>
    <svg className="u008-ok" viewBox="0 0 24 24" focusable="false">
      <path d="M5.5 12.5l4.2 4.2L18.5 8" />
    </svg>
  </span>;
}
