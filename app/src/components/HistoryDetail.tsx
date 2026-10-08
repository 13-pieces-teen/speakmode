import {Fragment,useCallback,useEffect,useId,useMemo,useRef,useState} from 'react';
import {Button,Select,TextArea} from '@radix-ui/themes';
import {ArrowLeftIcon,ChevronDownIcon,ChevronUpIcon,Pencil2Icon,ReaderIcon,ResetIcon,TrashIcon} from '@radix-ui/react-icons';
import {characterDiff,type Candidate,type Dictation,type Mode} from '../model';
import {ModeBadge,ModeSelector,RecordText} from './ui';
import {CopyAction} from './CopyAction';
import {focusInView} from './focusInView';
import {useModes} from '../hooks/useModes';
import {isPersonalMode} from '../modes';
import {useDictations} from '../hooks/useDictations';
import {useDesktop} from '../services/DesktopProvider';
import {errorMessage} from '../services/bridge';
import {useNavigationGuard} from './NavigationGuard';
import type {RestorePatch} from '../services/contracts';
import './HistoryDetail.css';
interface Props {record:Dictation;onBack:()=>void}
export function HistoryDetail({record,onBack}:Props) {
 const actions=useDictations(),desktop=useDesktop(),{findMode}=useModes(),navigation=useNavigationGuard();
 const [compareOpen,setCompareOpen]=useState(false),[modeOpen,setModeOpen]=useState(false),[draftMode,setDraftMode]=useState<Mode>(record.mode),[candidate,setCandidate]=useState<Candidate|null>(null);
 const [notice,setNotice]=useState(''),[error,setError]=useState(''),[busy,setBusy]=useState(''),[editing,setEditing]=useState(false),[editText,setEditText]=useState(''),[deleteOpen,setDeleteOpen]=useState(false);
 const [restoreBlock,setRestoreBlock]=useState(''),[restoreSource,setRestoreSource]=useState(''),[sourceRange,setSourceRange]=useState({start:0,end:0}),[targetRange,setTargetRange]=useState({start:0,end:0});
 const editBaseline=useRef(record);
 const candidateRunId=useRef<string|null>(null);
 const run=useRef(0),recordRef=useRef(record),actionsRef=useRef(actions),busyRef=useRef(false),heading=useRef<HTMLHeadingElement>(null),editor=useRef<HTMLTextAreaElement>(null);
 recordRef.current=record;actionsRef.current=actions;
 const panelId=useId(),candidateId=useId(),compareId=useId();
 const output=record.detail?.output,source=record.detail?.source;
 const dirty=editing&&editText!==editBaseline.current.current.join('\n\n');
 const mounted=useRef(true);
 useEffect(()=>{mounted.current=true;focusInView(heading.current);return()=>{mounted.current=false;run.current++;const runId=candidateRunId.current;candidateRunId.current=null;if(runId)void actionsRef.current.cancelCandidate(record.id,runId).catch(()=>{});navigation.register(null);};},[record.id,navigation.register]);
 const saveEdit=useCallback(async()=>{if(!editText.trim())throw new Error('结果不能为空。');const current=editBaseline.current;await actionsRef.current.edit(current,[{id:crypto.randomUUID(),text:editText,sourceRefs:[]}]);if(mounted.current){setEditing(false);setNotice('已保存手动修改，可撤销；其他应用中的内容未改变。');}},[editText]);
 useEffect(()=>{navigation.register({dirty,save:saveEdit,discard:()=>setEditing(false)});return()=>navigation.register(null);},[dirty,saveEdit,navigation.register]);
 async function perform(label:string,action:()=>Promise<unknown>,message:string){if(busyRef.current)return;busyRef.current=true;setBusy(label);setError('');try{await action();if(mounted.current)setNotice(message);}catch(e){if(mounted.current)setError(errorMessage(e));throw e;}finally{busyRef.current=false;if(mounted.current)setBusy('');}}
 function guarded(action:()=>void){navigation.request(action);}
 async function generate(){const generation=++run.current;const runId=crypto.randomUUID();candidateRunId.current=runId;setCandidate(null);setBusy('candidate');setError('');setNotice('');try{const next=await actions.generate(record,draftMode,runId);if(mounted.current&&run.current===generation&&candidateRunId.current===runId){setCandidate(next);setModeOpen(false);}}catch(e){if(mounted.current&&run.current===generation)setError(errorMessage(e));}finally{if(mounted.current&&run.current===generation)setBusy('');}}
 async function cancelCandidate(){const runId=candidateRunId.current;const generation=++run.current;setCandidate(null);setBusy('');setNotice('');if(!runId)return;try{await actions.cancelCandidate(record.id,runId);if(candidateRunId.current===runId)candidateRunId.current=null;if(mounted.current&&run.current===generation)setNotice('候选已取消，当前版本保持不变。');}catch(e){if(mounted.current&&run.current===generation)setError(errorMessage(e));}}
 const stale=candidate!==null&&candidate.baseRevision!==record.revision;
 const preview:Dictation|null=candidate?{...record,current:candidate.texts,mode:candidate.mode,resolved:candidate.resolved,sourceState:'model',modeName:candidate.modeName,modeVersion:candidate.modeVersion,modeInstructions:candidate.modeInstructions}:null;
 const block=output?.blocks.find(b=>b.id===restoreBlock),item=source?.items.find(i=>i.id===restoreSource);
 const replacement=item?Array.from(item.text).slice(sourceRange.start,sourceRange.end).join(''):'';
 const replacementPreview=block?Array.from(block.text).slice(0,targetRange.start).join('')+replacement+Array.from(block.text).slice(targetRange.end).join(''):'';
 const rangeValid=!!block&&!!item&&sourceRange.end>sourceRange.start&&targetRange.end>=targetRange.start&&targetRange.end<=Array.from(block.text).length;
 function chooseBlock(id:string){setRestoreBlock(id);const next=output?.blocks.find(b=>b.id===id);setTargetRange({start:0,end:Array.from(next?.text??'').length});}
 function chooseSource(id:string){setRestoreSource(id);const next=source?.items.find(i=>i.id===id);setSourceRange({start:0,end:Array.from(next?.text??'').length});}
 function selection(text:string,start:number,end:number){return {start:Array.from(text.slice(0,start)).length,end:Array.from(text.slice(0,end)).length};}
 const fullDiff=useMemo(()=>characterDiff(record.source.join('\n\n'),record.current.join('\n\n')),[record.source,record.current]);
 const showDiff=(segments:ReturnType<typeof characterDiff>['source'],className:string)=>segments.map((segment,i)=>segment.changed?<mark className={className} key={i}>{segment.text}</mark>:<Fragment key={i}>{segment.text}</Fragment>);
 return <section className="history-detail surface" aria-label={record.title} aria-busy={Boolean(busy)}>
  <Button size="2" variant="ghost" onClick={()=>guarded(onBack)} className="history-back"><ArrowLeftIcon/>返回列表</Button>
  <header className="history-detail-header"><div className="history-title-block"><p className="history-record-date">{record.date} · {record.time}{record.temporary?' · 临时结果，不保存正文':''}</p><h2 ref={heading} className="history-record-heading" tabIndex={-1}>{record.title}</h2></div><CopyAction record={record} label="复制文本"/></header>
  <div className="history-reading-layout"><div className="history-current"><p className="eyebrow">当前文本</p>{editing?<div className="history-edit-form"><TextArea ref={editor} aria-label="编辑当前文本" value={editText} onChange={e=>setEditText(e.target.value)} rows={12}/><div className="history-panel-actions"><Button variant="soft" color="gray" disabled={Boolean(busy)} onClick={()=>setEditing(false)}>取消修改</Button><Button disabled={!dirty||Boolean(busy)} onClick={()=>void perform('edit',saveEdit,'已保存修改。').catch(()=>{})}>保存修改</Button></div></div>:<RecordText record={record}/>}</div>
   <aside className="history-tools" aria-label="本条记录操作"><div className="history-tool-group"><h3 className="history-tool-label">整理方式</h3><ModeBadge mode={record.mode} resolved={record.resolved} name={record.modeName} sourceState={record.sourceState}/>{record.modeInstructions&&<details className="history-mode-snapshot"><summary>{record.sourceState==='model'?'本次使用的规则':'本次配置的规则'} · v{record.modeVersion}</summary><p>{record.modeInstructions}</p></details>}
    <Button size="2" variant="soft" color="gray" disabled={Boolean(busy)||editing||!source?.complete} aria-expanded={modeOpen} aria-controls={panelId} onClick={()=>{setDraftMode(isPersonalMode(record.mode)&&!findMode(record.mode)?'自动':record.mode);setModeOpen(!modeOpen);}}>换种整理{modeOpen?<ChevronUpIcon/>:<ChevronDownIcon/>}</Button>
   </div><div className="history-tool-group history-version-tools"><h3 className="history-tool-label">本地版本</h3><span className="history-version-summary">{record.change||'初始版本'}</span><Button size="2" variant="soft" color="gray" disabled={Boolean(busy)||editing||!output} onClick={()=>{editBaseline.current=record;setEditText(record.current.join('\n\n'));setEditing(true);requestAnimationFrame(()=>focusInView(editor.current));}}><Pencil2Icon/>手动修改</Button><Button size="2" variant="ghost" disabled={!record.undoAvailable||Boolean(busy)||editing} onClick={()=>void perform('undo',()=>actions.undo(record),'已撤销上次本地修改。').catch(()=>{})}><ResetIcon/>撤销上次修改</Button>
    {!record.temporary&&<Button size="2" variant="ghost" color="gray" disabled={Boolean(busy)||editing} onClick={()=>setDeleteOpen(!deleteOpen)}><TrashIcon/>删除记录</Button>}
   </div></aside></div>
  <p className="history-local-status" role="status" aria-live="polite">{busy==='candidate'?'正在生成候选…':busy?'正在保存…':notice}</p>{error&&<p className="inline-error" role="alert">{error}</p>}
  {deleteOpen&&<div className="history-mode-panel"><p>删除这条记录的正文与版本？今日使用量不会扣除。</p><div className="history-panel-actions"><Button variant="soft" color="gray" onClick={()=>setDeleteOpen(false)}>保留</Button><Button color="red" disabled={Boolean(busy)} onClick={()=>void perform('delete',async()=>{await actions.remove(record.id);onBack();},'').catch(()=>{})}>删除记录</Button></div></div>}
  {modeOpen&&<section id={panelId} className="history-mode-panel"><h3 className="section-title">选择整理方式</h3><p className="history-secondary">先生成候选，再决定是否采用。新口述默认设置保持不变。</p><ModeSelector value={draftMode} onChange={setDraftMode} label="本条候选的整理方式"/><div className="history-panel-actions"><Button variant="ghost" color="gray" onClick={()=>busy==='candidate'?void cancelCandidate():setModeOpen(false)}>取消</Button><Button loading={busy==='candidate'} disabled={!desktop.available||Boolean(busy)} onClick={()=>void generate()}>生成候选</Button></div></section>}
  {candidate&&preview&&<section className="history-candidate" aria-labelledby={candidateId}><div className="history-section-topline"><h3 className="section-title" id={candidateId}>新候选</h3><ModeBadge mode={candidate.mode} resolved={candidate.resolved} name={candidate.modeName} sourceState="model"/></div><p className="history-secondary">来自识别稿，当前文本尚未改变。</p><RecordText record={preview}/><p className="history-candidate-warning">采用会替换本条当前版本{record.modified?'，包括已有本地修改':''}；不会修改其他应用中的文本。</p>{stale&&<p role="alert" className="inline-error">当前版本已改变。候选仍保留供核对，请重新生成后采用。</p>}<div className="history-panel-actions"><Button variant="ghost" color="gray" onClick={()=>void cancelCandidate()}>取消候选</Button><Button disabled={stale||Boolean(busy)||editing} onClick={()=>void perform('adopt',async()=>{await actions.adopt(record,candidate);if(candidateRunId.current===candidate.native.id){candidateRunId.current=null;if(mounted.current)setCandidate(null);}},'已采用此版本。').catch(()=>{})}>采用这个版本</Button></div></section>}
  {!output&&source?.complete&&<div className="history-mode-panel"><p>整理未完成，完整识别稿仍可使用。</p><Button onClick={()=>void perform('source',async()=>{await desktop.command('result.useSource',{resultId:record.id,sessionId:record.id,expectedGeneration:record.detail?.session.generation,sourceRevisionId:source.id});await desktop.loadRecord(record.id);await desktop.refresh();},'已采用识别稿，请核对后复制。').catch(()=>{})}>使用识别稿</Button></div>}
  <Button size="2" variant="soft" color="gray" className="history-compare-toggle" aria-expanded={compareOpen} aria-controls={compareId} onClick={()=>setCompareOpen(!compareOpen)}><ReaderIcon/><strong>{compareOpen?'收起识别稿对照':'对照识别稿'}</strong><small>按需核对与恢复</small>{compareOpen?<ChevronUpIcon/>:<ChevronDownIcon/>}</Button>
  {compareOpen&&<section id={compareId} className="history-comparison"><h3 className="section-title">识别稿与当前文本</h3><p className="history-secondary">识别稿也可能听错。颜色仅表示字符差异，不判断意思是否正确；恢复只改本地结果。</p><div className="history-sentence-texts history-full-comparison"><div className="history-sentence-line"><span>识别稿</span><p>{showDiff(fullDiff.source,'history-source-diff')}</p></div><div className="history-sentence-line"><span>当前文本</span><p>{showDiff(fullDiff.current,'history-current-diff')}</p></div></div>
   {output&&source&&<div className="history-restore-form"><h3 className="section-title">恢复选定内容</h3><p className="history-secondary">选择来源与目标段落，可在文本框中选中更小的范围；预览确认后仅替换选中位置。</p>
    <div className="history-sentence-texts"><div><label htmlFor="restore-source">识别来源</label><Select.Root value={restoreSource} onValueChange={chooseSource}><Select.Trigger id="restore-source" placeholder="选择识别段" aria-label="选择识别来源"/><Select.Content>{source.items.map((i,n)=><Select.Item key={i.id} value={i.id}>第 {n+1} 段 · {i.text.slice(0,32)}</Select.Item>)}</Select.Content></Select.Root>{item&&<TextArea aria-label="选择识别稿中的恢复范围" value={item.text} readOnly rows={5} onSelect={e=>{const t=e.currentTarget;if(t.selectionEnd>t.selectionStart)setSourceRange(selection(item.text,t.selectionStart,t.selectionEnd));}}/>}</div>
     <div><label htmlFor="restore-target">当前结果位置</label><Select.Root value={restoreBlock} onValueChange={chooseBlock}><Select.Trigger id="restore-target" placeholder="选择结果段" aria-label="选择恢复目标"/><Select.Content>{output.blocks.map((b,n)=><Select.Item key={b.id} value={b.id}>第 {n+1} 段 · {b.text.slice(0,32)}</Select.Item>)}</Select.Content></Select.Root>{block&&<TextArea aria-label="选择当前文本中的替换范围" value={block.text} readOnly rows={5} onSelect={e=>{const t=e.currentTarget;if(t.selectionEnd>t.selectionStart)setTargetRange(selection(block.text,t.selectionStart,t.selectionEnd));}}/>}</div></div>
    {rangeValid&&<div className="history-restore-preview"><strong>替换后的目标段预览</strong><p>{replacementPreview}</p><Button disabled={Boolean(busy)||editing||replacementPreview===block?.text} onClick={()=>{const patch:RestorePatch={blockId:restoreBlock,start:targetRange.start,end:targetRange.end,sourceSelections:[{itemId:restoreSource,...sourceRange}]};void perform('restore',async()=>{await actions.restore(record,patch);setRestoreBlock('');setRestoreSource('');},'已恢复所选内容，其他段落保持不变，可撤销。').catch(()=>{});}}>确认恢复这部分</Button></div>}
   </div>}
  </section>}
 </section>;
}



