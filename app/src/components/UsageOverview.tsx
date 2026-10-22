import { useState } from 'react';
import { InfoCircledIcon } from '@radix-ui/react-icons';
import { useUsageOverview, type UsagePeriod } from '../hooks/useUsageOverview';
import { UsagePeriodSwitch } from './UsagePeriodSwitch';
import './UsageOverview.css';

const usageNumber = new Intl.NumberFormat('zh-CN');

export function UsageOverview() {
  const [period, setPeriod] = useState<UsagePeriod>('today');
  const usage = useUsageOverview(period);
  const title = period === 'today' ? '今日概览' : '累计概览';
  const seconds = usage.value ? Math.floor(usage.value.captureDurationMs / 1000) : null;
  const dateLabel = usage.date.replace(/^(\d{4})-(\d{2})-(\d{2})$/, '$1 年 $2 月 $3 日');
  return <section className="usage-overview" aria-label={title} aria-busy={usage.loading}>
    <div className="usage-period">
      <h2>{title}</h2>
      <p>{period === 'today' ? dateLabel : '此设备已完成的口述'}</p>
    </div>
    <dl className="usage-metrics">
      <div className="usage-metric"><dt>口述次数</dt><dd><strong>{usage.value ? usageNumber.format(usage.value.count) : '—'}</strong><span>次</span></dd></div>
      <div className="usage-metric"><dt>口述字数</dt><dd><strong>{usage.value ? usageNumber.format(usage.value.sourceCharCount) : '—'}</strong><span>字</span></dd></div>
      <div className="usage-metric"><dt>口述时长</dt><dd><strong>{seconds === null ? '—' : usageNumber.format(Math.floor(seconds / 60))}</strong><span>分</span><strong>{seconds === null ? '—' : seconds % 60}</strong><span>秒</span></dd></div>
    </dl>
    <UsagePeriodSwitch value={period} onChange={setPeriod} />
    <details className="usage-info"><summary aria-label="用量统计说明"><InfoCircledIcon aria-hidden="true" /><span className="sr-only">统计说明</span></summary>
      <p>{period === 'today' ? `${dateLabel} · 本机日期。` : '此设备已完成的全部口述。'}用量保存在本机；修改或删除历史记录不影响统计。</p>
    </details>
    <div className="usage-read-status" role="status">
      {!usage.available ? '在桌面应用中读取已保存的用量。' : usage.failed ? <>
        <span>{usage.value ? '更新失败，显示上次读取的用量。' : '用量暂时未读到。'}</span>
        <button type="button" onClick={() => void usage.reload()}>重新读取</button>
      </> : usage.loading ? (usage.value ? '正在更新…' : '正在读取已保存的用量…') :
        period === 'today' && usage.value?.count === 0 ? <span>今天还没有完成的口述，可切换「累计」查看之前的用量。</span> : null}
    </div>
  </section>;
}
