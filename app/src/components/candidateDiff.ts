import {diffArrays} from 'diff';

export type CandidateDiffPart =
 | {kind:'same';text:string}
 | {kind:'change';id:number;before:string;after:string};
export interface CandidateDiff {parts:CandidateDiffPart[];granularity:'characters'|'whole'|'unchanged'}

// Local document choices, not ASR alignment or native restoration ranges.
export function candidateDiff(before:string,after:string):CandidateDiff {
 const whole=():CandidateDiff=>before===after
  ? {parts:[{kind:'same',text:before}],granularity:'unchanged'}
  : {parts:[{kind:'change',id:0,before,after}],granularity:'whole'};
 if(before===after||!before||!after)return whole();
 if(before.length>24_000||after.length>24_000)return whole();
 const a=Array.from(before),b=Array.from(after);
 if(a.length>12_000||b.length>12_000||a.length*b.length>16_000_000)return whole();
 const changes=diffArrays(a,b,{maxEditLength:2048,timeout:16});
 if(!changes)return whole();
 const parts:CandidateDiffPart[]=[];
 let removed='',added='',id=0;
 const flush=()=>{
  if(removed||added)parts.push({kind:'change',id:id++,before:removed,after:added});
  removed='';added='';
 };
 for(let i=0;i<changes.length;i++){
  const change=changes[i],text=change.value.join('');
  if(change.removed)removed+=text;
  else if(change.added)added+=text;
  else if((removed||added)&&change.value.length<=6&&!/[\r\n。！？.!?]/u.test(text)&&(changes[i+1]?.added||changes[i+1]?.removed)){
   removed+=text;added+=text;
  }else{flush();parts.push({kind:'same',text});}
 }
 flush();
 if(id>160)return whole();
 return {parts,granularity:'characters'};
}

export function composeCandidate(diff:CandidateDiff,keepOriginal:ReadonlySet<number>):string {
 return diff.parts.map(part=>part.kind==='same'?part.text:keepOriginal.has(part.id)?part.before:part.after).join('');
}
