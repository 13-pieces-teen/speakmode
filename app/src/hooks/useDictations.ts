import {useDesktop} from '../services/DesktopProvider';
import type {HistoryDetail,HistoryCandidate,OutputBlock,RestorePatch,CorrectionPatch,CorrectionReceipt} from '../services/contracts';
import {fromCandidate,type Candidate,type Dictation,toModeId,type Mode} from '../model';
export function useDictations(){
 const desktop=useDesktop();
 const changed=async(name:string,payload:Record<string,unknown>)=>{const detail=await desktop.command<HistoryDetail>(name,payload);desktop.acceptDetail(detail);void desktop.refresh().catch(()=>{});return detail;};
 return {
  records:desktop.records,
  load:desktop.loadRecord,
  generate:async(record:Dictation,mode:Mode,runId:string):Promise<Candidate>=>fromCandidate(await desktop.command<HistoryCandidate>('history.rewrite',{sessionId:record.id,sourceRevisionId:record.sourceRevisionId,modeId:toModeId(mode),baseRevisionId:record.revision,runId})),
  cancelCandidate:async(id:string,runId:string|null|undefined)=>{if(!runId)return;await desktop.command('history.cancelCandidate',{sessionId:id,runId});},
  adopt:(record:Dictation,candidate:Candidate,reviewedText:string)=>changed('revision.adopt',{sessionId:record.id,expectedCurrentRevisionId:record.revision,candidateId:candidate.native.id,reviewedText}),
  edit:(record:Dictation,blocks:OutputBlock[])=>changed('revision.edit',{sessionId:record.id,expectedCurrentRevisionId:record.revision,blocks}),
  restore:(record:Dictation,patch:RestorePatch)=>changed('revision.restore',{sessionId:record.id,expectedCurrentRevisionId:record.revision,patch}),
  correctTerm:async(record:Dictation,patch:CorrectionPatch,remember:boolean,expectedDictionaryVersion?:number)=>{
   const receipt=await desktop.command<CorrectionReceipt>('revision.correctTerm',{sessionId:record.id,expectedCurrentRevisionId:record.revision,patch,remember,...(remember?{expectedDictionaryVersion}:{})});
   // The single native receipt is authoritative for both changes. A later list
   // refresh failure must not make a committed correction appear unsaved.
   desktop.acceptDetail(receipt.detail);if(receipt.dictionary)desktop.acceptDictionary(receipt.dictionary);
   void desktop.refresh().catch(()=>{});return receipt;
  },
  undo:(record:Dictation)=>changed('revision.undo',{sessionId:record.id,expectedCurrentRevisionId:record.revision}),
  remove:async(id:string)=>{await desktop.command('history.delete',{sessionId:id});await desktop.refresh();}
 };
}

