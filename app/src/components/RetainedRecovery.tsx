import {useRef,useState} from 'react';
import {Button} from '@radix-ui/themes';
import {useDesktop} from '../services/DesktopProvider';
import {errorMessage} from '../services/bridge';
import type {RetainedRecovery as Recovery} from '../services/contracts';

export function RetainedRecovery() {
 const desktop=useDesktop(),recovery=desktop.recovery;
 if(!recovery)return null;
 const session=recovery.session;
 return <RecoveryCard key={`${session.sessionId??session.id}:${session.generation}`} recovery={recovery}/>;
}

function RecoveryCard({recovery}:{recovery:Recovery}) {
 const desktop=useDesktop(),session=recovery.session,current=desktop.session;
 const [busy,setBusy]=useState(false),[error,setError]=useState(''),[confirmDiscard,setConfirmDiscard]=useState(false);
 const running=useRef(false);
 const processing=!!current&&(['starting','recording','finalizingasr','rewriting'].includes(current.state.toLowerCase())||current.workerReleased===false||['prepared','dispatched'].includes(current.deliveryState??''));
 const unresolved=!!current&&['failed','incomplete'].includes(current.state.toLowerCase())&&!current.canRetryAsr&&Boolean(current.pendingCommitId||current.sourceRevisionId||current.canCopyRecovery||current.canUseSource||current.canRetrySave||current.canRetryRewrite||current.partial?.trim());
 const deadline=new Date(recovery.expiresAt);
 const expiry=Number.isNaN(deadline.getTime())?'原保留期限内':deadline.toLocaleTimeString('zh-CN',{hour:'2-digit',minute:'2-digit'});
 async function act(name:'recovery.activate'|'recovery.discard') {
  if(running.current)return;
  running.current=true;setBusy(true);setError('');
  try {
   await desktop.command(name,{sessionId:session.sessionId??session.id,expectedGeneration:session.generation});
   await desktop.refresh();
  }catch(cause){setError(errorMessage(cause));}
  finally{running.current=false;setBusy(false);}
 }
 return <section className="session-recovery retained-recovery surface" aria-label="待恢复口述">
  <div className="retained-recovery-heading"><h2 className="section-title">上一条口述可恢复</h2><span>保留至 {expiry}</span></div>
  <p>完整音频暂存在本次运行的内存中，开始下一条口述不会立即丢失。打开后可重新识别，成功后先查看结果。</p>
  {session.partial&&<details><summary>查看已收到的文字</summary><p className="session-recovery-text">{session.partial}</p></details>}
  {processing&&<p>当前口述结束并释放资源后，可以打开恢复。</p>}
  {!processing&&unresolved&&<p>请先处理当前口述的保存或整理问题，再打开上一条。</p>}
  <div className="session-recovery-actions">
   <Button variant="soft" disabled={busy||processing||unresolved} onClick={()=>void act('recovery.activate')}>打开恢复</Button>
   {!confirmDiscard&&<Button variant="ghost" color="gray" disabled={busy} onClick={()=>setConfirmDiscard(true)}>放弃这条恢复</Button>}
  </div>
  {confirmDiscard&&<div className="retained-recovery-confirm">
   <p>确认清除这条未完成口述的文字和音频缓存？此操作无法撤销，不影响当前口述和已保存记录。</p>
   <div className="session-recovery-actions"><Button variant="soft" color="red" disabled={busy} onClick={()=>void act('recovery.discard')}>确认放弃</Button><Button variant="ghost" color="gray" disabled={busy} onClick={()=>setConfirmDiscard(false)}>保留恢复</Button></div>
  </div>}
  {busy&&<p role="status">正在处理…</p>}{error&&<p role="alert" className="inline-error">{error}</p>}
 </section>;
}
