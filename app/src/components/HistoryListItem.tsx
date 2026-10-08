import type { Ref } from 'react';
import { modeLabel, type Dictation } from '../model';
import { ModeIcon } from './ui';
import './HistoryListItem.css';

interface HistoryListItemProps {
  record: Dictation;
  selected: boolean;
  onSelect: (id: string) => void;
  buttonRef?: Ref<HTMLButtonElement>;
}

export function HistoryListItem({ record, selected, onSelect, buttonRef }: HistoryListItemProps) {
  return (
    <button
      ref={buttonRef}
      type="button"
      className="history-record-button"
      data-selected={selected}
      aria-current={selected ? 'true' : undefined}
      aria-label={`${record.title}，${record.group} ${record.time}，${modeLabel(record)}${record.modified ? '，已修改' : ''}`}
      onClick={() => onSelect(record.id)}
    >
      <span className="history-record-title">{record.title}</span>
      <span className="history-record-preview">{record.current.join(' ')}</span>
      <span className="history-record-meta">
        <time className="history-record-time">{record.time}</time>
        {record.modified && <span className="history-record-modified">已修改</span>}
        <span className="history-record-mode">
          <ModeIcon mode={record.mode} /><span>{modeLabel(record)}</span>
        </span>
      </span>
    </button>
  );
}
