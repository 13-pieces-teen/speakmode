import {diffArrays} from 'diff';
import type { HistoryCandidate, HistoryDetail, HistorySummary, OutputRevision, CandidateReviewIssue } from './services/contracts';
export type BuiltinMode='自动'|'轻整理'|'结构';
// Personal IDs are opaque native values (currently UUIDs); preserve legacy IDs verbatim.
export type Mode=BuiltinMode|(string & {});
export type ResolvedMode='轻整理'|'结构';
export type DictationSourceState='unprocessed'|'source'|'fallback'|'model'|'unknown';
export interface Snapshot { current:string[]; mode:Mode; resolved:ResolvedMode|null; modified:boolean; change:string; modeName?:string; modeVersion?:number; modeInstructions?:string }
export interface Dictation extends Snapshot {
 id:string;title:string;date:string;group:string;time:string;source:string[];durationSec:number;revision:string;undoAvailable:boolean;
 sourceRevisionId:string;sourceState:DictationSourceState;detail?:HistoryDetail;previewOnly?:boolean;temporary?:boolean;status?:string;
}
export interface Candidate { native:OutputRevision;mode:Mode;resolved:ResolvedMode|null;texts:string[];baseRevision:string;reviewIssues:CandidateReviewIssue[];modeName?:string;modeVersion?:number;modeInstructions?:string }
export interface DiffSegment {text:string;changed:boolean}
export const toModeId=(mode:Mode):string=>mode==='自动'?'auto':mode==='轻整理'?'light':mode==='结构'?'structure':mode;
export const fromModeId=(id:string):Mode=>id==='auto'?'自动':id==='light'?'轻整理':id==='structure'?'结构':id;
export const resolvedLabel=(id:string|undefined|null):ResolvedMode|null=>id==='structure'?'结构':id==='light'?'轻整理':null;
function dateFields(endedAt:string,date:string) { const today=new Date().toLocaleDateString('en-CA');const parsed=new Date(endedAt);return {date,group:date===today?'今天':date,time:Number.isNaN(parsed.getTime())?'':parsed.toLocaleTimeString('zh-CN',{hour:'2-digit',minute:'2-digit'})}; }
// Summaries omit output kind/reason; a stored resolvedMode alone cannot prove a model ran.
export function fromSummary(record:HistorySummary):Dictation {return {id:record.sessionId,title:record.title||'口述记录',...dateFields(record.endedAt,record.endLocalDate),source:[],current:record.preview?[record.preview]:[],mode:fromModeId(record.modeId),modeName:record.modeName,resolved:record.currentRevisionId&&!record.rewriteFallback?resolvedLabel(record.resolvedMode):null,durationSec:Math.round(record.captureDurationMs/1000),revision:record.currentRevisionId??'',sourceRevisionId:record.sourceRevisionId,sourceState:record.rewriteFallback?'fallback':record.currentRevisionId?'unknown':'unprocessed',modified:false,change:'',undoAvailable:false,previewOnly:true,status:record.status};}
export function fromDetail(detail:HistoryDetail,temporary=false):Dictation {
 const {session,source,output}=detail;const snapshot=output?.modeSnapshot??session.modeSnapshot;
 const current=output?.blocks.map(b=>b.text)??source.items.map(i=>i.text);
 const sourceState:DictationSourceState=!output?'unprocessed':output.rewriteFallback?'fallback':output.kind==='useSource'||output.creationReason==='useSource'?'source':output.kind==='initial'||output.kind==='adopt'?'model':'unknown';
 return {id:session.id,title:current.join(' ').slice(0,48)||'口述记录',...dateFields(session.endedAt,session.endLocalDate),source:source.items.map(i=>i.text),current,mode:fromModeId(snapshot.modeId),modeName:snapshot.name,resolved:sourceState==='unprocessed'||sourceState==='source'||sourceState==='fallback'?null:resolvedLabel(output?.resolvedMode),modeVersion:snapshot.revision,modeInstructions:snapshot.rules,durationSec:Math.round(session.captureDurationMs/1000),revision:output?.id??'',sourceRevisionId:source.id,sourceState,modified:!!output&&output.kind!=='initial',change:output?.rewriteFallback?'已使用识别稿':output?({initial:'初始版本',useSource:'采用识别稿',adopt:'采用了新整理版本',edit:'已保存手动修改',restore:'恢复了所选内容',undo:'已撤销上次修改'}[output.kind]):'尚无整理结果',undoAvailable:detail.canUndo,detail,temporary,status:session.status};
}
export function fromCandidate(native:HistoryCandidate):Candidate {return {native,mode:fromModeId(native.modeSnapshot.modeId),resolved:resolvedLabel(native.resolvedMode),texts:native.blocks.map(b=>b.text),baseRevision:native.baseRevisionId??'',reviewIssues:native.reviewIssues??[],modeName:native.modeSnapshot.name,modeVersion:native.modeSnapshot.revision,modeInstructions:native.modeSnapshot.rules};}
export function modeLabel(record:Pick<Dictation,'mode'|'resolved'|'modeName'|'sourceState'>) {
 if(record.sourceState==='unprocessed')return '未整理';
 if(record.sourceState==='source'||record.sourceState==='fallback')return '识别稿';
 return record.mode==='自动'&&record.resolved&&record.sourceState==='model'?`自动 → ${record.resolved}`:record.modeName??record.mode;
}
export function serializeRecord(record:Dictation) {return record.current.join('\n\n');}
// Fine-grained highlighting is optional. Keep the previous LCS input-product
// guard, and bound asymmetric inputs before allocating Unicode-token arrays.
const DIFF_MAX_SCALARS=4096,DIFF_MAX_INPUT_PRODUCT=250000;
const DIFF_MAX_EDIT_LENGTH=512,DIFF_TIMEOUT_MS=16;
export function characterDiff(source:string,target:string):{source:DiffSegment[];current:DiffSegment[];granularity:'characters'|'whole'|'unchanged'} {
 const whole=()=>({source:source?[{text:source,changed:source!==target}]:[],current:target?[{text:target,changed:source!==target}]:[],granularity:source===target?'unchanged' as const:'whole' as const});
 if(source===target||!source||!target)return whole();
 // A scalar occupies at most two UTF-16 code units. Reject obviously oversized
 // inputs before Array.from; the exact scalar limit is checked immediately after.
 if(source.length>DIFF_MAX_SCALARS*2||target.length>DIFF_MAX_SCALARS*2)return whole();
 const left=Array.from(source),right=Array.from(target);
 if(left.length>DIFF_MAX_SCALARS||right.length>DIFF_MAX_SCALARS||left.length*right.length>DIFF_MAX_INPUT_PRODUCT)return whole();
 // jsdiff's timeout is cooperative, not a hard wall-clock deadline. The input
 // guards and edit budget remain deterministic even if a timer check is late.
 const changes=diffArrays(left,right,{maxEditLength:DIFF_MAX_EDIT_LENGTH,timeout:DIFF_TIMEOUT_MS});
 if(!changes)return whole();
 const a:DiffSegment[]=[],b:DiffSegment[]=[];
 const add=(list:DiffSegment[],text:string,changed:boolean)=>{if(!text)return;const last=list.at(-1);if(last?.changed===changed)last.text+=text;else list.push({text,changed});};
 for(const change of changes){const text=change.value.join('');if(!change.added)add(a,text,!!change.removed);if(!change.removed)add(b,text,!!change.added);}
 // Visual correspondence only: never infer sourceRefs or RestorePatch ranges
 // from these segments. Restoration continues to use explicit source/target IDs,
 // user-selected scalar ranges, and the native revision checks.
 return {source:a,current:b,granularity:'characters'};
}
