import {useCallback,useEffect,useId,useRef,useState,type KeyboardEvent} from 'react';
import {Button,Checkbox,Select,TextArea,TextField} from '@radix-ui/themes';
import type {Dictation} from '../model';
import {useDesktop} from '../services/DesktopProvider';
import {useDictations} from '../hooks/useDictations';
import {errorMessage} from '../services/bridge';
import {dictionarySearchKey} from '../dictionary';
import {captureRestoreSelection,type RestoreSelection} from './restoreSelection';
import {buildCorrectionPreview,correctionTermError,moveCorrectionSelection} from './correctionSelection';
import {useNavigationGuard} from './NavigationGuard';
import {focusInView} from './focusInView';

export function TermCorrectionPanel({record,onClose,onSaved}:{record:Dictation;onClose:()=>void;onSaved:(message:string)=>void}) {
 const desktop=useDesktop(),actions=useDictations(),navigation=useNavigationGuard();
 const [baseline,setBaseline]=useState(record),[blockId,setBlockId]=useState(record.detail?.output?.blocks[0]?.id??'');
 const [selection,setSelection]=useState<RestoreSelection|null>(null),[replacement,setReplacement]=useState(''),[remember,setRemember]=useState(false);
 const [dictionaryVersion,setDictionaryVersion]=useState(desktop.dictionary?.version),[busy,setBusy]=useState(false),[error,setError]=useState('');
 const heading=useRef<HTMLHeadingElement>(null),busyRef=useRef(false),mounted=useRef(true),titleId=useId(),inputId=useId(),checkboxId=useId();
 const output=baseline.detail?.output,block=output?.blocks.find(item=>item.id===blockId);
 const target=block&&output?{id:block.id,text:block.text,revisionId:output.id}:undefined;
 const preview=buildCorrectionPreview(target,selection,replacement);
 const stale=baseline.id!==record.id||baseline.revision!==record.revision;
 const dictionaryStale=remember&&dictionaryVersion!==desktop.dictionary?.version;
 const termError=replacement?correctionTermError(replacement):'';
 const exists=desktop.dictionary?.entries.some(entry=>dictionarySearchKey(entry.term)===dictionarySearchKey(replacement.trim()))??false;
 const full=remember&&!exists&&(desktop.dictionary?.entries.length??0)>=200;
 // Native Unicode case-folding is authoritative for duplicates (e.g. ß / SS).
 // A full dictionary can still accept an existing term without adding an entry.
 const canSave=desktop.available&&!!preview&&!stale&&!dictionaryStale&&!termError&&(!remember||dictionaryVersion!==undefined);
 const dirty=Boolean(replacement||remember||selection&&selection.range.end>selection.range.start);
 useEffect(()=>{mounted.current=true;focusInView(heading.current);return()=>{mounted.current=false;navigation.register(null);};},[navigation.register]);
 const save=useCallback(async()=>{
  if(busyRef.current)throw new Error('正在保存，请稍候。');
  if(!canSave||!preview)throw new Error(stale?'当前文本已变化，请重新选择词条。':dictionaryStale?'词典已变化，请先更新词典状态。':'请划选要纠正的词，并填写有效的新写法。');
  busyRef.current=true;setBusy(true);setError('');
  try {
   const result=await actions.correctTerm(baseline,preview.patch,remember,dictionaryVersion);
   if(mounted.current){
    const message=result.dictionaryStatus==='added'?'已纠正这一处并记入词典；从下次口述开始生效。':result.dictionaryStatus==='existing'?'已纠正这一处；词典已有此词，沿用原有写法。':'已纠正这一处。';
    onSaved(message+' 可撤销文本修改；其他应用中的文本未改变。');
   }
  }catch(error){if(mounted.current)setError(errorMessage(error));void desktop.refresh().catch(()=>{});void desktop.loadRecord(baseline.id).catch(()=>{});throw error;}
  finally{busyRef.current=false;if(mounted.current)setBusy(false);}
 },[actions,baseline,canSave,desktop,dictionaryStale,dictionaryVersion,onSaved,preview,remember,stale]);
 useEffect(()=>{navigation.register({dirty:dirty||busy,pending:busy,save,discard:onClose});return()=>navigation.register(null);},[dirty,busy,save,onClose,navigation.register]);
 function choose(id:string){setBlockId(id);setSelection(null);setError('');}
 function capture(element:HTMLTextAreaElement){setSelection(target?captureRestoreSelection(target,element.selectionStart,element.selectionEnd):null);}
 function selectByKeyboard(event:KeyboardEvent<HTMLTextAreaElement>){
  const element=event.currentTarget,next=moveCorrectionSelection(element.value,element.selectionStart,element.selectionEnd,element.selectionDirection,event.key,event.shiftKey,event.ctrlKey||event.altKey||event.metaKey||event.nativeEvent.isComposing);
  if(!next)return;event.preventDefault();element.setSelectionRange(next.start,next.end,next.direction);capture(element);
 }
 function reselect(){setBaseline(record);setBlockId(record.detail?.output?.blocks[0]?.id??'');setSelection(null);setError('');}
 const selected=selection?Array.from(selection.text).slice(selection.range.start,selection.range.end).join(''):'';
 return <section className="history-term-correction" aria-labelledby={titleId} aria-busy={busy}>
  <div className="history-section-topline"><h3 id={titleId} ref={heading} tabIndex={-1} className="section-title">纠正一个词</h3><span className="history-secondary">仅修改选中的这一处</span></div>
  <p className="history-secondary">划选需要纠正的词，输入正确写法。</p>
  {output&&output.blocks.length>1&&<Select.Root value={blockId} onValueChange={choose} disabled={busy}><Select.Trigger aria-label="选择纠词目标段"/><Select.Content>{output.blocks.map((item,index)=><Select.Item key={item.id} value={item.id}>第 {index+1} 段 · {item.text.slice(0,28)}</Select.Item>)}</Select.Content></Select.Root>}
  {block&&<TextArea className="history-correction-source" key={`${output?.id}:${blockId}`} aria-label="划选当前文本中要纠正的词" readOnly value={block.text} rows={5} disabled={busy||stale} onSelect={event=>capture(event.currentTarget)} onPointerUp={event=>capture(event.currentTarget)} onKeyDown={selectByKeyboard} onKeyUp={event=>capture(event.currentTarget)}/>}
  <p className="history-correction-selection" role="status">{selected?<>已选：<strong>{selected}</strong>{Array.from(selected).length>80?' · 请选择 80 字以内的词条。':''}</>:'支持鼠标划选，也可以用 Shift + ← / → 选择。'}</p>
  <label className="history-correction-label" htmlFor={inputId}>正确写法</label><TextField.Root id={inputId} value={replacement} disabled={busy} placeholder="例如 SpeakMode" onChange={event=>{setReplacement(event.target.value);setError('');}} maxLength={160} aria-describedby={`${inputId}-hint`} autoComplete="off"/>
  <p id={`${inputId}-hint`} className="history-secondary">{termError}</p>
  <label className="history-correction-remember" htmlFor={checkboxId}><Checkbox id={checkboxId} checked={remember} disabled={busy||!desktop.dictionary} onCheckedChange={value=>{setRemember(value===true);if(value===true)setDictionaryVersion(desktop.dictionary?.version);}}/><span><strong>记住这个写法</strong><small>{record.temporary?'本次正文不保存；勾选后，这个词会单独保存在个人词典。':'加入个人词典，从下一次口述开始提供识别提示。'}</small></span></label>
  {remember&&<p className="history-secondary">{exists?'词典中已有此词。':'撤销文本修改不会删除已记住的词。'}</p>}
  {full&&<p className="history-secondary" role="status">词典已满，仍可使用已有词条；如果是新词，本次文字与词典均不会保存。可取消“记住这个写法”只纠正文稿。</p>}
  {stale&&<div className="history-correction-conflict" role="alert"><p>当前文本已有新版本，草稿仍保留。请重新选择要纠正的位置。</p><Button variant="soft" color="gray" disabled={busy} onClick={reselect}>重新选择当前版本</Button></div>}
  {dictionaryStale&&<div className="history-correction-conflict" role="alert"><p>词典已更新，当前纠词草稿仍保留。</p><Button variant="soft" color="gray" disabled={busy} onClick={()=>setDictionaryVersion(desktop.dictionary?.version)}>使用最新词典状态</Button></div>}
  {preview&&!stale&&<div className="history-correction-preview"><span className="history-tool-label">修改后预览</span><p>{preview.text}</p></div>}
  {error&&<p className="inline-error" role="alert">{error}</p>}
  <div className="history-panel-actions"><Button variant="soft" color="gray" disabled={busy} onClick={onClose}>取消纠正</Button><Button loading={busy} disabled={!canSave||busy} onClick={()=>void save().catch(()=>{})}>{remember?'纠正并记住':'保存这一处'}</Button></div>
 </section>;
}
