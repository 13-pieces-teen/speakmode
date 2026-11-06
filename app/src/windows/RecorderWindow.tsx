import {useCallback,useEffect,useRef,useState} from 'react';
import {Button} from '@radix-ui/themes';
import {Cross2Icon,ExclamationTriangleIcon,StopIcon} from '@radix-ui/react-icons';
import {useDesktop} from '../services/DesktopProvider';
import {errorMessage} from '../services/bridge';
import {recorderPresentation,type RecorderAction} from '../services/recorderPresentation';
import {useRecorderSize} from './useRecorderSize';
import {CaptionStrip} from './CaptionStrip';
import {RecorderWaveform} from './RecorderWaveform';
import {RecorderModeMenu} from './RecorderModeMenu';
import './recorder.css';

export function RecorderWindow(){
 const desktop=useDesktop(),session=desktop.session;
 const identity=session?(session.sessionId??session.id)+':'+session.generation:'none';
 const identityRef=useRef(identity);identityRef.current=identity;
 const operation=useRef(0),pendingOwner=useRef<string|null>(null);
 const [menu,setMenu]=useState(false),[localError,setLocalError]=useState<{identity:string;message:string}|null>(null);
 const [busyIdentity,setBusyIdentity]=useState<string|null>(null),[copiedIdentity,setCopiedIdentity]=useState<string|null>(null);
 const assembly=useRef<HTMLElement>(null),[menuElement,setMenuElement]=useState<HTMLDivElement|null>(null);
 const view=recorderPresentation(session,desktop.recorderCaptionsEnabled);
 const busy=busyIdentity===identity,copied=copiedIdentity===identity,menuOpen=menu&&view.canStop;
 const {error:resizeError,menuReady}=useRecorderSize(assembly,menuElement,menuOpen,desktop.available,desktop.command,identity);
 const localMessage=localError?.identity===identity?localError.message:'';
 const presentationError=localMessage||desktop.error||resizeError||session?.error||'';
 const attention=view.state==='attention'||!!presentationError;
 const details=attention||view.showCaptions||session?.recordingLimitRemainingMs!=null;
 useEffect(()=>{document.documentElement.dataset.surface='recorder';return()=>{delete document.documentElement.dataset.surface;};},[]);
 useEffect(()=>{setLocalError(null);setBusyIdentity(null);setCopiedIdentity(null);},[identity]);
 async function act(name:string,payload:Record<string,unknown>={}){
  if(pendingOwner.current===identity)return false;
  const owner=identity,token=++operation.current;pendingOwner.current=owner;setBusyIdentity(owner);setLocalError(null);
  const owns=()=>identityRef.current===owner&&operation.current===token;
  try{await desktop.command(name,{sessionId:session?.sessionId??session?.id,expectedGeneration:session?.generation,...payload});if(owns()&&(name==='session.stop'||name==='session.cancel'||name==='session.selectMode'))setMenu(false);return true;}
  catch(e){if(owns())setLocalError({identity:owner,message:errorMessage(e)});return false;}
  finally{if(operation.current===token)pendingOwner.current=null;if(owns())setBusyIdentity(null);}
 }
 async function perform(action:RecorderAction){
  if(action==='openResult'){await act('app.showMain',{currentResult:true});return;}
  const owner=identity,recovery=action==='copyRecovery';
  if(await act(recovery?'recoverable.copy':'result.copy',recovery?{recoverableContentId:session?.recoverableContentId}:{})&&identityRef.current===owner)setCopiedIdentity(owner);
 }
 const reportCaptionRender=useCallback((token:string,asrAttemptId:string|null)=>{
  void desktop.command('session.captionRendered',{sessionId:session?.sessionId??session?.id,expectedGeneration:session?.generation,asrAttemptId,token}).catch(()=>{});
 },[desktop.command,session?.sessionId,session?.id,session?.generation]);
 const actionLabel=view.primary==='copyRecovery'?'复制已有文字':view.primary==='copyOutput'?'复制文字':'查看恢复';
 return <main className="recorder-surface"><section ref={assembly} className="recorder-assembly" data-mode-open={menuOpen} data-details={details} aria-label="录音浮层">
  {!desktop.available&&<p className="recorder-environment" role="status">请在桌面应用中打开，浏览器不会录音或生成字幕。</p>}
  <div className="recorder-toolbar" data-state={attention?'attention':view.state} role="group" aria-label="口述控制">
   <RecorderModeMenu identity={identity} canSelect={view.canStop} busy={busy} open={menuOpen} onOpenChange={setMenu} element={menuElement} setElement={setMenuElement} ready={menuReady} onSelect={(modeId,revision)=>{setMenu(false);void act('session.selectMode',{modeId,revision});}}/>
   <button className="recorder-action" aria-label={view.close==='dismiss'?'收起提示':'取消本次口述'} title={view.close==='dismiss'?'收起提示，保留已有内容':'取消本次口述'} disabled={!desktop.available||!view.close||busy} onClick={()=>void act(view.close==='dismiss'?'window.dismissRecorder':'session.cancel')}><Cross2Icon/></button>
   <div className="recorder-level" role="status" aria-live="polite" aria-atomic="true">
    <div className="recorder-signal" aria-hidden="true">
     {view.waveform?<RecorderWaveform key={identity} level={session?.level??0} receivedAt={session?.levelReceivedAt}/>:attention?<ExclamationTriangleIcon/>:view.state==='processing'||view.canStop?<span className="recorder-progress"/>:<span className="recorder-resting"/>}
    </div>
    <span className="recorder-phase" title={view.label}>{view.label}</span>
   </div>
   <div className="recorder-action-slot">{view.canStop&&<button className="recorder-action recorder-finish" aria-label="结束本次口述" title="结束并整理" disabled={!desktop.available||busy} onClick={()=>void act('session.stop')}><StopIcon/></button>}</div>
  </div>
  {view.showCaptions&&<CaptionStrip key={identity} identity={identity} session={session} reportRender={reportCaptionRender}/>}
  {view.canStop&&session?.recordingLimitRemainingMs!=null&&<p className="recorder-limit-warning" role="status">本次录音还可继续 {Math.ceil(session.recordingLimitRemainingMs/1000)} 秒，请结束后另起一段。</p>}
  {attention&&<div className="recorder-recovery" role={presentationError?'alert':'status'} aria-live="polite">
   <p>{presentationError||view.message}</p>
   {copied&&<p className="recorder-copy-notice">已复制，可回到输入框粘贴。</p>}
   <div className="recorder-recovery-actions">
    {(view.showResult||presentationError)&&view.primary!=='openResult'&&<Button size="1" variant="ghost" color="gray" disabled={!desktop.available||busy} onClick={()=>void perform('openResult')}>查看结果</Button>}
    {view.primary&&<Button size="1" variant="soft" disabled={!desktop.available||busy} onClick={()=>void perform(view.primary!)}>{copied&&view.primary!=='openResult'?'已复制':actionLabel}</Button>}
   </div>
  </div>}
 </section></main>;
}

