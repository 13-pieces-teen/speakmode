import {Fragment,useCallback,useEffect,useId,useMemo,useRef,useState} from 'react';
import {Button,Select,TextArea} from '@radix-ui/themes';
import {ArrowLeftIcon,ChevronDownIcon,ChevronUpIcon,InfoCircledIcon,Pencil2Icon,ReaderIcon,ResetIcon,TrashIcon} from '@radix-ui/react-icons';
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
import {buildRestorePreview,captureRestoreSelection,type RestoreSelection} from './restoreSelection';
import {TermCorrectionPanel} from './TermCorrectionPanel';
import {CandidateComparison} from './CandidateComparison';
import {ConfirmedContextEditor,ConfirmationSummary} from './ConfirmedContextEditor';
import type {ConfirmedContext} from '../services/contracts';
import './HistoryDetail.css';
interface Props {record:Dictation;onBack:()=>void}
export function HistoryDetail({record,onBack}:Props) {
 const actions=useDictations(),desktop=useDesktop(),{findMode}=useModes(),navigation=useNavigationGuard();
 const [compareOpen,setCompareOpen]=useState(false),[modeOpen,setModeOpen]=useState(false),[draftMode,setDraftMode]=useState<Mode>(record.mode),[candidate,setCandidate]=useState<Candidate|null>(null);
 const [notice,setNotice]=useState(''),[error,setError]=useState(''),[busy,setBusy]=useState(''),[editing,setEditing]=useState(false),[editText,setEditText]=useState(''),[deleteOpen,setDeleteOpen]=useState(false);
 const [correcting,setCorrecting]=useState(false);
 const [confirmedContext,setConfirmedContext]=useState<ConfirmedContext|undefined>(record.detail?.output?.confirmedContext);
 useEffect(()=>{setConfirmedContext(record.detail?.output?.confirmedContext);},[record.id,record.revision]);
 const [candidateBaseline,setCandidateBaseline]=useState(''),[candidateText,setCandidateText]=useState('');
 const [restoreBlock,setRestoreBlock]=useState(''),[restoreSource,setRestoreSource]=useState(''),[sourceSelection,setSourceSelection]=useState<RestoreSelection|null>(null),[targetSelection,setTargetSelection]=useState<RestoreSelection|null>(null);
 const editBaseline=useRef(record);
 const candidateRunId=useRef<string|null>(null);
 const run=useRef(0),recordRef=useRef(record),actionsRef=useRef(actions),busyRef=useRef(false),heading=useRef<HTMLHeadingElement>(null),editor=useRef<HTMLTextAreaElement>(null);
 const modeTrigger=useRef<HTMLButtonElement>(null),correctionTrigger=useRef<HTMLButtonElement>(null),deleteTrigger=useRef<HTMLButtonElement>(null);
 const modeHeading=useRef<HTMLHeadingElement>(null),deleteHeading=useRef<HTMLHeadingElement>(null),candidateHeading=useRef<HTMLHeadingElement>(null);
 recordRef.current=record;actionsRef.current=actions;
 const panelId=useId(),candidateId=useId(),compareId=useId(),correctionId=useId(),deleteId=useId();
 const output=record.detail?.output,source=record.detail?.source;
 useEffect(()=>{setRestoreBlock('');setRestoreSource('');setSourceSelection(null);setTargetSelection(null);},[record.id,record.revision,source?.id]);
 const dirty=(editing&&editText!==editBaseline.current.current.join('\n\n'))||candidate!==null;
 const mounted=useRef(true);
 useEffect(()=>{mounted.current=true;focusInView(heading.current);return()=>{mounted.current=false;run.current++;const runId=candidateRunId.current;candidateRunId.current=null;if(runId)void actionsRef.current.cancelCandidate(record.id,runId).catch(()=>{});navigation.register(null);};},[record.id,navigation.register]);
 useEffect(()=>{if(modeOpen)focusInView(modeHeading.current);},[modeOpen]);
 useEffect(()=>{if(deleteOpen)focusInView(deleteHeading.current);},[deleteOpen]);
 useEffect(()=>{if(candidate)focusInView(candidateHeading.current);},[candidate]);
 const saveEdit=useCallback(async()=>{if(!editText.trim())throw new Error('结果不能为空。');const current=editBaseline.current;await actionsRef.current.edit(current,[{id:crypto.randomUUID(),text:editText,sourceRefs:[]}]);if(mounted.current){setEditing(false);setNotice('已保存手动修改，可撤销；其他应用中的内容未改变。');}},[editText]);
 const saveCandidate=useCallback(async()=>{
  if(!candidate)throw new Error('请先生成候选。');
  if(!candidateText.trim())throw new Error('结果不能为空。');
  const current=recordRef.current;
  if(candidate.baseRevision!==current.revision)throw new Error('当前版本已变化，请重新生成后保存。');
  await actionsRef.current.adopt(current,candidate,candidateText);
  if(candidateRunId.current===candidate.native.id){candidateRunId.current=null;if(mounted.current)setCandidate(null);}
 },[candidate,candidateText]);
 useEffect(()=>{if(correcting)return;navigation.register({dirty,pending:Boolean(busy),save:candidate?async()=>{await perform('adopt',saveCandidate,'已保存你的选择，可撤销。');}:saveEdit,discard:()=>{setEditing(false);if(candidate)void cancelCandidate();}});return()=>navigation.register(null);},[dirty,saveEdit,saveCandidate,correcting,navigation.register,busy,candidate]);
 async function perform(label:string,action:()=>Promise<unknown>,message:string){if(busyRef.current)return;busyRef.current=true;setBusy(label);setError('');try{await action();if(mounted.current)setNotice(message);}catch(e){if(mounted.current)setError(errorMessage(e));throw e;}finally{busyRef.current=false;if(mounted.current)setBusy('');}}
 function guarded(action:()=>void){navigation.request(action);}
 function closeCorrection(){setCorrecting(false);requestAnimationFrame(()=>focusInView(correctionTrigger.current));}
 function closeDelete(){setDeleteOpen(false);requestAnimationFrame(()=>focusInView(deleteTrigger.current));}
 async function closeMode(){setModeOpen(false);const cancellation=busy==='candidate'?cancelCandidate():undefined;const generation=run.current;requestAnimationFrame(()=>{if(mounted.current&&run.current===generation)focusInView(modeTrigger.current);});await cancellation;}
 async function generate(mode=draftMode){const generation=++run.current;const runId=crypto.randomUUID();candidateRunId.current=runId;setCandidate(null);setCandidateText('');setCandidateBaseline(record.current.join('\n\n'));setBusy('candidate');setError('');setNotice('');try{const next=await actions.generate(record,mode,runId,confirmedContext);if(mounted.current&&run.current===generation&&candidateRunId.current===runId){setCandidateText(next.texts.join('\n\n'));setCandidate(next);setModeOpen(false);}}catch(e){if(mounted.current&&run.current===generation)setError(errorMessage(e));}finally{if(mounted.current&&run.current===generation)setBusy('');}}
 async function cancelCandidate(restoreFocus=false){const runId=candidateRunId.current;candidateRunId.current=null;const generation=++run.current;setCandidate(null);setBusy('');setNotice('');try{if(runId){await actions.cancelCandidate(record.id,runId);if(mounted.current&&run.current===generation)setNotice('候选已取消，当前版本保持不变。');}}catch(e){if(mounted.current&&run.current===generation){if(!candidateRunId.current)candidateRunId.current=runId;setError(errorMessage(e));}}finally{if(restoreFocus&&mounted.current&&run.current===generation)requestAnimationFrame(()=>focusInView(modeTrigger.current));}}
 const stale=candidate!==null&&candidate.baseRevision!==record.revision;
 const candidateHints=candidate?.reviewIssues.map(issue=>issue==='valuesChanged'?'数字、单位或版本可能有变化':issue==='literalsChanged'?'引语或代码有变化':issue==='tenseChanged'?'现在或过去的状态可能有变化':issue==='confirmedTextChanged'?'有已确认内容未按原样保留':issue==='reviewRejected'?'全文复核仍有疑点，请核对后选择保留或修改':issue==='reviewUnavailable'?'全文复核未完成，请自行核对':'条件、否定或其他信息可能有变化')??[];
 const block=output?.blocks.find(b=>b.id===restoreBlock),item=source?.items.find(i=>i.id===restoreSource);
 const sourceText=item&&source?{id:item.id,text:item.text,revisionId:source.id}:undefined;
 const targetText=block&&output?{id:block.id,text:block.text,revisionId:output.id}:undefined;
 const restorePreview=buildRestorePreview(sourceText,targetText,sourceSelection,targetSelection);
 function chooseBlock(id:string){setRestoreBlock(id);setTargetSelection(null);}
 function chooseSource(id:string){setRestoreSource(id);const next=source?.items.find(i=>i.id===id);setSourceSelection(next&&source?captureRestoreSelection({id:next.id,text:next.text,revisionId:source.id},0,next.text.length):null);setTargetSelection(null);}
 function selectSource(textarea:HTMLTextAreaElement){setSourceSelection(sourceText?captureRestoreSelection(sourceText,textarea.selectionStart,textarea.selectionEnd):null);}
 function selectTarget(textarea:HTMLTextAreaElement){setTargetSelection(targetText?captureRestoreSelection(targetText,textarea.selectionStart,textarea.selectionEnd):null);}
 function toggleComparison(){if(compareOpen){setRestoreBlock('');setRestoreSource('');setSourceSelection(null);setTargetSelection(null);}setCompareOpen(!compareOpen);}
 const fullDiff=useMemo(()=>characterDiff(record.source.join('\n\n'),record.current.join('\n\n')),[record.source,record.current]);
 const showDiff=(segments:ReturnType<typeof characterDiff>['source'],className:string)=>segments.map((segment,i)=>segment.changed?<mark className={className} key={i}>{segment.text}</mark>:<Fragment key={i}>{segment.text}</Fragment>);
 return <section className="history-detail surface" aria-label={record.title} aria-busy={Boolean(busy)}>
  <Button size="2" variant="ghost" onClick={()=>guarded(onBack)} className="history-back"><ArrowLeftIcon/>返回列表</Button>
  <header className="history-detail-header"><div className="history-title-block"><p className="history-record-date">{record.date} · {record.time}{record.temporary?' · 临时结果，不保存正文':''}</p><h2 ref={heading} className="history-record-heading" tabIndex={-1}>{record.title}</h2></div><CopyAction record={record} label="复制文本"/></header>
  <div className="history-reading-layout"><div className="history-current"><p className="eyebrow">当前文本</p>{editing?<div className="history-edit-form"><TextArea ref={editor} aria-label="编辑当前文本" value={editText} onChange={e=>setEditText(e.target.value)} rows={12}/><div className="history-panel-actions"><Button variant="soft" color="gray" disabled={Boolean(busy)} onClick={()=>setEditing(false)}>取消修改</Button><Button disabled={!dirty||Boolean(busy)} onClick={()=>void perform('edit',saveEdit,'已保存修改。').catch(()=>{})}>保存修改</Button></div></div>:<RecordText record={record}/>}</div>
   <aside className="history-tools" aria-label="本条记录操作"><div className="history-tool-group"><h3 className="history-tool-label">整理方式</h3><ModeBadge mode={record.mode} resolved={record.resolved} name={record.modeName} sourceState={record.sourceState}/>{record.modeInstructions&&<details className="history-mode-snapshot"><summary>{record.sourceState==='model'?'本次使用的规则':'本次配置的规则'} · v{record.modeVersion}</summary><p>{record.modeInstructions}</p></details>}
    <Button ref={modeTrigger} size="2" variant="soft" color="gray" disabled={Boolean(busy)||Boolean(candidate)||editing||correcting||!source?.complete} aria-expanded={modeOpen} aria-controls={panelId} onClick={()=>{setDraftMode(isPersonalMode(record.mode)&&!findMode(record.mode)?'自动':record.mode);setDeleteOpen(false);setModeOpen(!modeOpen);}}>换种整理{modeOpen?<ChevronUpIcon/>:<ChevronDownIcon/>}</Button>
   </div><div className="history-tool-group history-version-tools"><h3 className="history-tool-label">本地版本</h3><span className="history-version-summary">{record.change||'初始版本'}</span><Button size="2" variant="soft" color="gray" disabled={Boolean(busy)||Boolean(candidate)||editing||correcting||!output} onClick={()=>{editBaseline.current=record;setEditText(record.current.join('\n\n'));setEditing(true);requestAnimationFrame(()=>focusInView(editor.current));}}><Pencil2Icon/>手动修改</Button><Button ref={correctionTrigger} size="2" variant="soft" color="gray" disabled={Boolean(busy)||Boolean(candidate)||editing||correcting||!output} aria-expanded={correcting} aria-controls={correctionId} onClick={()=>{setCorrecting(true);setModeOpen(false);setDeleteOpen(false);setNotice('');setError('');}}>纠正词条</Button><Button size="2" variant="ghost" disabled={!record.undoAvailable||Boolean(busy)||Boolean(candidate)||editing||correcting} onClick={()=>void perform('undo',()=>actions.undo(record),'已撤销上次本地修改。').catch(()=>{})}><ResetIcon/>撤销上次修改</Button>
    {!record.temporary&&<Button ref={deleteTrigger} size="2" variant="ghost" color="gray" disabled={Boolean(busy)||Boolean(candidate)||editing||correcting} aria-expanded={deleteOpen} aria-controls={deleteId} onClick={()=>{setModeOpen(false);setDeleteOpen(!deleteOpen);}}><TrashIcon/>删除记录</Button>}
   </div></aside></div>
  {output?.confirmedContext&&<ConfirmationSummary context={output.confirmedContext}/>}
  {correcting&&<div id={correctionId}><TermCorrectionPanel record={record} onClose={closeCorrection} onSaved={message=>{closeCorrection();setNotice(message);}}/></div>}
  <p className="history-local-status" role="status" aria-live="polite">{busy==='candidate'?'正在生成候选…':busy?'正在保存…':notice}</p>{error&&<p className="inline-error" role="alert">{error}</p>}
  {deleteOpen&&<section id={deleteId} className="history-mode-panel history-delete-panel" aria-labelledby={`${deleteId}-title`}><h3 id={`${deleteId}-title`} ref={deleteHeading} tabIndex={-1} className="section-title">删除这条口述？</h3><p className="history-secondary">将删除这条记录的正文与版本，今日使用量不会扣除。</p><div className="history-panel-actions"><Button variant="soft" color="gray" disabled={Boolean(busy)} onClick={closeDelete}>保留</Button><Button color="red" disabled={Boolean(busy)} onClick={()=>void perform('delete',async()=>{await actions.remove(record.id);onBack();},'').catch(()=>{})}>删除记录</Button></div></section>}
  {modeOpen&&<section id={panelId} className="history-mode-panel" aria-labelledby={`${panelId}-title`}><h3 id={`${panelId}-title`} ref={modeHeading} tabIndex={-1} className="section-title">选择整理方式</h3><p className="history-secondary">先生成候选，再决定是否采用。新口述默认设置保持不变。</p><ModeSelector value={draftMode} onChange={setDraftMode} label="本条候选的整理方式"/>{source&&<ConfirmedContextEditor key={source.id} source={source} value={confirmedContext} onChange={setConfirmedContext} disabled={Boolean(busy)}/>}<div className="history-panel-actions"><Button variant="ghost" color="gray" onClick={()=>void closeMode()}>取消</Button><Button loading={busy==='candidate'} disabled={!desktop.available||Boolean(busy)} onClick={()=>void generate()}>生成候选</Button></div></section>}
  {candidate&&<section className="history-candidate" aria-labelledby={candidateId}>
   <div className="history-section-topline"><h3 ref={candidateHeading} tabIndex={-1} className="section-title" id={candidateId}>新候选</h3><ModeBadge mode={candidate.mode} resolved={candidate.resolved} name={candidate.modeName} sourceState="model"/></div>
   <p className="history-secondary">先核对改动，再保存你想保留的内容。</p>
   {candidateHints.length>0&&<div className="candidate-review-note" role="status"><InfoCircledIcon/><p>生成稿提示：{candidateHints.join('；')}。可保留原文或直接编辑后保存。</p></div>}
   {candidate.native.confirmedContext&&<ConfirmationSummary context={candidate.native.confirmedContext}/>}
   <CandidateComparison key={candidate.native.id} baseline={candidateBaseline} generated={candidate.texts.join('\n\n')} value={candidateText} onChange={setCandidateText} disabled={Boolean(busy)}/>
   {candidateHints.length>0&&<details className="candidate-source-reference"><summary>查看识别稿</summary><p>{record.source.join('\n\n')}</p></details>}
   <p className="history-candidate-warning">保存后更新这条记录，可撤销。其他应用中的文字需重新复制。</p>
   {stale&&<div className="candidate-review-note" role="status"><InfoCircledIcon/><p>当前版本已变化。你的草稿仍可编辑；重新生成后再保存。</p></div>}
   <div className="history-panel-actions"><Button variant="ghost" color="gray" disabled={Boolean(busy)} onClick={()=>void cancelCandidate(true)}>取消候选</Button>{stale&&<Button variant="soft" disabled={Boolean(busy)} onClick={()=>void generate(candidate.mode)}>重新生成</Button>}<Button aria-label="保存这个版本" loading={busy==='adopt'} disabled={stale||Boolean(busy)||editing||correcting||!candidateText.trim()||Array.from(candidateText).length>80_000} onClick={()=>void perform('adopt',saveCandidate,'已保存你的选择，可撤销。').catch(()=>{})}>保存这个版本</Button></div>
  </section>}
  {!output&&source?.complete&&<div className="history-mode-panel"><p>整理未完成，完整识别稿仍可使用。</p><Button disabled={Boolean(busy)||Boolean(candidate)} onClick={()=>void perform('source',async()=>{await desktop.command('result.useSource',{resultId:record.id,sessionId:record.id,expectedGeneration:record.detail?.session.generation,sourceRevisionId:source.id});await desktop.loadRecord(record.id);await desktop.refresh();},'已采用识别稿，请核对后复制。').catch(()=>{})}>使用识别稿</Button></div>}
  <Button size="2" variant="soft" color="gray" className="history-compare-toggle" aria-expanded={compareOpen} aria-controls={compareId} onClick={toggleComparison}><ReaderIcon/><strong>{compareOpen?'收起识别稿对照':'对照识别稿'}</strong><small>按需核对与恢复</small>{compareOpen?<ChevronUpIcon/>:<ChevronDownIcon/>}</Button>
  {compareOpen&&<section id={compareId} className="history-comparison"><h3 className="section-title">识别稿与当前文本</h3><p className="history-secondary">识别稿也可能听错。颜色仅表示字符差异，不判断意思是否正确；恢复只改本地结果。{fullDiff.granularity==='whole'&&'文本较长或改动较多，已整体标色，不表示每个字都被改动。'}</p><div className="history-sentence-texts history-full-comparison"><div className="history-sentence-line"><span>识别稿</span><p>{showDiff(fullDiff.source,'history-source-diff')}</p></div><div className="history-sentence-line"><span>当前文本</span><p>{showDiff(fullDiff.current,'history-current-diff')}</p></div></div>
   {output&&source&&<div className="history-restore-form"><h3 className="section-title">恢复选定内容</h3><p className="history-secondary">选择识别来源后，在当前文本中点击插入位置，或划选要替换的文字；确认预览后仅修改该位置。</p>
    <div className="history-sentence-texts"><div><label htmlFor="restore-source">识别来源</label><Select.Root value={restoreSource} onValueChange={chooseSource} disabled={Boolean(busy)||Boolean(candidate)||editing||correcting}><Select.Trigger id="restore-source" placeholder="选择识别段" aria-label="选择识别来源"/><Select.Content>{source.items.map((i,n)=><Select.Item key={i.id} value={i.id}>第 {n+1} 段 · {i.text.slice(0,32)}</Select.Item>)}</Select.Content></Select.Root>{item&&<><TextArea key={`${source.id}:${item.id}`} aria-label="选择识别稿中的恢复范围" value={item.text} readOnly disabled={Boolean(busy)||Boolean(candidate)||editing||correcting} rows={5} onSelect={e=>selectSource(e.currentTarget)} onPointerUp={e=>selectSource(e.currentTarget)} onKeyUp={e=>selectSource(e.currentTarget)}/><p className="history-secondary" role="status">{sourceSelection&&sourceSelection.range.end>sourceSelection.range.start?`已选来源 ${sourceSelection.range.end-sourceSelection.range.start} 字，可划选更小范围。`:'请在识别稿中划选要恢复的文字。'}</p></>}</div>
     <div><label htmlFor="restore-target">当前结果位置</label><Select.Root value={restoreBlock} onValueChange={chooseBlock} disabled={Boolean(busy)||Boolean(candidate)||editing||correcting}><Select.Trigger id="restore-target" placeholder="选择结果段" aria-label="选择恢复目标"/><Select.Content>{output.blocks.map((b,n)=><Select.Item key={b.id} value={b.id}>第 {n+1} 段 · {b.text.slice(0,32)}</Select.Item>)}</Select.Content></Select.Root>{block&&<><TextArea key={`${output.id}:${block.id}`} aria-label="选择当前文本中的插入或替换位置" value={block.text} readOnly disabled={Boolean(busy)||Boolean(candidate)||editing||correcting} rows={5} onSelect={e=>selectTarget(e.currentTarget)} onPointerUp={e=>selectTarget(e.currentTarget)} onKeyUp={e=>selectTarget(e.currentTarget)}/><p className="history-secondary" role="status">{targetSelection?targetSelection.range.start===targetSelection.range.end?'将在当前光标处插入，不替换其他文字。':`已选目标 ${targetSelection.range.end-targetSelection.range.start} 字，将仅替换这部分。`:'请在此文本中点击插入位置，或划选要替换的文字。'}</p></>}</div></div>
    {restorePreview&&<div className="history-restore-preview"><strong>{restorePreview.kind==='insert'?'插入后的目标段预览':'替换后的目标段预览'}</strong><p>{restorePreview.text}</p><Button disabled={Boolean(busy)||editing||correcting||restorePreview.text===block?.text} onClick={()=>{const patch=restorePreview.patch;void perform('restore',async()=>{await actions.restore(record,patch);setRestoreBlock('');setRestoreSource('');setSourceSelection(null);setTargetSelection(null);},'已恢复所选内容，其他文字保持不变，可撤销。').catch(()=>{});}}>{restorePreview.kind==='insert'?'确认插入这部分':'确认替换这部分'}</Button></div>}
   </div>}
  </section>}
 </section>;
}
