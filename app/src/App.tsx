import {useEffect,useRef,useState} from 'react';
import {Button} from '@radix-ui/themes';
import {AppShell,type View} from './components/AppShell';
import {HomePage} from './pages/HomePage';
import {HistoryPage} from './pages/HistoryPage';
import {SettingsPage} from './pages/SettingsPage';
import {ModesPage} from './pages/ModesPage';
import {DictionaryPage} from './pages/DictionaryPage';
import {ModeProvider,useModes} from './hooks/useModes';
import {useDictations} from './hooks/useDictations';
import {DesktopProvider,useDesktop} from './services/DesktopProvider';
import {NavigationGuardProvider,useNavigationGuard} from './components/NavigationGuard';
import {fromNativePreferences,toNativePatch,type Preferences} from './preferences';
import {fromDetail,type Mode} from './model';
import {errorMessage} from './services/bridge';
import {RecorderWindow} from './windows/RecorderWindow';
import {SessionRecovery} from './components/SessionRecovery';
import {RetainedRecovery} from './components/RetainedRecovery';
import {OnboardingFlow} from './onboarding/OnboardingFlow';
import {WindowChrome} from './components/WindowChrome';
export function App(){return <DesktopProvider><ModeProvider><NavigationGuardProvider>{new URLSearchParams(location.search).get('window')==='recorder'?<RecorderWindow/>:<AppContent/>}</NavigationGuardProvider></ModeProvider></DesktopProvider>;}
function AppContent(){
 const desktop=useDesktop(),dictations=useDictations(),modes=useModes(),navigation=useNavigationGuard();
 const [view,setView]=useState<View>('home'),[selectedId,setSelectedId]=useState<string|null>(null),[modeOrigin,setModeOrigin]=useState<'home'|'settings'>('home'),[modeDirty,setModeDirty]=useState(false),[pendingView,setPendingView]=useState<View|'recorder'|null>(null),[error,setError]=useState('');
 const preferences=fromNativePreferences(desktop.preferences);
 const autoOpened=useRef(false);
 useEffect(()=>{
  if(desktop.status!=='ready'||!desktop.onboarding||autoOpened.current)return;
  if(!desktop.onboarding.progress.autoShow||desktop.onboarding.active){autoOpened.current=true;return;}
  autoOpened.current=true;
  void desktop.command('onboarding.open').then(()=>desktop.refresh()).catch(e=>setError(errorMessage(e)));
 },[desktop.status,desktop.onboarding,desktop.command,desktop.refresh]);
 const showError=(e:unknown)=>setError(errorMessage(e));
 function openRecord(id:string){navigation.request(()=>{setSelectedId(id);setView('history');});}
 function commitNavigation(next:View|'recorder'){setModeDirty(false);setPendingView(null);if(next==='recorder'){void desktop.command('session.start').catch(showError);return;}setView(next);if(next==='history')setSelectedId(null);if(next==='home')void desktop.searchHistory('').catch(showError);}
 function navigate(next:View|'recorder'){if(view==='modes'&&modeDirty){setPendingView(next);return;}navigation.request(()=>commitNavigation(next));}
 function manageModes(origin:'home'|'settings'){navigation.request(()=>{setModeOrigin(origin);setView('modes');});}
 async function updatePreferences(next:Preferences,captureId?:string){await desktop.command('preferences.update',{patch:toNativePatch(next),expectedVersion:desktop.preferences.version,...(captureId?{captureId}:{})});await desktop.refresh();}
 async function updateDefault(mode:Mode){await modes.setDefaultMode(mode);}
 function openOnboarding(){navigation.request(()=>{void desktop.command('onboarding.open').then(()=>desktop.refresh()).catch(showError);});}
 const homeRecords=desktop.temporaryResult?[fromDetail(desktop.temporaryResult,true),...dictations.records.filter(r=>r.id!==desktop.temporaryResult?.session.id)]:dictations.records;
 if(desktop.onboarding?.active)return <OnboardingFlow state={desktop.onboarding}/>;
 const preparingGuide=desktop.status==='ready'&&desktop.onboarding?.progress.autoShow&&!desktop.onboarding.active&&!error&&!desktop.error;
 if(desktop.available&&(desktop.status==='loading'||preparingGuide))return <div className="onboarding-loading"><WindowChrome available/><p role="status">正在连接 SpeakMode…</p></div>;
 return <AppShell view={view} onNavigate={navigate} onRecorder={()=>navigate('recorder')}>
  {!desktop.available&&<div className="environment-banner" role="status"><strong>请在 SpeakMode 桌面应用中使用</strong><p>此浏览器页面只展示界面。录音、云服务、历史和设置均从桌面原生服务读取，这里不会生成示例数据。</p></div>}
  {desktop.available&&desktop.status==='loading'&&<p role="status" className="environment-banner">正在连接桌面服务…</p>}
  {desktop.available&&!desktop.provider.configured&&view!=='settings'&&<div className="environment-banner"><div><strong>先配置云端服务</strong><p>选择语音识别与文字整理服务，保存对应的 API Key 后即可开始口述。</p></div><Button variant="soft" onClick={()=>navigate('settings')}>前往设置</Button></div>}
  {(error||desktop.error)&&<div role="alert" className="environment-banner environment-error"><p>{error||desktop.error}</p><Button variant="soft" color="gray" onClick={()=>{setError('');void desktop.refresh();}}>重新读取</Button></div>}
  {desktop.available&&<SessionRecovery onOpen={openRecord}/>} 
  {desktop.available&&<RetainedRecovery/>}
  {view==='home'&&desktop.onboarding&&!desktop.onboarding.progress.completedAt&&<div className="environment-banner"><div><strong>用两次口述，熟悉 SpeakMode</strong><p>设置服务、测试快捷键，在聊天与文档场景中练习。</p></div><Button variant="soft" onClick={openOnboarding}>{desktop.onboarding.progress.step==='welcome'?'开始使用引导':'继续使用引导'}</Button></div>}
  {view==='home'&&<HomePage preferences={preferences} records={homeRecords} defaultMode={modes.defaultMode} onDefaultModeChange={updateDefault} onOpen={openRecord} onHistory={()=>navigate('history')} onSettings={()=>navigate('settings')} onManageModes={()=>manageModes('home')}/>}
  {view==='history'&&<HistoryPage selectedId={selectedId} onSelect={setSelectedId}/>}
  {view==='dictionary'&&<DictionaryPage/>}
  {view==='settings'&&<SettingsPage preferences={preferences} onPreferencesChange={updatePreferences} defaultMode={modes.defaultMode} onDefaultModeChange={updateDefault} onManageModes={()=>manageModes('settings')} onOnboarding={openOnboarding}/>}
  {view==='modes'&&<ModesPage records={dictations.records.filter(r=>r.detail)} origin={modeOrigin==='home'?'首页':'设置'} onBack={()=>navigate(modeOrigin)} onDirtyChange={setModeDirty} leaving={pendingView!==null} onKeepEditing={()=>setPendingView(null)} onDiscardAndLeave={()=>pendingView&&commitNavigation(pendingView)}/>}
 </AppShell>;
}


