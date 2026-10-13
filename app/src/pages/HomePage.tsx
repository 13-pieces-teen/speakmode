import { Button, Kbd } from '@radix-ui/themes';
import { ArrowRightIcon, ChevronRightIcon, CounterClockwiseClockIcon, GearIcon, KeyboardIcon, GlobeIcon, SpeakerLoudIcon } from '@radix-ui/react-icons';
import type { Dictation, Mode } from '../model';
import { useDesktop } from '../services/DesktopProvider';
import { ModeBadge, ModeSelector } from '../components/ui';
import { LatestDictationPreview } from '../components/LatestDictationPreview';
import { CopyAction } from '../components/CopyAction';
import { HomeSectionCard } from '../components/HomeSectionCard';
import { UsageOverview } from '../components/UsageOverview';
import { EmptyStateNote } from '../components/EmptyStateNote';
import { languageLabels, type Preferences } from '../preferences';
import './home.css';

interface HomePageProps {
  preferences: Preferences;
  records: Dictation[];
  defaultMode: Mode;
  onDefaultModeChange: (mode: Mode) => void;
  onOpen: (id: string) => void;
  onHistory: () => void;
  onSettings: () => void;
  onManageModes: () => void;
}

function QuickSettings({ onSettings, preferences }: Pick<HomePageProps, 'onSettings' | 'preferences'>) {
  const desktop = useDesktop();
  const microphone = preferences.microphoneId ? desktop.devices.find(device => device.id === preferences.microphoneId)?.name ?? '所选设备暂不可用' : '系统默认';
  return <HomeSectionCard className="quick-settings" titleId="quick-title" title="快捷设置"
    action={<Button size="1" variant="ghost" color="gray" onClick={onSettings} aria-label="打开设置"><GearIcon /></Button>}>
    <dl><div><dt><KeyboardIcon />录音快捷键</dt><dd><Kbd>{preferences.shortcut}</Kbd></dd></div><div><dt><SpeakerLoudIcon />麦克风</dt><dd>{microphone}</dd></div><div><dt><GlobeIcon />识别语言</dt><dd>{languageLabels[preferences.language]}</dd></div></dl>
  </HomeSectionCard>;
}

function HomeShortcut({ shortcut, onSettings }: { shortcut: string; onSettings: () => void }) {
  const keys = shortcut.split(/\s*\+\s*/);
  return <button type="button" className="home-shortcut" onClick={onSettings}
    aria-label={`查看录音快捷键设置，当前为 ${shortcut}`} title="前往设置修改录音快捷键">
    <span className="home-shortcut-label">录音快捷键<GearIcon aria-hidden="true" /></span>
    <span className="home-shortcut-keys">{keys.map((key, index) => <span className="home-shortcut-key-part" key={`${index}:${key}`}>
      {index > 0 && <span className="home-shortcut-plus" aria-hidden="true">+</span>}<Kbd>{key}</Kbd>
    </span>)}</span>
  </button>;
}

export function HomePage({ preferences, records, defaultMode, onDefaultModeChange, onOpen, onHistory, onSettings, onManageModes }: HomePageProps) {
  const latest = records[0];
  const desktop = useDesktop();
  return <div className="home-page">
    <header className="home-hero">
      <div className="home-hero-copy"><h1>开口说，按你的方式整理。</h1>
        <p>按下 <Kbd>{preferences.shortcut}</Kbd> 开始，再按一次结束。<span className="hero-continuation">停顿或查资料时，录音继续。</span></p></div>
      <HomeShortcut shortcut={preferences.shortcut} onSettings={onSettings} />
    </header>
    <UsageOverview />
    <div className="home-grid">
      <div className="home-records">
        <HomeSectionCard className="latest-card" titleId="latest-title" title="最近一次口述"
          action={latest ? <time>{latest.temporary ? '临时结果' : latest.group} {latest.time}</time> : undefined}>
          {latest ? <>
          <div className="latest-title-row"><h3>{latest.title}</h3><ModeBadge mode={latest.mode} resolved={latest.resolved} name={latest.modeName} sourceState={latest.sourceState} /></div>
          <LatestDictationPreview key={`${latest.id}:${latest.revision}`} record={latest} />
          <div className="latest-actions"><CopyAction record={latest} label="复制结果"><Button variant="soft" color="gray" onClick={() => onOpen(latest.id)}>查看与调整<ArrowRightIcon /></Button>{latest.modified && <span className="modified-label">已在本页修改</span>}</CopyAction></div>
          </> : <div className="home-empty">
            <EmptyStateNote />
            <div className="home-empty-copy"><strong>从一句话开始</strong>
              <p>{desktop.available ? desktop.provider.configured ? <>按下 <Kbd>{preferences.shortcut}</Kbd>，说一句想留下的话。结束后，在这里查看、复制或调整。</> : '先在设置里配置云端服务，再开始第一段口述。' : '在桌面应用中完成第一段口述，结果会显示在这里。'}</p>
              <Button className="sm-tactile" variant="soft" onClick={onSettings}><GearIcon />查看设置</Button>
            </div>
          </div>}
        </HomeSectionCard>
        <HomeSectionCard className="recent-section" titleId="recent-title" title="更早的口述"
          action={<Button variant="ghost" size="1" onClick={onHistory}>全部记录<ArrowRightIcon /></Button>}>
          {records.length < 2 && <p className="muted">更早的已保存口述会显示在这里。</p>}
          {records.slice(1, 3).map(record => <button className="recent-item" key={record.id} onClick={() => onOpen(record.id)}>
            <span className="recent-icon"><CounterClockwiseClockIcon /></span>
            <span className="recent-copy"><strong>{record.title}</strong><span>{record.current.join(' ')}</span></span>
            <time>{record.time}</time><ChevronRightIcon />
          </button>)}
        </HomeSectionCard>
      </div>
      <aside className="home-controls" aria-label="录音偏好">
        <HomeSectionCard className="default-mode-card" titleId="default-mode-title" title="默认 Mode"
          action={<Button variant="ghost" size="1" onClick={onManageModes} aria-label="管理 Mode">管理<ArrowRightIcon /></Button>}>
          <ModeSelector value={defaultMode} onChange={onDefaultModeChange} compact label="新口述默认 Mode" />
          <p className="mode-footnote">下次口述生效，录音时也可切换。</p>
        </HomeSectionCard>
        <QuickSettings onSettings={onSettings} preferences={preferences} />
      </aside>
    </div>
  </div>;
}


