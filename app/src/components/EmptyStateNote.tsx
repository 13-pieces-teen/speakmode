import './EmptyStateNote.css';

/** Original product illustrations, separate from any recording or saved content. */
export function EmptyStateNote({ variant = 'dictation' }: { variant?: 'dictation' | 'dictionary' }) {
  return <div className={`empty-state-note empty-state-note--${variant}`} aria-hidden="true">
    <img src={`/illustrations/${variant === 'dictionary' ? 'personal-vocabulary' : 'voice-to-words'}.svg`}
      width="144" height="120" alt="" draggable="false" />
  </div>;
}
