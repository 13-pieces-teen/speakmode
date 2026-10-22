import { Button } from '@radix-ui/themes';
import { ChevronDownIcon, SpeakerLoudIcon, ArrowRightIcon } from '@radix-ui/react-icons';
import { useDesktop } from '../services/DesktopProvider';
import { languageLabels, type Preferences } from '../preferences';
import './InputPreferences.css';

/** A readable summary, with settings details only when requested. */
export function InputPreferences({ preferences, onSettings }: { preferences: Preferences; onSettings: () => void }) {
  const desktop = useDesktop();
  const microphone = preferences.microphoneId
    ? desktop.devices.find(device => device.id === preferences.microphoneId)?.name ?? '所选设备暂不可用'
    : '系统默认';
  const language = languageLabels[preferences.language];
  return <details className="input-preferences">
    <summary aria-label={`输入偏好：${microphone}，${language}`}>
      <SpeakerLoudIcon aria-hidden="true" />
      <span className="input-summary"><span className="preference-label">输入</span><span>{microphone} · {language}</span></span>
      <ChevronDownIcon className="input-chevron" aria-hidden="true" />
    </summary>
    <div className="input-preferences-body">
      <dl><div><dt>麦克风</dt><dd>{microphone}</dd></div><div><dt>识别语言</dt><dd>{language}</dd></div></dl>
      <Button variant="ghost" size="1" onClick={onSettings}>修改输入设置<ArrowRightIcon aria-hidden="true" /></Button>
    </div>
  </details>;
}
