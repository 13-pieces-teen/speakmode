import {useEffect,useId,useRef,useState} from 'react';
import {AlertDialog,Button} from '@radix-ui/themes';
import {UpdateIcon} from '@radix-ui/react-icons';
import {useDesktop} from '../services/DesktopProvider';
import type {AppUpdateStatus} from '../services/contracts';
import {errorMessage} from '../services/bridge';
import {useNavigationGuard} from './NavigationGuard';
import {SettingsSection,SettingRow} from './SettingsControls';
import {focusInView} from './focusInView';

const activeStates=new Set<AppUpdateStatus['state']>(['checking','downloading','installing']);
const statusLabels:Record<AppUpdateStatus['state'],string>={disabled:'此版本暂不支持应用内更新。',idle:'可以手动检查新版本。',checking:'正在检查更新…',upToDate:'已是最新版本。',available:'有新版本可下载。',downloading:'正在下载更新…',downloaded:'更新已下载，确认后即可安装。',installing:'正在启动安装程序…',error:'更新未完成，请重试。'};
const bytes=(value:number)=>`${(Math.max(0,value)/1024/1024).toFixed(1)} MB`;

export function UpdateSettings({installBlocked}: {installBlocked?:string}) {
 const {available,command}=useDesktop(),navigation=useNavigationGuard();
 const [status,setStatus]=useState<AppUpdateStatus|null>(null),[error,setError]=useState(''),[readError,setReadError]=useState(''),[operation,setOperation]=useState<'check'|'download'|'install'|null>(null),[confirmVersion,setConfirmVersion]=useState<string|null>(null);
 const statusRef=useRef(status),blockedRef=useRef(installBlocked),mounted=useRef(false),operationRef=useRef<'check'|'download'|'install'|null>(null),epoch=useRef(0),refreshStatus=useRef<(()=>void)|null>(null),installTrigger=useRef<HTMLButtonElement>(null);
 statusRef.current=status;blockedRef.current=installBlocked;
 const statusId=useId();

 useEffect(()=>{
  if(!available)return;
  let disposed=false,polling=false,wake=false,timer:ReturnType<typeof setTimeout>|undefined;
  mounted.current=true;
  async function poll(){
   if(polling){wake=true;return;}
   polling=true;
   const requestEpoch=epoch.current;
   try{
    const next=await command<AppUpdateStatus>('update.status');
    if(!disposed&&requestEpoch===epoch.current){statusRef.current=next;setStatus(next);setReadError('');}
   }catch(cause){if(!disposed&&requestEpoch===epoch.current&&!operationRef.current)setReadError(errorMessage(cause));}
   polling=false;
   if(!disposed){const delay=wake?0:operationRef.current||activeStates.has(statusRef.current?.state??'idle')?750:30000;wake=false;timer=setTimeout(()=>void poll(),delay);}
  }
  refreshStatus.current=()=>{if(timer)clearTimeout(timer);void poll();};
  void poll();
  return()=>{disposed=true;mounted.current=false;refreshStatus.current=null;epoch.current++;if(timer)clearTimeout(timer);};
 },[available,command]);

 async function perform(kind:'check'|'download'|'install',version?:string){
  if(!available||!mounted.current||operationRef.current)return;
  if(kind!=='check'&&(!version||statusRef.current?.version!==version))return;
  if(kind==='install'&&(blockedRef.current||statusRef.current?.state!=='downloaded'))return;
  const requestEpoch=++epoch.current;
  operationRef.current=kind;setOperation(kind);setError('');setReadError('');
  if(statusRef.current){statusRef.current={...statusRef.current,state:kind==='check'?'checking':kind==='download'?'downloading':'installing',message:undefined};setStatus(statusRef.current);}
  try{
   const pending=command<AppUpdateStatus>(`update.${kind}`,kind==='check'?{}:{version});refreshStatus.current?.();
   const next=await pending;
   if(mounted.current&&requestEpoch===epoch.current){epoch.current++;statusRef.current=next;setStatus(next);setError('');}
  }catch(cause){
   if(mounted.current&&requestEpoch===epoch.current){epoch.current++;if(statusRef.current){statusRef.current={...statusRef.current,state:'error',message:undefined};setStatus(statusRef.current);}setError(errorMessage(cause));}
  }finally{operationRef.current=null;if(mounted.current)setOperation(null);}
 }
 function requestInstall(){
  navigation.request(()=>{
   const current=statusRef.current;
   if(mounted.current&&!operationRef.current&&!blockedRef.current&&current?.version&&current.state==='downloaded')setConfirmVersion(current.version);
  });
 }
 function confirmInstall(){
  const version=confirmVersion;
  if(!version)return;
  if(statusRef.current?.version!==version||statusRef.current.state!=='downloaded'){setConfirmVersion(null);setError('更新版本或下载状态已变化，请重新确认安装。');return;}
  navigation.request(()=>{if(mounted.current&&!blockedRef.current){setConfirmVersion(null);void perform('install',version);}});
 }

 const state=status?.state,busy=operation!==null||(state!==undefined&&activeStates.has(state)),canDownload=Boolean(status?.version&&(state==='available'||state==='error')),canInstall=Boolean(status?.version&&state==='downloaded');
 const downloaded=Number.isFinite(status?.downloadedBytes)?Math.max(0,status!.downloadedBytes!):0;
 const total=Number.isFinite(status?.totalBytes)&&status!.totalBytes!>0?status!.totalBytes!:undefined;
 const progress=total?Math.min(100,Math.round(downloaded/total*100)):undefined;
 const message=error||readError||(state==='error'?status?.message||statusLabels.error:status?.message);
 const hasError=Boolean(error||readError||state==='error');
 const label=operation==='check'&&state!=='checking'?'正在检查更新…':operation==='download'&&state!=='downloading'?'正在准备下载…':operation==='install'&&state!=='installing'?'正在准备安装…':state?statusLabels[state]:'正在读取版本信息…';
 return <SettingsSection title="应用更新" icon={<UpdateIcon/>} className="settings-update">
  <SettingRow title="SpeakMode" description={status?.currentVersion?`当前版本 ${status.currentVersion}`:available?'正在读取当前版本…':'请在 SpeakMode 桌面应用中查看版本和安装更新。'}>
   <div className="settings-update-actions"><Button variant="soft" color="gray" aria-describedby={available?statusId:undefined} disabled={!available||busy||state==='disabled'} loading={operation==='check'||state==='checking'} onClick={()=>void perform('check')}>{hasError?'重新检查':'检查更新'}</Button>
    {canDownload&&<Button disabled={busy} onClick={()=>void perform('download',status?.version)}>{state==='error'?'重试下载':'下载更新'}</Button>}
    {canInstall&&<Button ref={installTrigger} disabled={busy||Boolean(installBlocked)} onClick={requestInstall}>安装并重启</Button>}</div>
  </SettingRow>
  {available&&<div className="settings-update-status" id={statusId}><p role="status" aria-live="polite" aria-atomic="true">{label}</p>{status?.version&&<p>可用版本 {status.version}</p>}{(state==='downloading'||operation==='download')&&<><progress aria-label="更新下载进度" max={100} value={progress}/><p>{total?`${bytes(downloaded)} / ${bytes(total)}（${progress}%）`:downloaded?`已下载 ${bytes(downloaded)}`:'正在等待下载进度…'}</p></>}</div>}
  {message&&<p className={hasError?'settings-error':'settings-control-hint'} role={hasError?'alert':'status'}>{message}</p>}
  {canInstall&&installBlocked&&<p className="settings-control-hint">{installBlocked}</p>}
  {status?.version&&status.notes&&<details className="settings-update-notes"><summary>版本 {status.version} 更新说明</summary><p tabIndex={0}>{status.notes}</p></details>}
  <AlertDialog.Root open={confirmVersion!==null} onOpenChange={open=>{if(!open)setConfirmVersion(null);}}><AlertDialog.Content maxWidth="460px" onCloseAutoFocus={event=>{event.preventDefault();focusInView(installTrigger.current);}}>
   <AlertDialog.Title>安装 SpeakMode {confirmVersion}？</AlertDialog.Title><AlertDialog.Description>将关闭 SpeakMode 并启动安装程序，安装完成后重新打开。请先结束口述并保存修改。</AlertDialog.Description>
   {installBlocked&&<p className="settings-error" role="alert">{installBlocked}</p>}
   <div className="settings-update-confirm-actions"><AlertDialog.Cancel><Button variant="soft" color="gray">暂不安装</Button></AlertDialog.Cancel><Button disabled={busy||Boolean(installBlocked)} onClick={confirmInstall}>确认安装并重启</Button></div>
  </AlertDialog.Content></AlertDialog.Root>
 </SettingsSection>;
}
