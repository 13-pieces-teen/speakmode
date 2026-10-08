import { Badge, RadioCards } from '@radix-ui/themes';
import { CheckIcon, ListBulletIcon, MixerHorizontalIcon, TextAlignLeftIcon, Pencil2Icon } from '@radix-ui/react-icons';
import type { Dictation, DictationSourceState, Mode, ResolvedMode } from '../model';
import { builtinModes, isPersonalMode } from '../modes';
import { useModes } from '../hooks/useModes';

export function ModeIcon({ mode }: { mode: Mode }) {
  const Icon = isPersonalMode(mode) ? Pencil2Icon : mode === '自动' ? MixerHorizontalIcon : mode === '结构' ? ListBulletIcon : TextAlignLeftIcon;
  return <Icon aria-hidden="true" />;
}

export function ModeSelector({ value, onChange, compact = false, label = '选择 Mode' }: {
  value: Mode; onChange: (mode: Mode) => void; compact?: boolean; label?: string;
}) {
  const { modes, sessionAvailable } = useModes();
  const modeOptions = [...builtinModes.map(mode => ({ value: mode.id as Mode, name: mode.id, caption: mode.caption })), ...modes.map(mode => ({ value: mode.id as Mode, name: mode.name, caption: `我的 Mode · 基于${mode.base}` }))];
  return <RadioCards.Root className={`mode-selector${compact ? ' mode-selector--compact' : ''}`}
    columns="1" gap="2" size="1" value={value} onValueChange={next => onChange(next as Mode)} aria-label={label}>
    {modeOptions.map(option => <RadioCards.Item className="mode-option" data-personal={isPersonalMode(option.value)} key={option.value} value={option.value} disabled={!sessionAvailable}>
      <span className="mode-option-icon"><ModeIcon mode={option.value} /></span>
      <span className="mode-option-copy"><strong>{option.name}</strong><span>{option.caption}</span></span>
      <span className="mode-option-check" aria-hidden="true">{value === option.value && <CheckIcon />}</span>
    </RadioCards.Item>)}
  </RadioCards.Root>;
}

export function ModeBadge({ mode, resolved, name, sourceState }: { mode: Mode; resolved?: ResolvedMode|null; name?: string; sourceState?: DictationSourceState }) {
  const { label } = useModes();
  const sourceLabel = sourceState === 'unprocessed' ? '未整理' : sourceState === 'source' ? '识别稿' : null;
  return <Badge className="mode-badge" color={sourceLabel ? 'gray' : 'blue'} variant="soft" radius="medium" size="1">
    {sourceLabel ? <TextAlignLeftIcon aria-hidden="true" /> : <ModeIcon mode={mode} />}{sourceLabel ?? name ?? label(mode)}{!sourceLabel && sourceState === 'model' && mode === '自动' && resolved ? ` · ${resolved}` : ''}
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

