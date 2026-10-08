import { Button } from '@radix-ui/themes';
import { CheckIcon, PlusIcon } from '@radix-ui/react-icons';

const preferences = [
  { label: '保留英文术语', rule: '保留英文名称、产品名与缩写的原始拼写，不翻译成中文。' },
  { label: '简洁自然', rule: '减少冗余措辞，使用自然简洁的表达，保留全部实质信息。' },
  { label: '简体中文', rule: '中文部分使用简体中文，保留原有英文内容与专有名词。' },
  { label: '少些客套', rule: '直接表达原意，不额外添加客套话、承诺或结论。' },
] as const;

export function ExpressionPreferences({ value, disabled, onChange }: {
  value: string; disabled: boolean; onChange: (value: string) => void;
}) {
  const lines = value.split(/\r?\n/);
  return <div className="mode-expression-preferences" role="group" aria-labelledby="expression-preferences-label">
    <span id="expression-preferences-label" className="mode-field-label">表达偏好</span>
    <div className="mode-preference-options">{preferences.map(preference => {
      const selected = lines.some(line => line.trim() === preference.rule);
      const next = selected ? lines.filter(line => line.trim() !== preference.rule).join('\n')
        : `${value}${value && !value.endsWith('\n') ? '\n' : ''}${preference.rule}`;
      return <Button key={preference.label} type="button" size="1" variant={selected ? 'soft' : 'outline'}
        color={selected ? 'blue' : 'gray'} aria-pressed={selected}
        disabled={disabled || (!selected && Array.from(next).length > 4000)} onClick={() => onChange(next)}>
        {selected ? <CheckIcon /> : <PlusIcon />}{preference.label}
      </Button>;
    })}</div>
    <p className="mode-field-hint">点选后加入下方整理要求，可继续编辑，随这个 Mode 保存。</p>
  </div>;
}
