import {useEffect,useRef,useState} from 'react';
import {Button} from '@radix-ui/themes';
import {useDesktop} from '../services/DesktopProvider';
import {errorMessage} from '../services/bridge';
import {recorderPresentation} from '../services/recorderPresentation';
export function SessionRecovery({onOpen}:{onOpen:(id:string)=>void}) {
 const desktop=useDesktop(),s=desktop.session;
 const [busy,setBusy]=useState(''),[error,setError]=useState(''),[notice,setNotice]=useState('');
 const identity=s?(s.sessionId??s.id)+':'+s.generation:'none';
 const current=useRef(identity),operation=useRef(false);current.current=identity;
 useEffect(()=>{setError('');setNotice('');},[identity]);
 if(!s)return null;
 const phase=s.state.toLowerCase(),ongoing=['starting','recording','finalizingasr','rewriting'].includes(phase),problem=['failed','incomplete'].includes(phase);
 const view=recorderPresentation(s);
 // A normal dispatch receipt is internal bookkeeping, not a recovery task.
 // Keep genuine delivery/storage problems actionable without claiming receipt
 // by the target app or issuing another paste/navigation side effect.
 const needsAttention=view.state==='attention'||(phase==='ready'&&Boolean(s.error));
 if(!ongoing&&!problem&&!needsAttention)return null;
 async function act(name:string){
  if(operation.current||!s)return;operation.current=true;setBusy(name);setError('');setNotice('');
  const expectedIdentity=identity;
  try {
   await desktop.command(name,{sessionId:s.sessionId??s.id,expectedGeneration:s.generation,recoverableContentId:s.recoverableContentId,pendingCommitId:s.pendingCommitId,failedAttemptId:s.asrAttemptId,sourceRevisionId:s.sourceRevisionId});
   await desktop.refresh();
   if(current.current===expectedIdentity&&name==='recoverable.copy')setNotice('已复制可恢复文本，请核对完整性。');
   if(current.current===expectedIdentity&&name==='result.copy')setNotice('成稿已复制，可切回输入位置粘贴。');
  }catch(e){if(current.current===expectedIdentity)setError(errorMessage(e));}
  finally{operation.current=false;setBusy('');}
 }
 const retryDescriptions:Record<string,string>={asr:'可使用保留的完整音频重新识别；成功后先查看结果。',rewrite:'可沿用完整识别稿重新整理；成功后先查看结果。',save:'内容尚未保存成功，可重试本次保存；不会补发到其他应用。'};
 return <section className="session-recovery surface" aria-label="当前口述状态">
  <h2 className="section-title">{view.label}</h2>{(s.error||view.message)&&<p role="status" aria-live="polite">{s.error||view.message}</p>}
  <div className="session-recovery-actions">
   {problem&&<Button disabled={Boolean(busy)||s.workerReleased!==true} onClick={()=>void act('session.start')}>开始下一条口述</Button>}
   {ongoing&&<Button variant="soft" color="gray" disabled={Boolean(busy)} onClick={()=>void act('session.cancel')}>取消本次口述</Button>}
   {view.canStop&&<Button disabled={Boolean(busy)} onClick={()=>void act('session.stop')}>结束口述</Button>}
   {s.canRetryRewrite&&<Button variant="soft" disabled={Boolean(busy)||!s.workerReleased} onClick={()=>void act('rewrite.retry')}>重试整理</Button>}
   {s.canRetryAsr&&<Button variant="soft" disabled={Boolean(busy)||!s.workerReleased} onClick={()=>void act('asr.retry')}>重试识别</Button>}
   {s.canRetrySave&&s.pendingCommitId&&<Button disabled={Boolean(busy)||!s.workerReleased} onClick={()=>void act('result.retrySave')}>重试保存</Button>}
   {view.primary==='copyRecovery'&&<Button variant="soft" disabled={Boolean(busy)} onClick={()=>void act('recoverable.copy')}>复制已有文字</Button>}
   {view.primary==='copyOutput'&&<Button disabled={Boolean(busy)} onClick={()=>void act('result.copy')}>复制成稿</Button>}
   {phase==='ready'&&view.showResult&&<Button variant="soft" onClick={()=>onOpen(s.sessionId??s.id!)}>查看成稿</Button>}
  </div>
  {problem&&<p>{s.workerReleased===true?'可以继续录音，已有内容会保留在待恢复列表中。':'正在释放本次录音资源，完成后即可继续录音。'}</p>}
  {problem&&<details className="session-recovery-more"><summary>更多恢复方式</summary>
   {s.partial&&<p className="session-recovery-text">{s.partial}</p>}
   {s.workerReleased===false&&<p>正在释放音频资源，完成前请稍候。</p>}
   {s.retryKind&&retryDescriptions[s.retryKind]&&<p>{retryDescriptions[s.retryKind]}</p>}
   <div className="session-recovery-actions">
    {s.canUseSource&&<Button variant="soft" disabled={Boolean(busy)||!s.workerReleased} onClick={()=>void act('result.useSource')}>使用完整识别稿</Button>}
    <Button variant="ghost" color="gray" disabled={Boolean(busy)} onClick={()=>void act('session.cancel')}>放弃本次恢复</Button>
   </div>
   <p className="history-secondary">放弃后清除本次未完成的文字与音频缓存，不影响已经完成的口述记录。</p>
  </details>}
  {busy&&<p role="status">正在处理…</p>}{notice&&<p role="status">{notice}</p>}{error&&<p role="alert" className="inline-error">{error}</p>}
 </section>;
}

