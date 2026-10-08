import { useCallback, useEffect, useRef, useState } from 'react';
import { useDesktop } from '../services/DesktopProvider';
import type { DailyUsage, UsageTotals } from '../services/contracts';

export type UsagePeriod = 'today' | 'total';
interface UsageRead {
  period: UsagePeriod;
  date: string;
  value: UsageTotals | null;
  loading: boolean;
  failed: boolean;
}

function localDate() {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
}

/** The SQLite ledger is authoritative; the UI never accumulates or persists counters. */
export function useUsageOverview(period: UsagePeriod) {
  const { available, command, usage } = useDesktop();
  const [read, setRead] = useState<UsageRead | null>(null);
  const request = useRef(0);
  const observedDate = useRef(localDate());
  const reload = useCallback(async () => {
    if (!available) return;
    const run = ++request.current;
    const date = localDate();
    observedDate.current = date;
    setRead(previous => ({
      period, date,
      value: previous?.period === period && (period === 'total' || previous.date === date) ? previous.value : null,
      loading: true, failed: false,
    }));
    try {
      const value = period === 'today'
        ? await command<DailyUsage>('usage.today')
        : await command<UsageTotals>('usage.total');
      if (run !== request.current) return;
      // A slow response spanning midnight must not relabel yesterday as today.
      if (period === 'today' && (!('date' in value) || value.date !== localDate())) {
        setRead({ period, date: localDate(), value: null, loading: false, failed: true });
        return;
      }
      const resultDate = period === 'today' ? (value as DailyUsage).date : localDate();
      observedDate.current = resultDate;
      setRead({ period, date: resultDate, value, loading: false, failed: false });
    } catch {
      if (run === request.current) setRead(previous => previous ? { ...previous, loading: false, failed: true } : previous);
    }
  }, [available, command, period]);

  // Committed native usage changes update the visible range, including totals.
  useEffect(() => {
    void reload();
    return () => { request.current++; };
  }, [reload, usage.date, usage.count, usage.sourceCharCount, usage.captureDurationMs]);

  useEffect(() => {
    if (!available) return;
    const visible = () => { if (document.visibilityState === 'visible') void reload(); };
    const checkDate = () => {
      if (document.visibilityState === 'visible' && observedDate.current !== localDate()) void reload();
    };
    window.addEventListener('focus', visible);
    document.addEventListener('visibilitychange', visible);
    const timer = window.setInterval(checkDate, 30_000);
    return () => {
      window.removeEventListener('focus', visible);
      document.removeEventListener('visibilitychange', visible);
      window.clearInterval(timer);
    };
  }, [available, reload]);

  const current = read?.period === period ? read : null;
  return {
    available,
    value: current?.value ?? null,
    date: current?.date ?? localDate(),
    loading: available && (!current || current.loading),
    failed: current?.failed ?? false,
    reload,
  };
}
