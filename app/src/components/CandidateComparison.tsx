import {Fragment,useId,useMemo,useState} from 'react';
import {Button} from '@radix-ui/themes';
import {ChevronDownIcon,ChevronUpIcon} from '@radix-ui/react-icons';
import {characterDiff,type Dictation} from '../model';
import {RecordText} from './ui';

// Display-only correspondence. Adoption and restoration continue to use native
// revision/source IDs; a character diff must never become a restore patch.
export function CandidateComparison({baseline,preview}:{baseline:string;preview:Dictation}) {
 const [expanded,setExpanded]=useState(false),id=useId();
 const target=preview.current.join('\n\n');
 const diff=useMemo(()=>expanded?characterDiff(baseline,target):null,[expanded,baseline,target]);
 const render=(segments:ReturnType<typeof characterDiff>['source'],className:string)=>segments.map((part,i)=>part.changed?<mark className={className} key={i}>{part.text}</mark>:<Fragment key={i}>{part.text}</Fragment>);
 return <div className="candidate-review">
  <Button size="2" variant="soft" color="gray" aria-expanded={expanded} aria-controls={id} onClick={()=>setExpanded(value=>!value)}>{expanded?'收起改动对照':'对比生成前文本'}{expanded?<ChevronUpIcon/>:<ChevronDownIcon/>}</Button>
  {expanded&&diff?<div id={id} role="region" aria-label="候选改动对照">
   <p className="history-secondary">{diff.granularity==='unchanged'?'两份文本相同。清楚的短句无需为了区分模式而改写。':diff.granularity==='whole'?'文本较长或改动较多，已展示完整文本并整体标色，不表示每个字都被改动。':'暖色标出生成前被删改的文字，蓝色标出候选新增或改写的文字。颜色不判断原意是否正确。'}</p>
   <div className="history-sentence-texts"><div className="history-sentence-line"><span>生成前文本</span><p>{render(diff.source,'history-source-diff')}</p></div><div className="history-sentence-line"><span>新候选</span><p>{render(diff.current,'history-current-diff')}</p></div></div>
  </div>:<RecordText record={preview}/>}
 </div>;
}
