import {Fragment,useId,useMemo,useState} from 'react';
import {Button,TextArea} from '@radix-ui/themes';
import {CheckIcon,Pencil2Icon,ReaderIcon,ResetIcon} from '@radix-ui/react-icons';
import {candidateDiff,composeCandidate} from './candidateDiff';
import './CandidateComparison.css';

interface Props {baseline:string;generated:string;value:string;onChange:(text:string)=>void;disabled?:boolean}
export function CandidateComparison({baseline,generated,value,onChange,disabled=false}:Props) {
 const [target,setTarget]=useState(generated),[kept,setKept]=useState<Set<number>>(new Set()),[editing,setEditing]=useState(false);
 const diff=useMemo(()=>candidateDiff(baseline,target),[baseline,target]);
 const changes=diff.parts.filter(part=>part.kind==='change');
 const accepted=changes.length-kept.size,id=useId();
 function select(next:Set<number>){setKept(next);onChange(composeCandidate(diff,next));}
 function toggle(change:number){const next=new Set(kept);if(next.has(change))next.delete(change);else next.add(change);select(next);}
 function edit(text:string){
  // Earlier choices are materialized in value before rebasing on typed text.
  setTarget(text);setKept(new Set());onChange(text);
 }
 function reset(){setTarget(generated);setKept(new Set());onChange(generated);}
 return <div className="candidate-review">
  <div className="candidate-review-toolbar">
   <div className="candidate-view-switch" role="group" aria-label="候选查看方式">
    <Button size="2" variant="ghost" color="gray" aria-pressed={!editing} onClick={()=>setEditing(false)}><ReaderIcon/>改动对照</Button>
    <Button size="2" variant="ghost" color="gray" aria-pressed={editing} onClick={()=>setEditing(true)}><Pencil2Icon/>编辑文本</Button>
   </div>
   <span className="candidate-change-count" role="status">{changes.length?`采用 ${accepted} / ${changes.length} 处改动`:'与生成前相同'}</span>
  </div>
  {editing?<div className="candidate-text-editor">
   <label htmlFor={id}>最终文本</label>
   <TextArea id={id} aria-label="编辑候选文本" value={value} disabled={disabled} onChange={event=>edit(event.target.value)} rows={10}/>
   <p className="history-secondary">可直接修改文字，返回对照后查看新的差异。</p>
  </div>:<>
   <div className="candidate-diff-legend"><span><del>原文删改</del></span><span><ins>候选新增</ins></span><small>点击标记，切换采用或保留原文</small></div>
   {diff.granularity==='whole'&&<p className="history-secondary">本次按整段对照，可保留整段原文，也可进入“编辑文本”调整局部。</p>}
   <div className="candidate-inline-diff" role="region" aria-label="候选改动对照"><p>
    {diff.parts.map((part,index)=>part.kind==='same'?<Fragment key={index}>{part.text}</Fragment>:<span
     key={index} className="candidate-diff-change" data-choice={kept.has(part.id)?'original':'candidate'} data-disabled={disabled}
     title={kept.has(part.id)?'点击采用改动':'点击保留原文'} onClick={()=>{if(!disabled)toggle(part.id);}}>
      {part.before&&(kept.has(part.id)?<span className="candidate-kept-text">{part.before}</span>:<del>{part.before}</del>)}
      {part.after&&(kept.has(part.id)?<del className="candidate-rejected-text">{part.after}</del>:<ins>{part.after}</ins>)}
      <button type="button" className="candidate-choice-toggle" disabled={disabled} aria-pressed={!kept.has(part.id)}
       aria-label={`第 ${part.id+1} 处改动：${kept.has(part.id)?'已保留原文，点击采用改动':'已采用改动，点击保留原文'}`}
       onClick={event=>{event.stopPropagation();toggle(part.id);}}><span key={kept.has(part.id)?'original':'candidate'} className="candidate-choice-icon" aria-hidden="true">{kept.has(part.id)?<ResetIcon/>:<CheckIcon/>}</span></button>
     </span>)}
   </p></div>
   {changes.length>0&&<div className="candidate-bulk-actions">
    <Button size="2" variant="ghost" color="gray" disabled={disabled||accepted===changes.length} onClick={()=>select(new Set())}>全部采用</Button>
    <Button size="2" variant="ghost" color="gray" disabled={disabled||kept.size===changes.length} onClick={()=>select(new Set(changes.map(part=>part.id)))}>保留全部原文</Button>
   </div>}
  </>}
  {value!==generated&&<Button size="2" variant="ghost" color="gray" className="candidate-reset" disabled={disabled} onClick={reset}><ResetIcon/>重置为生成候选</Button>}
 </div>;
}
