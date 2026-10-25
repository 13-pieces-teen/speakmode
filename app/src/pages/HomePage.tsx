import { Button, Kbd } from '@radix-ui/themes';
import { ArrowRightIcon, ChevronRightIcon, CounterClockwiseClockIcon, GearIcon } from '@radix-ui/react-icons';
import type { Dictation, Mode } from '../model';
import { useDesktop } from '../services/DesktopProvider';
import { ModeBadge } from '../components/ui';
import { LatestDictationPreview } from '../components/LatestDictationPreview';
import { CopyAction } from '../components/CopyAction';
import { HomeSectionCard } from '../components/HomeSectionCard';
import { HomeModePicker } from '../components/HomeModePicker';
import { InputPreferences } from '../components/InputPreferences';
import { UsageOverview } from '../components/UsageOverview';
import { EmptyStateNote } from '../components/EmptyStateNote';
import type { Preferences } from '../preferences';
import './home.css';

interface HomePageProps {
  preferences: Preferences;
  records: Dictation[];
  defaultMode: Mode;
  onDefaultModeChange: (mode: Mode) => Promise<void>;
  onOpen: (id: string) => void;
  onHistory: () => void;
  onSettings: () => void;
  onManageModes: () => void;
  onPersonalization: () => void;
}

function HomeShortcut({ shortcut, onSettings }: { shortcut: string; onSettings: () => void }) {
  const keys = shortcut.split(/\s*\+\s*/);
  return <button type="button" className="home-shortcut" onClick={onSettings}
    aria-label={`查看录音快捷键设置，当前为 ${shortcut}`} title="前往设置修改录音快捷键">
    <span className="home-shortcut-keys">{keys.map((key, index) => <span className="home-shortcut-key-part" key={`${index}:${key}`}>
      {index > 0 && <span className="home-shortcut-plus" aria-hidden="true">+</span>}<Kbd>{key}</Kbd>
    </span>)}</span>
    <span>开始 / 结束</span><GearIcon aria-hidden="true" />
  </button>;
}

function RecentDictations({ records, onOpen, onHistory }: Pick<HomePageProps, 'records' | 'onOpen' | 'onHistory'>) {
  return <HomeSectionCard className="recent-section" titleId="recent-title" title="更早的口述"
    action={<Button variant="ghost" size="1" onClick={onHistory}>全部记录<ArrowRightIcon /></Button>}>
    {records.length < 2 && <p className="muted">完成更多口述后，在这里继续查看。</p>}
    {records.slice(1, 3).map(record => <button className="recent-item" key={record.id} onClick={() => onOpen(record.id)}>
      <CounterClockwiseClockIcon className="recent-icon" aria-hidden="true" />
      <span className="recent-copy"><strong>{record.title}</strong></span>
      <time>{record.time}</time><ChevronRightIcon aria-hidden="true" />
    </button>)}
  </HomeSectionCard>;
}

export function HomePage({ preferences, records, defaultMode, onDefaultModeChange, onOpen, onHistory, onSettings, onManageModes, onPersonalization }: HomePageProps) {
  const latest = records[0];
  const desktop = useDesktop();
  return <div className="home-page">
    <header className="home-hero">
      <h1><span>开口说，</span><span>按照你的方式来整理</span></h1>
      <HomeShortcut shortcut={preferences.shortcut} onSettings={onSettings} />
    </header>
    <UsageOverview />
    <section className="home-preferences" aria-label="口述偏好">
      <HomeModePicker value={defaultMode} onChange={onDefaultModeChange} onManage={onManageModes} onPersonalization={onPersonalization} />
      <InputPreferences preferences={preferences} onSettings={onSettings} />
    </section>
    <HomeSectionCard className="latest-card" titleId="latest-title" title="最近一次口述"
      action={latest ? <time>{latest.temporary ? '临时结果' : latest.group} {latest.time}</time> : undefined}>
      {latest ? <>
        <div className="latest-metadata"><ModeBadge mode={latest.mode} resolved={latest.resolved} name={latest.modeName}
          sourceState={latest.sourceState} receipt={latest.detail?.output?.writingReceipt} />
          {latest.modified && <span className="modified-label">已修改</span>}
        </div>
        <LatestDictationPreview key={`${latest.id}:${latest.revision}`} record={latest} />
        <div className="latest-actions"><CopyAction record={latest} label="复制结果">
          <Button variant="ghost" color="gray" onClick={() => onOpen(latest.id)}>查看与调整<ArrowRightIcon /></Button>
        </CopyAction></div>
      </> : <div className="home-empty">
        <EmptyStateNote />
        <div className="home-empty-copy"><strong>从一句话开始</strong>
          <p>{desktop.available ? desktop.provider.configured
            ? <>按下 <Kbd>{preferences.shortcut}</Kbd> 开始，再按一次结束。<br />停顿或查资料时，录音继续。</>
            : '先配置云端服务，再开始第一段口述。'
            : '在桌面应用中开始口述，结果会显示在这里。'}</p>
          <Button variant="soft" onClick={onSettings}><GearIcon />查看设置</Button>
        </div>
      </div>}
    </HomeSectionCard>
    <RecentDictations records={records} onOpen={onOpen} onHistory={onHistory} />
  </div>;
}
