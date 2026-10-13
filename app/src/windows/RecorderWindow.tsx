import {useCallback,useEffect,useRef,useState} from 'react';
import {Button,Popover} from '@radix-ui/themes';
import {CheckIcon,ChevronDownIcon,Cross2Icon} from '@radix-ui/react-icons';
import {useDesktop} from '../services/DesktopProvider';
import {useModes} from '../hooks/useModes';
import {ModeIcon} from '../components/ui';
import {fromModeId,toModeId,type Mode} from '../model';
import {builtinModes} from '../modes';
import {errorMessage} from '../services/bridge';
import {deliveryFeedback} from '../services/deliveryFeedback';
import {useRecorderSize} from './useRecorderSize';
import {CaptionStrip} from './CaptionStrip';
import {RecorderWaveform} from './RecorderWaveform';
import './recorder.css';
function focusModeChoice(button:HTMLButtonElement|undefined|null){
 if(!button)return;button.focus({preventScroll:true});
 const panel=button.closest<HTMLElement>('.recorder-mode-options');if(!panel)return;
 const item=button.getBoundingClientRect(),bounds=panel.getBoundingClientRect();
 // Scroll the bounded menu, never the transparent desktop window/document.
 if(item.top<bounds.top)panel.scrollTop+=item.top-bounds.top;
 else if(item.bottom>bounds.bottom)panel.scrollTop+=item.bottom-bounds.bottom;
}
export function RecorderWindow(){
 const desktop=useDesktop(),catalog=useModes(),session=desktop.session;
 const identity=session?`${session.sessionId??session.id}:${session.generation}`:'none';
 const identityRef=useRef(identity);identityRef.current=identity;
 const operation=useRef(0),handledMenuRequest=useRef(-1);
 const [menu,setMenu]=useState(false),[openedFromTray,setOpenedFromTray]=useState(false),[localError,setLocalError]=useState<{identity:string;message:string}|null>(null),[busyIdentity,setBusyIdentity]=useState<string|null>(null);
 const [copiedIdentity,setCopiedIdentity]=useState<string|null>(null);
 const error=localError?.identity===identity?localError.message:'',busy=busyIdentity===identity;
 const assembly=useRef<HTMLElement>(null);
 const [menuElement,setMenuElement]=useState<HTMLDivElement|null>(null);
 const focusedChoice=useRef<HTMLButtonElement|null>(null);
 const [menuMotion,setMenuMotion]=useState<'pointer'|'instant'>('instant');
 const phase=(session?.state??'idle').toLowerCase();const recording=phase==='recording',canStop=recording||phase==='starting',canCancel=['starting','recording','finalizingasr','rewriting','finalizing_asr','failed','incomplete'].includes(phase),problem=['failed','incomplete'].includes(phase);
 const delivery=deliveryFeedback(session),dismissResult=phase==='ready';
 const copied=copiedIdentity===identity;
 const canCopyRecovery=problem&&session?.canCopyRecovery===true;
 const canCopyReady=phase==='ready'&&session?.workerReleased===true&&(delivery?.canCopyOutput||!delivery);
 const menuOpen=menu&&canStop;
 const {error:resizeError,menuReady}=useRecorderSize(assembly,menuElement,menuOpen,desktop.available,desktop.command);
 const presentationError=error||session?.error||desktop.error||resizeError;
 const current=fromModeId(session?.modeSnapshot?.modeId??session?.modeId??'auto');
 const currentLabel=session?.modeSnapshot?.name??catalog.label(current);
 const reportCaptionRender=useCallback((token:string,asrAttemptId:string|null)=>{
  // One native-clock diagnostic proxy per attempt. No text, renderer timestamp or retry side effect.
  void desktop.command('session.captionRendered',{sessionId:session?.sessionId??session?.id,expectedGeneration:session?.generation,asrAttemptId,token}).catch(()=>{});
 },[desktop.command,session?.sessionId,session?.id,session?.generation]);
 const scenePhase=session?.writingScene==='chat'?'聊天整理':session?.writingScene==='document'?'文档整理':session?.writingScene==='technical'?'技术整理':'正在整理';
 const phaseLabel:Record<string,string>={idle:'等待开始',starting:'正在启动',recording:'正在录音',finalizingasr:'正在收齐文字',finalizing_asr:'正在收齐文字',rewriting:scenePhase,ready:delivery?.warning?'未自动输入':delivery?.title??'成稿已就绪',failed:'暂时无法继续',incomplete:'识别未完整',cancelled:'已取消',delivering:'正在交付'};
 useEffect(()=>{document.documentElement.dataset.surface='recorder';return()=>{delete document.documentElement.dataset.surface;};},[]);
 useEffect(()=>{setMenu(false);setOpenedFromTray(false);setLocalError(null);setBusyIdentity(null);setCopiedIdentity(null);},[identity]);
 useEffect(()=>{if(!canStop)setMenu(false);},[canStop]);
 useEffect(()=>{
  if(!menuReady||!menuElement)return;
  const previous=focusedChoice.current;
  focusModeChoice(previous&&menuElement.contains(previous)?previous:menuElement.querySelector<HTMLButtonElement>('button[aria-pressed="true"]'));
 },[menuReady,menuElement]);
 useEffect(()=>{const request=desktop.recorderModeRequest;if(!request||request.sequence<=handledMenuRequest.current)return;handledMenuRequest.current=request.sequence;if(!canStop||request.sessionId!==(session?.sessionId??session?.id)||request.generation!==session?.generation)return;setMenuMotion('instant');setOpenedFromTray(true);setMenu(true);},[desktop.recorderModeRequest,canStop,session]);
 // Closing a native Mode menu on app blur never stops or cancels recording.
 useEffect(()=>{if(!menuOpen)return;const closeMenu=(event:Event)=>{if(event.target===window)setMenu(false);};window.addEventListener('blur',closeMenu);return()=>window.removeEventListener('blur',closeMenu);},[menuOpen]);
 async function act(name:string,payload:Record<string,unknown>={}){
  const owner=identity,token=++operation.current;setBusyIdentity(owner);setLocalError(null);
  const currentOperation=()=>identityRef.current===owner&&operation.current===token;
  try{await desktop.command(name,{sessionId:session?.sessionId??session?.id,expectedGeneration:session?.generation,...payload});if(currentOperation()&&(name==='session.stop'||name==='session.cancel'||name==='session.selectMode'))setMenu(false);return true;}
  catch(e){if(currentOperation())setLocalError({identity:owner,message:errorMessage(e)});return false;}
  finally{if(currentOperation())setBusyIdentity(null);}
 }
 async function copyText(recovery=false){
  const owner=identity;
  const ok=await act(recovery?'recoverable.copy':'result.copy',recovery?{recoverableContentId:session?.recoverableContentId}:{});
  if(ok&&identityRef.current===owner)setCopiedIdentity(owner);
 }
 const options=[...builtinModes.map(m=>({id:m.id as Mode,name:m.id,caption:m.caption,revision:desktop.modes.find(n=>n.id===toModeId(m.id))?.revision??1})),...catalog.modes.map(m=>({id:m.id as Mode,name:m.name,caption:`个人 Mode · 基于${m.base}`,revision:m.version}))];
 return <main className="recorder-surface"><section ref={assembly} className="recorder-assembly" data-mode-open={menuOpen} aria-label="录音浮层">
  {!desktop.available&&<p className="recorder-environment" role="status">请在桌面应用中打开，浏览器不会录音或生成字幕。</p>}
  <div className="recorder-toolbar" role="group" aria-label="口述控制">
   <Popover.Root open={menuOpen} onOpenChange={open=>{setOpenedFromTray(false);setMenu(open);}}>
    <Popover.Trigger>
     <button className="recorder-mode-trigger" data-motion={menuMotion} disabled={!desktop.available||!canStop||busy} aria-label={`本次 Mode：${currentLabel}`}
      onPointerDown={()=>setMenuMotion('pointer')} onKeyDown={()=>setMenuMotion('instant')}>
      <ModeIcon mode={current}/><span>{currentLabel}</span><ChevronDownIcon/>
     </button>
    </Popover.Trigger>
    <Popover.Content ref={setMenuElement} avoidCollisions={false} side="top" align="start" sideOffset={17}
     className="recorder-mode-menu" data-motion={menuMotion} aria-label="选择本次 Mode"
     onFocusCapture={event=>{const target=event.target;if(target instanceof HTMLButtonElement&&target.classList.contains('recorder-mode-option'))focusedChoice.current=target;}}
     onOpenAutoFocus={event=>event.preventDefault()}>
     <div className="recorder-mode-heading">本次 Mode</div>
     {openedFromTray&&<p className="recorder-mode-notice" role="status">结束前请切回输入窗口，无法确认位置时会保留成稿。</p>}
     <div className="recorder-mode-options" role="group" aria-label="本次输出方式" onKeyDown={event=>{
      if(!["ArrowDown","ArrowUp","Home","End"].includes(event.key))return;
      event.preventDefault();
      const buttons=Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
      const index=buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next=event.key==="Home"?0:event.key==="End"?buttons.length-1:(index+(event.key==="ArrowDown"?1:-1)+buttons.length)%buttons.length;
      focusModeChoice(buttons[next]);
     }}>
      {options.map(option=><button key={option.id} type="button" className="recorder-mode-option"
       aria-pressed={option.id===current} title={`${option.name} · ${option.caption}`} disabled={busy}
       onClick={()=>void act('session.selectMode',{modeId:toModeId(option.id),revision:option.revision})}>
       <ModeIcon mode={option.id}/><span>{option.name}</span>{option.id===current&&<CheckIcon/>}
      </button>)}
     </div>
    </Popover.Content>
   </Popover.Root>
   <span className="recorder-divider"/><div className="recorder-level" aria-label={phaseLabel[phase]??phase}>{recording?<><RecorderWaveform key={identity} level={session?.level??0} receivedAt={session?.levelReceivedAt}/><span className="sr-only">正在录音。波形表示麦克风音量，静音不会自动结束。</span></>:<span className="recorder-phase" role="status">{phaseLabel[phase]??phase}</span>}</div>
   <button className="recorder-action" aria-label={dismissResult?"关闭成稿提示":problem?"放弃本次恢复":"取消本次口述"} title={dismissResult?"关闭提示，成稿仍保留":problem?"放弃本次未完成的文字与音频缓存":"取消本次口述"} disabled={!desktop.available||(!canCancel&&!dismissResult)||busy} onClick={()=>void act('session.cancel')}><Cross2Icon/></button><button className="recorder-action recorder-finish" aria-label="结束本次口述" disabled={!desktop.available||!canStop||busy} onClick={()=>void act('session.stop')}><CheckIcon/></button>
  </div>
  <CaptionStrip key={identity} identity={identity} session={session} reportRender={reportCaptionRender}/>
  {canStop&&session?.recordingLimitRemainingMs!=null&&<p className="recorder-limit-warning" role="status" aria-live="polite">本次录音还可继续 {Math.ceil(session.recordingLimitRemainingMs/1000)} 秒，请结束本次录音并另起一段。</p>}
  {presentationError&&<div className="recorder-message" role="alert"><p>{presentationError}</p>{(canCopyRecovery||canCopyReady)&&<Button size="1" variant="soft" disabled={!desktop.available||busy} onClick={()=>void copyText(canCopyRecovery)}>{copied?'已复制':canCopyRecovery?'复制已识别文字':'复制文本'}</Button>}</div>}
  {delivery&&!presentationError&&<div className="recorder-delivery" data-warning={delivery.warning} role="status" aria-live="polite"><p>{copied?'已复制，可回到输入框粘贴。':delivery.message}</p>{delivery.canCopyOutput&&<Button size="1" variant="soft" disabled={!desktop.available||busy||!canCopyReady} onClick={()=>void copyText()}>{copied?'已复制':'复制文本'}</Button>}</div>}
  {!presentationError&&!delivery&&(canCopyReady||canCopyRecovery)&&<div className="recorder-delivery" role="status" aria-live="polite"><p>{copied?'已复制，可回到输入框粘贴。':canCopyRecovery?'已有文字可直接复制。':session?.rewriteFallback?'已使用识别稿。':'成稿已保留。'}</p><Button size="1" variant="soft" disabled={!desktop.available||busy} onClick={()=>void copyText(canCopyRecovery)}>{copied?'已复制':canCopyRecovery?'复制已识别文字':'复制文本'}</Button></div>}
 </section></main>;
}

