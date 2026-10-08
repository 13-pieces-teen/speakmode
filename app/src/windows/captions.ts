import type {CaptionItem,LiveSession} from '../services/contracts';

/** Merge a complete DTO snapshot without accepting stale revisions or changing final items. */
export function mergeCaptions(previous:readonly CaptionItem[],incoming:readonly CaptionItem[]):CaptionItem[] {
 const items=new Map(previous.map(item=>[item.id,item]));let changed=false;
 for(const item of incoming){
  const old=items.get(item.id);
  if(old?.final){
   if(item.captureOrder!==old.captureOrder){items.set(item.id,{...old,captureOrder:item.captureOrder});changed=true;}
   continue;
  }
  if(old&&item.revision<old.revision)continue;
  if(old&&item.revision===old.revision&&!item.final){
   if(item.captureOrder!==old.captureOrder){items.set(item.id,{...old,captureOrder:item.captureOrder});changed=true;}
   continue;
  }
  if(old&&item.revision===old.revision&&item.final===old.final&&item.text===old.text&&item.captureOrder===old.captureOrder)continue;
  // The provider assembler may resolve a predecessor late; ordering remains authoritative.
  items.set(item.id,{...item});changed=true;
 }
 if(!changed)return previous as CaptionItem[];
 return [...items.values()].sort((a,b)=>a.captureOrder-b.captureOrder);
}

/** Older desktop builds remain readable, but cannot provide a stable per-item prefix. */
export function captionSnapshot(session:LiveSession|null,previous:readonly CaptionItem[]):CaptionItem[] {
 if(session?.captions!==undefined)return session.captions;
 const text=session?.partial||session?.transcript||session?.items?.map(item=>item.text).join(' ')||'';
 if(!text)return [];
 const old=previous.find(item=>item.id==='legacy-caption');
 return [{id:'legacy-caption',text,revision:(old?.revision??0)+(old?.text===text?0:1),final:false,captureOrder:0}];
}

export function hasNewFinal(previous:readonly CaptionItem[],next:readonly CaptionItem[]):boolean {
 const old=new Map(previous.map(item=>[item.id,item]));
 return next.some(item=>item.final&&!old.get(item.id)?.final);
}

/** Full data is retained separately. The collapsed strip only renders a bounded tail. */
export function captionTail(items:readonly CaptionItem[],maxItems=8,maxChars=1200):CaptionItem[] {
 const result:CaptionItem[]=[];let remaining=maxChars;
 for(let i=items.length-1;i>=0&&result.length<maxItems&&remaining>0;i--){
  const item=items[i],points=Array.from(item.text);
  if(!points.length)continue;
  const clipped=points.length>remaining;
  result.unshift(clipped?{...item,text:'…'+points.slice(-remaining).join('')}:item);
  remaining-=Math.min(points.length,remaining);
 }
 return result;
}

/** Backspacing a short tentative tail must not pull the confirmed prefix to the right. */
export function followCaptionOffset(previous:number,end:number,viewportWidth:number):number {
 if(end===0)return 0; // A complete short correction must never sit outside the strip.
 // Retain at least 65% of the strip when a provider replaces a much longer draft.
 // Such a large correction cannot keep both a fixed anchor and a visible latest tail.
 return Math.min(Math.max(previous,end),end+viewportWidth*.35);
}
