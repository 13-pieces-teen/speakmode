import { useState, type CSSProperties } from 'react';
import { InfoCircledIcon } from '@radix-ui/react-icons';
import { useUsageOverview, type UsagePeriod } from '../hooks/useUsageOverview';
import { UsagePeriodSwitch } from './UsagePeriodSwitch';
import './UsageOverview.css';

const usageNumber = new Intl.NumberFormat('zh-CN');

function numberWidth(...values: string[]): CSSProperties {
  // Conservative em widths keep the exact totals, including grouping separators.
  const width = [...values.join('')].reduce((sum, char) => sum + (char === '—' ? 1 : char === ',' ? 0.35 : 0.65), 0);
  return { '--usage-number-width': width } as CSSProperties;
}

export function UsageOverview() {
  const [period, setPeriod] = useState<UsagePeriod>('today');
  const usage = useUsageOverview(period);
  const title = period === 'today' ? '今日概览' : '累计概览';
  const seconds = usage.value ? Math.floor(usage.value.captureDurationMs / 1000) : null;
  const count = usage.value ? usageNumber.format(usage.value.count) : '—';
  const characters = usage.value ? usageNumber.format(usage.value.sourceCharCount) : '—';
  const minutes = seconds === null ? '—' : usageNumber.format(Math.floor(seconds / 60));
  const remainder = seconds === null ? '—' : String(seconds % 60);
  const dateLabel = usage.date.replace(/^(\d{4})-(\d{2})-(\d{2})$/, '$1 年 $2 月 $3 日');

  return <section className="usage-overview" aria-label={title} aria-busy={usage.loading}>
    <div className="usage-period">
      <h2>{title}</h2>
      <p className="usage-read-status" role="status">
        {!usage.available ? '在桌面应用中查看用量' : usage.failed ? <>
          <span>{usage.value ? '更新失败 · 旧值' : '读取失败'}</span>
          <button type="button" onClick={() => void usage.reload()}>重新读取</button>
        </> : usage.loading ? (usage.value ? '正在更新…' : '正在读取…') :
          period === 'today' && usage.value?.count === 0 ? '今日暂无口述' :
            period === 'today' ? dateLabel : '此设备已完成的口述'}
      </p>
    </div>
    <dl className="usage-metrics">
      <div className="usage-metric"><dt>口述次数</dt><dd style={numberWidth(count)}><strong>{count}</strong><span>次</span></dd></div>
      <div className="usage-metric"><dt>口述字数</dt><dd style={numberWidth(characters)}><strong>{characters}</strong><span>字</span></dd></div>
      <div className="usage-metric"><dt>口述时长</dt><dd className="usage-duration" style={numberWidth(minutes, remainder)}><strong>{minutes}</strong><span>分</span><strong>{remainder}</strong><span>秒</span></dd></div>
    </dl>
    <UsagePeriodSwitch value={period} onChange={setPeriod} />
    <details className="usage-info"><summary aria-label="用量统计说明"><InfoCircledIcon aria-hidden="true" /><span className="sr-only">统计说明</span></summary>
      <p>{dateLabel} · 本机日期。统计此设备已完成的口述，用量保存在本机；修改或删除历史记录不影响统计。今日暂无口述时，可切换「累计」查看之前的用量。</p>
    </details>
  </section>;
}
