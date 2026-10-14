import {useId,useState} from 'react';
import {Button,Select,TextArea,TextField} from '@radix-ui/themes';
import type {ConfirmedContext,SourceRevision,SourceCorrection} from '../services/contracts';
import {captureRestoreSelection,type RestoreSelection} from './restoreSelection';
import {moveCorrectionSelection} from './correctionSelection';

export function ConfirmationSummary({context}:{context:ConfirmedContext}) {
 return <details className="candidate-source-reference"><summary>本次整理参考 · {context.language==='english'?'英文成稿':'跟随口述'} · {context.corrections.length} 处确认</summary>
  <p className="history-secondary">以下是你确认的识别内容，原始识别稿仍保留供对照。</p>
  <ul>{context.corrections.map(c=><li key={`${c.start}:${c.end}`}><q>{c.expectedText}</q> → <strong>{c.replacement}</strong></li>)}</ul>
 </details>;
}

export function ConfirmedContextEditor({source,value,onChange,disabled}:{source:SourceRevision;value:ConfirmedContext|undefined;onChange:(value:ConfirmedContext|undefined)=>void;disabled:boolean}) {
 const id=useId(),text=source.items.map(i=>i.text).join('\n');
 const [selection,setSelection]=useState<RestoreSelection|null>(null),[replacement,setReplacement]=useState('');
 const context:ConfirmedContext=value??{sourceRevisionId:source.id,language:'asSpoken',corrections:[]};
 const range=selection?.range;
 const selected=range?Array.from(text).slice(range.start,range.end).join(''):'';
 const valid=selection?.text===text&&selection.revisionId===source.id&&range&&range.end>range.start&&range.end-range.start<=240
  &&context.corrections.length<8&&!context.corrections.some(c=>range.start<c.end&&range.end>c.start)
  &&replacement.trim()&&Array.from(replacement.trim()).length<=240&&!/[\p{Cc}\p{Cf}\p{Cs}\u2028\u2029]/u.test(replacement)&&selected!==replacement.trim();
 function update(next:ConfirmedContext){onChange(next.language==='asSpoken'&&!next.corrections.length?undefined:next);}
 function select(element:HTMLTextAreaElement){setSelection(captureRestoreSelection({id:source.id,revisionId:source.id,text},element.selectionStart,element.selectionEnd));}
 function add(){if(!valid||!range)return;const correction:SourceCorrection={start:range.start,end:range.end,expectedText:selected,replacement:replacement.trim()};update({...context,corrections:[...context.corrections,correction].sort((a,b)=>a.start-b.start)});setSelection(null);setReplacement('');}
 return <div className="confirmed-context-editor">
  <label htmlFor={`${id}-language`}>成稿语言</label>
  <Select.Root value={context.language} disabled={disabled} onValueChange={language=>update({...context,language:language as ConfirmedContext['language']})}>
   <Select.Trigger id={`${id}-language`} aria-label="本条成稿语言"/><Select.Content><Select.Item value="asSpoken">跟随口述</Select.Item><Select.Item value="english">英文成稿</Select.Item></Select.Content>
  </Select.Root>
  <details><summary>确认识别内容{context.corrections.length>0?`（${context.corrections.length} 处）`:''}</summary>
   <p className="history-secondary">有听错的专名或年份？划选原文，填入你确认的内容。可以跨段选择，每处最多 240 字，共 8 处。只用于这条记录。</p>
   <TextArea aria-label="选择需要确认的识别内容" value={text} readOnly disabled={disabled} rows={7} onSelect={e=>select(e.currentTarget)} onPointerUp={e=>select(e.currentTarget)} onKeyDown={e=>{
    const el=e.currentTarget,next=moveCorrectionSelection(el.value,el.selectionStart,el.selectionEnd,el.selectionDirection,e.key,e.shiftKey,e.ctrlKey||e.altKey||e.metaKey||e.nativeEvent.isComposing);
    if(next){e.preventDefault();el.setSelectionRange(next.start,next.end,next.direction);select(el);}
   }}/>
   <p className="history-secondary" role="status">{selected?`已选：${selected}`:'请先划选一处需要纠正的内容。'}</p>
   <TextField.Root aria-label="这处识别内容的正确表达" placeholder="填写你确认的正确内容" value={replacement} disabled={disabled} onChange={e=>setReplacement(e.target.value)}/>
   <Button size="2" variant="soft" disabled={disabled||!valid} onClick={add}>确认这一处</Button>
   {context.corrections.length>0&&<ul>{context.corrections.map(c=><li key={`${c.start}:${c.end}`}><span><q>{c.expectedText}</q> → <strong>{c.replacement}</strong></span> <Button size="1" variant="ghost" color="gray" disabled={disabled} aria-label={`移除确认：${c.replacement}`} onClick={()=>update({...context,corrections:context.corrections.filter(other=>other!==c)})}>移除</Button></li>)}</ul>}
   <p className="history-secondary">生成后核对完整候选，再保存到当前记录。确认内容不会加入通用词库。</p>
  </details>
 </div>;
}
