import { ChatBubbleIcon, Pencil2Icon } from '@radix-ui/react-icons';
import './EmptyStateNote.css';

/** A quiet paper illustration. It never represents recording or saved content. */
export function EmptyStateNote({ variant = 'dictation' }: { variant?: 'dictation' | 'dictionary' }) {
  return <div className={`empty-state-note empty-state-note--${variant}`} aria-hidden="true">
    <span className="empty-state-note-wash" />
    <span className="empty-state-note-sheet">
      {variant === 'dictionary' && <span className="empty-state-note-letter">Aa</span>}
      <span className="empty-state-note-line" />
      <span className="empty-state-note-line" />
      {variant === 'dictation' && <span className="empty-state-note-line" />}
    </span>
    <span className="empty-state-note-badge">{variant === 'dictionary' ? <Pencil2Icon /> : <ChatBubbleIcon />}</span>
  </div>;
}
