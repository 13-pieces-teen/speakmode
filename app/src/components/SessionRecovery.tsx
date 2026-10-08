import {useEffect,useRef,useState} from 'react';
import {Button} from '@radix-ui/themes';
import {useDesktop} from '../services/DesktopProvider';
import {errorMessage} from '../services/bridge';
import {deliveryFeedback} from '../services/deliveryFeedback';
export function SessionRecovery({onOpen}:{onOpen:(id:string)=>void}) {
 const desktop=useDesktop(),s=desktop.session;
 const [busy,setBusy]=useState(''),[error,setError]=useState(''),[notice,setNotice]=useState('');
 const identity=s?(s.sessionId??s.id)+':'+s.generation:'none';
 const current=useRef(identity),operation=useRef(false);current.current=identity;
 useEffect(()=>{setError('');setNotice('');},[identity]);
 if(!s)return null;
 const phase=s.state.toLowerCase(),ongoing=['starting','recording','finalizingasr','rewriting'].includes(phase),problem=['failed','incomplete'].includes(phase);
 const delivery=deliveryFeedback(s);
 if(!ongoing&&!problem&&!delivery)return null;
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
 const labels:Record<string,string>={starting:'正在启动口述',recording:'正在录音，停顿不会结束',finalizingasr:'正在收齐识别结果',rewriting:'正在整理',failed:'这次口述需要处理',incomplete:'这次识别未完整'};
 const retryDescriptions:Record<string,string>={asr:'可使用保留的完整音频重新识别；成功后先查看结果。',rewrite:'可沿用完整识别稿重新整理；成功后先查看结果。',save:'内容尚未保存成功，可重试本次保存；不会补发到其他应用。'};
 return <section className="session-recovery surface" aria-label="当前口述状态">
  <h2 className="section-title">{labels[phase]??delivery?.title??'交付结果'}</h2><p role={delivery?'status':undefined} aria-live={delivery?'polite':undefined}>{s.error??delivery?.message}</p>
  {problem&&s.partial&&<details><summary>查看已收到的识别文字</summary><p className="session-recovery-text">{s.partial}</p></details>}
  {s.workerReleased===false&&problem&&<p>正在释放音频资源，完成前请稍候。</p>}
  {problem&&s.retryKind&&retryDescriptions[s.retryKind]&&<p>{retryDescriptions[s.retryKind]}</p>}
  <div className="session-recovery-actions">
   {ongoing&&<Button variant="soft" color="gray" disabled={Boolean(busy)} onClick={()=>void act('session.cancel')}>取消本次口述</Button>}
   {(phase==='recording'||phase==='starting')&&<Button disabled={Boolean(busy)} onClick={()=>void act('session.stop')}>结束口述</Button>}
   {problem&&s.canCopyRecovery&&<Button variant="soft" disabled={Boolean(busy)} onClick={()=>void act('recoverable.copy')}>复制已有文字</Button>}
   {s.canUseSource&&<Button variant="soft" disabled={Boolean(busy)||!s.workerReleased} onClick={()=>void act('result.useSource')}>使用完整识别稿</Button>}
   {s.canRetryAsr&&<Button variant="soft" disabled={Boolean(busy)||!s.workerReleased} onClick={()=>void act('asr.retry')}>重试识别</Button>}
   {s.canRetryRewrite&&<Button variant="soft" disabled={Boolean(busy)||!s.workerReleased} onClick={()=>void act('rewrite.retry')}>重试整理</Button>}
   {s.canRetrySave&&s.pendingCommitId&&<Button variant="soft" disabled={Boolean(busy)||!s.workerReleased} onClick={()=>void act('result.retrySave')}>重试保存</Button>}
   {delivery?.canCopyOutput&&<Button disabled={Boolean(busy)} onClick={()=>void act('result.copy')}>复制成稿</Button>}
   {delivery?.canViewOutput&&<Button variant="soft" onClick={()=>onOpen(s.sessionId??s.id!)}>查看成稿</Button>}
   {problem&&<Button variant="ghost" color="gray" disabled={Boolean(busy)} onClick={()=>void act('session.cancel')}>放弃本次恢复</Button>}
  </div>
  {problem&&<p className="history-secondary">放弃后清除本次未完成的文字与音频缓存，不影响已经完成的口述记录。</p>}
  {busy&&<p role="status">正在处理…</p>}{notice&&<p role="status">{notice}</p>}{error&&<p role="alert" className="inline-error">{error}</p>}
 </section>;
}

