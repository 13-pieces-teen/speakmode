import type {LiveSession} from './contracts';
import {deliveryFeedback} from './deliveryFeedback';

export type RecorderAction = 'copyOutput'|'copyRecovery'|'openResult';
export interface RecorderPresentation {
 state:'idle'|'listening'|'processing'|'settled'|'attention';
 label:string;
 message:string;
 waveform:boolean;
 canStop:boolean;
 close:'cancel'|'dismiss'|null;
 primary:RecorderAction|null;
 showResult:boolean;
 showCaptions:boolean;
}

/** Presentation only. Native session identity, capabilities and receipts remain authoritative. */
export function recorderPresentation(session:LiveSession|null,captionsEnabled=false):RecorderPresentation {
 const phase=(session?.state??'idle').toLowerCase();
 const view:RecorderPresentation={state:'idle',label:'等待开始',message:'',waveform:false,canStop:false,close:null,primary:null,showResult:false,showCaptions:false};
 if(!session)return view;
 const starting=phase==='starting',recording=phase==='recording';
 const processing=['finalizingasr','finalizing_asr','rewriting','delivering'].includes(phase);
 if(starting||recording||processing){
  // Native startup also waits for ASR readiness. Microphone feedback starts
  // as soon as real samples arrive, including while the provider connects.
  const listening=(starting||recording)&&session.captureReady===true;
  return {...view,state:starting||recording?'listening':'processing',label:listening?'正在聆听':starting||recording?'准备录音':'正在转成文字',
   waveform:listening,canStop:starting||recording,close:phase==='delivering'?null:'cancel',
   showCaptions:(captionsEnabled||!!session.onboardingLesson)&&!session.rewriteFallback&&!starting};
 }
 if(phase==='failed'||phase==='incomplete')return {...view,state:'attention',label:phase==='incomplete'?'识别未完整':'这次口述需要处理',
  message:session.error??session.message??'已有内容仍可恢复。',close:'dismiss',showResult:true,
  primary:session.canCopyRecovery===true?'copyRecovery':'openResult'};
 if(phase==='ready'){
  const delivery=deliveryFeedback(session);
  if(['prepared','dispatched'].includes(session.deliveryState??'')||session.workerReleased!==true)
   return {...view,state:'processing',label:'正在完成',message:session.error??'',showCaptions:false};
  if(delivery?.warning||session.error||session.rewriteFallback){
   const canCopy=delivery?.canCopyOutput||!!session.error||!delivery;
   return {...view,state:'attention',label:session.error?'结果需要核对':delivery?.title??'识别原文已保留',
    message:session.error??delivery?.message??'整理未完成，完整识别原文仍可使用。',close:'dismiss',showResult:true,
    primary:canCopy?'copyOutput':'openResult'};
  }
  // A normal unconfirmed dispatch is quiet, never a claim that the host received text.
  return {...view,state:'settled',label:session.deliveryState==='confirmed'?'已填入':session.deliveryState==='unconfirmed'?'口述已结束':'文字已保留',close:'dismiss'};
 }
 if(phase==='cancelled')return {...view,state:'settled',label:'已取消',close:'dismiss'};
 return view;
}
