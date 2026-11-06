import { Badge, RadioCards } from '@radix-ui/themes';
import { CheckIcon, ListBulletIcon, MixerHorizontalIcon, TextAlignLeftIcon, Pencil2Icon, ChatBubbleIcon, FileTextIcon, EnvelopeClosedIcon, CodeIcon } from '@radix-ui/react-icons';
import { writingReceiptSceneLabel, type Dictation, type DictationSourceState, type Mode, type ResolvedMode } from '../model';
import { scenarioOptions, isPersonalMode } from '../modes';
import { useModes } from '../hooks/useModes';
import type { WritingReceipt } from '../services/contracts';

export function ModeIcon({ mode }: { mode: Mode }) {
  const Icon = isPersonalMode(mode) ? Pencil2Icon : mode === '自动' ? MixerHorizontalIcon : mode === '结构' ? ListBulletIcon : mode === '聊天消息' ? ChatBubbleIcon : mode === '文档写作' ? FileTextIcon : mode === '邮件沟通' ? EnvelopeClosedIcon : mode === '技术沟通' ? CodeIcon : TextAlignLeftIcon;
  return <Icon aria-hidden="true" />;
}

export function ModeSelector({ value, onChange, compact = false, label = '选择使用场合' }: {
  value: Mode; onChange: (mode: Mode) => void; compact?: boolean; label?: string;
}) {
  const { modes, sessionAvailable } = useModes();
  const modeOptions = scenarioOptions(value,modes);
  return <RadioCards.Root className={`mode-selector${compact ? ' mode-selector--compact' : ''}`}
    columns="1" gap="2" size="1" value={value} onValueChange={next => onChange(next as Mode)} aria-label={label}>
    {modeOptions.map(option => <RadioCards.Item className="mode-option" data-personal={isPersonalMode(option.id)} key={option.id} value={option.id} disabled={!sessionAvailable}>
      <span className="mode-option-icon"><ModeIcon mode={option.id} /></span>
      <span className="mode-option-copy"><strong>{option.name}</strong><span>{option.caption}</span></span>
      <span className="mode-option-check" aria-hidden="true">{value === option.id && <CheckIcon />}</span>
    </RadioCards.Item>)}
  </RadioCards.Root>;
}

export function ModeBadge({ mode, resolved, name, sourceState, receipt }: { receipt?:WritingReceipt; mode: Mode; resolved?: ResolvedMode|null; name?: string; sourceState?: DictationSourceState }) {
  const { label } = useModes();
  const sourceLabel = sourceState === 'unprocessed' ? '未整理' : sourceState === 'basic' ? '基础整理' : sourceState === 'source' || sourceState === 'fallback' ? '识别稿' : null;
  return <Badge className="mode-badge" color={sourceLabel ? 'gray' : 'blue'} variant="soft" radius="medium" size="1">
    {sourceLabel ? <TextAlignLeftIcon aria-hidden="true" /> : <ModeIcon mode={mode} />}{sourceLabel ?? name ?? label(mode)}{!sourceLabel && sourceState === 'model' && mode === '自动' ? receipt ? (writingReceiptSceneLabel(receipt)?` · ${writingReceiptSceneLabel(receipt)}`:'') : resolved ? ` · ${resolved}` : '' : ''}
  </Badge>;
}

/** Show the actual output text. A Mode label must not invent another list layer. */
export function OutputBlocks({ texts, className = '' }: { texts: string[]; className?: string }) {
  return <div className={`record-text ${className}`.trim()}>
    {texts.map((text, i) => <p key={i}>{text}</p>)}
  </div>;
}

export function RecordText({ record }: { record: Dictation }) {
  return <OutputBlocks texts={record.current} />;
}

export function PageHeading({ title, description }: { title: string; description?: string }) {
  return <header className="page-heading"><h1>{title}</h1>{description && <p>{description}</p>}</header>;
}

