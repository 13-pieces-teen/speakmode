import type { OnboardingState } from '../onboarding/types';
import {createContext,useCallback,useContext,useEffect,useRef,useState,type ReactNode} from 'react';
import {desktopAvailable,desktopCommand,errorMessage,subscribeDesktop} from './bridge';
import {initialPreferences,initialProvider,type AppEvent,type AudioDevice,type DailyUsage,type DesktopError,type RecorderModeRequest,type HistoryDetail,type HistoryPage,type LiveSession,type ModeRecord,type NativePreferences,type NativeSnapshot,type ProviderStatus,type DictionarySnapshot,type RetainedRecovery} from './contracts';
import {fromDetail,fromSummary,type Dictation} from '../model';
import {mergeLiveLevel,receiveLevel} from './liveLevel';
export interface DesktopState {onboarding:OnboardingState|null;available:boolean;status:'browser'|'loading'|'ready'|'error';error:string;errors:DesktopError[];recorderModeRequest:RecorderModeRequest|null;preferences:NativePreferences;modes:ModeRecord[];dictionary:DictionarySnapshot|null;usage:DailyUsage;provider:ProviderStatus;devices:AudioDevice[];devicesStatus:'idle'|'loading'|'ready'|'error';session:LiveSession|null;recovery:RetainedRecovery|null;temporaryResult:HistoryDetail|null;capabilities:Record<string,boolean>}
interface DesktopContextValue extends DesktopState {
 records:Dictation[];historyTotal:number;nextCursor:number|null;historyLoading:boolean;query:string;
 command:<T>(name:string,payload?:Record<string,unknown>)=>Promise<T>;refresh:()=>Promise<void>;
 searchHistory:(query:string)=>Promise<void>;loadMore:()=>Promise<void>;loadRecord:(id:string)=>Promise<Dictation>;acceptDetail:(detail:HistoryDetail)=>void;
 acceptDictionary:(snapshot:DictionarySnapshot)=>void;
}
const Context=createContext<DesktopContextValue|null>(null);
const emptyUsage:DailyUsage={date:'',count:0,sourceCharCount:0,captureDurationMs:0};
const sessionId=(session:LiveSession|null)=>session?.sessionId??session?.id;
const matchesSession=(identity:{sessionId?:string;generation?:number},session:LiveSession|null)=>!!session&&identity.sessionId===sessionId(session)&&identity.generation===session.generation;
const canOpenMode=(session:LiveSession|null)=>!!session&&['starting','recording'].includes(session.state.toLowerCase());
const issueKey=(issue:DesktopError)=>[issue.scope,issue.sessionId??'',issue.generation??'',issue.operation??''].join(':');
function withErrors(state:DesktopState,errors:DesktopError[]):DesktopState{return {...state,errors,error:[...new Set(errors.map(issue=>issue.message))].join('；')};}
function putError(state:DesktopState,issue:DesktopError):DesktopState{return withErrors(state,[...state.errors.filter(old=>issueKey(old)!==issueKey(issue)),issue]);}
function clearReadError(state:DesktopState,operation:string):DesktopState{return withErrors(state,state.errors.filter(issue=>issue.scope!=='transport'||issue.operation!==operation));}
function withSession(state:DesktopState,session:LiveSession|null):DesktopState {
 session=mergeLiveLevel(state.session,session);
 // Identity changes retire only the old session's errors, never application faults.
 return withErrors({...state,session,recorderModeRequest:matchesSession(state.recorderModeRequest??{},session)&&canOpenMode(session)?state.recorderModeRequest:null},state.errors.filter(issue=>issue.scope!=='session'||matchesSession(issue,session)));
}
function eventIssue(event:AppEvent,current:LiveSession|null):DesktopError|null {
 const payload=event.payload&&typeof event.payload==='object'?event.payload as Record<string,unknown>:{};
 const explicitScope=payload.scope??event.scope;
 const id=typeof payload.sessionId==='string'?payload.sessionId:event.sessionId;
 const generation=typeof payload.generation==='number'?payload.generation:event.generation;
 const scope=explicitScope==='application'||explicitScope==='transport'||explicitScope==='session'?explicitScope:(id!==undefined||generation!==undefined||current?'session':'application');
 // Legacy unscoped errors are attributed at receipt. Native producers must attach
 // identity to session errors so a late error cannot be attributed to a new session.
 const issue:DesktopError={scope,message:errorMessage(event.payload),...(scope==='session'?{sessionId:id??(explicitScope===undefined?sessionId(current):undefined),generation:generation??(explicitScope===undefined?current?.generation:undefined)}:{}),...(scope==='transport'?{operation:'event'}:{})};
 return scope==='session'&&!matchesSession(issue,current)?null:issue;
}
export function DesktopProvider({children}:{children:ReactNode}) {
 const available=desktopAvailable();
 const recorderSurface=new URLSearchParams(location.search).get('window')==='recorder';
 const [state,setState]=useState<DesktopState>({onboarding:null,available,status:available?'loading':'browser',error:'',errors:[],recorderModeRequest:null,preferences:{...initialPreferences},modes:[],dictionary:null,usage:emptyUsage,provider:initialProvider,devices:[],devicesStatus:'idle',session:null,recovery:null,temporaryResult:null,capabilities:{}});
 const [records,setRecords]=useState<Dictation[]>([]),[historyTotal,setHistoryTotal]=useState(0),[nextCursor,setNextCursor]=useState<number|null>(null),[historyLoading,setHistoryLoading]=useState(false),[query,setQuery]=useState('');
 const queryRef=useRef(''),listRun=useRef(0),deviceReadRun=useRef(0),sequence=useRef(-1),alive=useRef(true),refreshTimer=useRef<ReturnType<typeof setTimeout>|undefined>(undefined);
 const details=useRef(new Map<string,Dictation>());
 const sessionHead=useRef<{id:string;generation:number}|null>(null),retiredSessions=useRef(new Map<string,number>());
 // Only an authoritative snapshot can reactivate an older session identity,
 // and its generation must advance. Delayed worker events cannot resurrect it.
 const acceptSession=useCallback((next:LiveSession|null,authoritative=false)=>{
  const id=sessionId(next),head=sessionHead.current;
  if(next&&id){
   const retired=retiredSessions.current.get(id);
   if((retired!==undefined&&(!authoritative||next.generation<=retired))||(head?.id===id&&next.generation<head.generation))return false;
   if(head&&head.id!==id)retiredSessions.current.set(head.id,head.generation);
   if(retired!==undefined)retiredSessions.current.delete(id);
   sessionHead.current={id,generation:next.generation};
  }else if(head){retiredSessions.current.set(head.id,head.generation);sessionHead.current=null;}
  return true;
 },[]);
 const command=useCallback(<T,>(name:string,payload:Record<string,unknown>={})=>desktopCommand<T>(name,payload),[]);
 const acceptDictionary=useCallback((dictionary:DictionarySnapshot)=>{setState(prev=>prev.dictionary&&prev.dictionary.version>dictionary.version?prev:{...prev,dictionary});},[]);
 const acceptDetail=useCallback((detail:HistoryDetail)=>{const record=fromDetail(detail,detail.session.storagePolicy==='memoryOnly');if(!record.temporary)details.current.set(record.id,record);setRecords(prev=>prev.map(r=>r.id===record.id?record:r));setState(prev=>prev.temporaryResult?.session.id===record.id?{...prev,temporaryResult:detail}:prev);},[]);
 const loadRecord=useCallback(async(id:string)=>{const detail=await command<HistoryDetail>('history.get',{sessionId:id});const record=fromDetail(detail,detail.session.storagePolicy==='memoryOnly');acceptDetail(detail);return record;},[command,acceptDetail]);
 const fetchHistory=useCallback(async(search:string,cursor:number|null=null)=>{
  if(!available)return;const run=++listRun.current;setHistoryLoading(true);
  try {const page=await command<HistoryPage>('history.list',{query:search,cursor,limit:20});if(!alive.current||run!==listRun.current)return;
   const mapped=page.items.map(item=>{const cached=details.current.get(item.sessionId);return cached??fromSummary(item);});
   setRecords(prev=>cursor===null?mapped:[...prev,...mapped.filter(r=>!prev.some(p=>p.id===r.id))]);setHistoryTotal(page.total);setNextCursor(page.nextCursor??null);
   const needsDetail=page.items.filter((item,index)=>(cursor===null&&!search&&index<3)||(details.current.has(item.sessionId)&&(details.current.get(item.sessionId)?.revision!==(item.currentRevisionId??'')||details.current.get(item.sessionId)?.sourceRevisionId!==item.sourceRevisionId)));await Promise.allSettled(needsDetail.map(item=>loadRecord(item.sessionId)));
   if(cursor===null&&!search){const present=new Set(page.items.map(i=>i.sessionId));for(const [id] of details.current){if(!present.has(id))details.current.delete(id);}}
  } finally {if(alive.current&&run===listRun.current)setHistoryLoading(false);}
 },[available,command,loadRecord]);
 const applySnapshot=useCallback((snap:NativeSnapshot)=>{
  if(!snap||typeof snap!=='object')return;if(typeof snap.sequence==='number'&&snap.sequence<sequence.current)return;
  sequence.current=Math.max(sequence.current,snap.sequence??0);
  const updateSession='session'in snap&&acceptSession(snap.session??null,true);
  setState(prev=>{const next={...prev,...(snap.onboarding?{onboarding:snap.onboarding}:{}),...(snap.preferences?{preferences:snap.preferences}:{}),...(snap.modes?{modes:snap.modes}:{}),...(snap.dictionary&&(!prev.dictionary||snap.dictionary.version>=prev.dictionary.version)?{dictionary:snap.dictionary}:{}),...(snap.usage?{usage:snap.usage}:{}),...(snap.provider?{provider:snap.provider}:{}),...(snap.devices?{devices:snap.devices}:{}),...('recovery'in snap?{recovery:snap.recovery??null}:{}),...('temporaryResult'in snap?{temporaryResult:snap.temporaryResult??null}:{}),...(snap.capabilities?{capabilities:snap.capabilities}:{}),status:'ready' as const};return updateSession?withSession(next,snap.session??null):next;});
 },[acceptSession]);
 const readFailed=useCallback((operation:string,error:unknown)=>{if(alive.current)setState(prev=>putError(prev,{scope:'transport',operation,message:errorMessage(error)}));},[]);
 const refresh=useCallback(async()=>{
  if(!available)return;
  if(recorderSurface){try{applySnapshot(await command<NativeSnapshot>('state.snapshot'));if(alive.current)setState(prev=>clearReadError(prev,'snapshot'));}catch(e){readFailed('snapshot',e);}return;}
  const deviceRun=++deviceReadRun.current;
  setState(prev=>({...prev,devicesStatus:'loading'}));
  const results=await Promise.allSettled([command<NativeSnapshot>('state.snapshot'),command<AudioDevice[]>('audio.listDevices'),fetchHistory(queryRef.current)]);
  if(!alive.current)return;
  if(results[0].status==='fulfilled')applySnapshot(results[0].value);
  setState(prev=>{let next:DesktopState={...prev,status:results[0].status==='fulfilled'?'ready':'error'};
   // Enumeration is an independent read. Keep the saved preference and reject
   // late device results/errors once a newer refresh owns this display state.
   if(deviceRun===deviceReadRun.current){const result=results[1];next=result.status==='fulfilled'
    ?clearReadError({...next,devices:result.value,devicesStatus:'ready'},'devices')
    :putError({...next,devicesStatus:'error'},{scope:'transport',operation:'devices',message:errorMessage(result.reason)});}
   for(const [index,operation] of [[0,'snapshot'],[2,'history']] as const){const result=results[index];next=result.status==='fulfilled'?clearReadError(next,operation):putError(next,{scope:'transport',operation,message:errorMessage(result.reason)});}return next;});
 },[available,recorderSurface,command,fetchHistory,applySnapshot,readFailed]);
 useEffect(()=>{
  alive.current=true;if(!available)return;
  let active=true;let resyncing=false;let queued:AppEvent[]=[];
  const applyIntent=(event:AppEvent)=>{
   if(event.kind==='error'){setState(prev=>{const issue=eventIssue(event,prev.session);return issue?putError(prev,issue):prev;});return;}
   if(event.kind==='recorder.openMode'&&recorderSurface){const payload=event.payload as Partial<RecorderModeRequest>|null;if((event.sessionId!==undefined&&payload?.sessionId!==undefined&&event.sessionId!==payload.sessionId)||(event.generation!==undefined&&payload?.generation!==undefined&&event.generation!==payload.generation))return;const request={sequence:event.sequence,sessionId:event.sessionId??payload?.sessionId??'',generation:event.generation??payload?.generation??-1};setState(prev=>matchesSession(request,prev.session)&&canOpenMode(prev.session)?{...prev,recorderModeRequest:request}:prev);}
  };
  const resync=()=>{if(resyncing)return;resyncing=true;void command<NativeSnapshot>('state.snapshot').then(snap=>{if(!active)return;applySnapshot(snap);setState(prev=>clearReadError(prev,'snapshot'));const pending=queued;queued=[];resyncing=false;pending.sort((a,b)=>a.sequence-b.sequence).forEach(event=>{
   // Snapshots do not contain ephemeral errors/menu intents. Preserve queued
   // intents even when the snapshot's sequence already includes their publication.
   if(event.sequence<=sequence.current&&(event.kind==='error'||event.kind==='recorder.openMode'))applyIntent(event);else receive(event);
  });}).catch(e=>{const pending=queued;queued=[];resyncing=false;if(active){readFailed('snapshot',e);pending.filter(event=>event.kind==='error').forEach(applyIntent);}});};
  const receive=(event:AppEvent)=>{
   if(!active)return;
   if(event.kind==='level'){const raw=event.payload as {level:number;sessionId?:string;generation?:number};const payload={...raw,sessionId:raw.sessionId??event.sessionId,generation:raw.generation??event.generation};if(typeof payload.level!=='number'||!Number.isFinite(payload.level))return;setState(prev=>{const session=receiveLevel(prev.session,payload,performance.now());return session===prev.session?prev:{...prev,session};});return;}
   if(resyncing){queued.push(event);return;}
   if(typeof event.sequence==='number'&&event.sequence<=sequence.current)return;
   if(sequence.current>=0&&event.sequence>sequence.current+1){queued.push(event);resync();return;}
   if(event.kind==='snapshot'){applySnapshot({...event.payload as NativeSnapshot,sequence:event.sequence});return;}
   sequence.current=event.sequence;
   if(event.kind==='session'){const next=event.payload as LiveSession|null;if((next===null||typeof next==='object')&&acceptSession(next))setState(prev=>withSession(prev,next));return;}
   if(event.kind==='error'||event.kind==='recorder.openMode'){applyIntent(event);return;}
   clearTimeout(refreshTimer.current);refreshTimer.current=setTimeout(()=>{if(recorderSurface)resync();else void refresh();},80);
  };
  void subscribeDesktop(receive).then(snap=>{if(active){applySnapshot(snap);setState(prev=>clearReadError(prev,'subscribe'));}}).catch(error=>{if(active){readFailed('subscribe',error);setState(prev=>({...prev,status:'error'}));}});
  void refresh();
  const visible=()=>{if(document.visibilityState==='visible')void refresh();};document.addEventListener('visibilitychange',visible);
  return()=>{active=false;alive.current=false;clearTimeout(refreshTimer.current);document.removeEventListener('visibilitychange',visible);};
 },[available,recorderSurface,command,applySnapshot,refresh,acceptSession,readFailed]);
 const searchHistory=useCallback(async(search:string)=>{queryRef.current=search;setQuery(search);await fetchHistory(search);},[fetchHistory]);
 return <Context.Provider value={{...state,records,historyTotal,nextCursor,historyLoading,query,command,refresh,searchHistory,loadMore:()=>fetchHistory(queryRef.current,nextCursor),loadRecord,acceptDetail,acceptDictionary}}>{children}</Context.Provider>;
}
export function useDesktop(){const value=useContext(Context);if(!value)throw new Error('DesktopProvider is required');return value;}



