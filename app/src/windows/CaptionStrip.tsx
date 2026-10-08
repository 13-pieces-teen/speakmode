import {memo,useLayoutEffect,useRef,useState} from 'react';
import type {CaptionItem,LiveSession} from '../services/contracts';
import {captionSnapshot,captionTail,followCaptionOffset,hasNewFinal,mergeCaptions} from './captions';

function useCaptionItems(identity:string,session:LiveSession|null){
 const latest=useRef<{identity:string;items:CaptionItem[]}>({identity,items:[]});
 const frame=useRef(0),[display,setDisplay]=useState(latest.current);
 useLayoutEffect(()=>{
  if(latest.current.identity!==identity)latest.current={identity,items:[]};
  const phase=(session?.state??'idle').toLowerCase(),previous=latest.current.items;
  const next=['idle','cancelled'].includes(phase)?[]:mergeCaptions(previous,captionSnapshot(session,previous));
  const critical=hasNewFinal(previous,next)||phase!=='recording'||!!session?.error;
  latest.current={identity,items:next};
  const commit=()=>{frame.current=0;setDisplay(old=>old.identity===latest.current.identity&&old.items===latest.current.items?old:latest.current);};
  if(critical){cancelAnimationFrame(frame.current);commit();}
  else if(!frame.current)frame.current=requestAnimationFrame(commit);
 },[identity,session?.captions,session?.partial,session?.transcript,session?.items,session?.state,session?.error]);
 useLayoutEffect(()=>()=>{cancelAnimationFrame(frame.current);frame.current=0;},[]);
 return display.identity===identity?display.items:[];
}

const Segment=memo(function Segment({item}:{item:CaptionItem}){
 return <span className="recorder-caption-segment" data-caption-id={item.id} data-caption-revision={item.revision} data-final={item.final}>{item.text}</span>;
});

/** The caption interaction owns a reading snapshot; it never freezes provider intake. */
export function CaptionStrip({identity,session,reportRender}:{identity:string;session:LiveSession|null;reportRender?:(token:string,asrAttemptId:string|null)=>void}){
 const items=useCaptionItems(identity,session);
 const [expanded,setExpanded]=useState(false),[hovered,setHovered]=useState(false),[focused,setFocused]=useState(false),[held,setHeld]=useState<CaptionItem[]|null>(null),[offset,setOffset]=useState(0);
 const viewport=useRef<HTMLButtonElement>(null),track=useRef<HTMLSpanElement>(null),previousStart=useRef('');
 const renderAcks=useRef(new Set<string>()),report=useRef(reportRender);report.current=reportRender;
 const holding=expanded||hovered||focused,visible=holding&&held?held:items;
 const hasCaption=items.some(item=>item.text);
 const shown=expanded?visible:captionTail(visible);
 const hasDisplayedCaption=shown.some(item=>item.text.trim());
 const start=shown.length?`${shown[0].id}:${shown[0].text.startsWith('…')?shown[0].text.length:0}`:'';
 const newCaptions=holding&&held!==null&&held!==items;
 useLayoutEffect(()=>{setExpanded(false);setHovered(false);setFocused(false);setHeld(null);setOffset(0);previousStart.current='';renderAcks.current.clear();},[identity]);
 useLayoutEffect(()=>{
  if(!holding)setHeld(null);
  else if(held===null)setHeld(items);
 },[holding,held,items]);
 useLayoutEffect(()=>{
  if(!viewport.current||!track.current||holding)return;
  const measure=()=>{
   if(!viewport.current||!track.current)return;
   const width=viewport.current.clientWidth,end=Math.max(0,track.current.scrollWidth-width),reset=previousStart.current!==start;
   previousStart.current=start;
   setOffset(old=>reset?end:followCaptionOffset(old,end,width));
  };
  measure();const observer=new ResizeObserver(measure);observer.observe(viewport.current);observer.observe(track.current);
  return()=>observer.disconnect();
 },[items,holding,start]);
 useLayoutEffect(()=>{
  const token=session?.captionRenderToken,attempt=session?.asrAttemptId??null;
  if(!token||!hasDisplayedCaption||!report.current)return;
  const key=`${identity}:${attempt??''}:${token}`;if(renderAcks.current.has(key))return;
  let frame=0;
  const schedule=()=>{
   if(document.visibilityState!=='visible'||frame)return;
   frame=requestAnimationFrame(()=>{
    frame=0;
    if(!viewport.current?.getClientRects().length||!track.current?.textContent?.trim()||document.visibilityState!=='visible'||renderAcks.current.has(key))return;
    renderAcks.current.add(key);
    if(renderAcks.current.size>8)renderAcks.current.delete(renderAcks.current.values().next().value!);
    report.current?.(token,attempt);
   });
  };
  schedule();document.addEventListener('visibilitychange',schedule);
  return()=>{cancelAnimationFrame(frame);document.removeEventListener('visibilitychange',schedule);};
 },[identity,session?.captionRenderToken,session?.asrAttemptId,hasDisplayedCaption]);
 if(!hasCaption)return null;
 const hold=()=>setHeld(old=>old??items);
 function resume(){setExpanded(false);setHovered(false);setFocused(false);setHeld(null);previousStart.current='';viewport.current?.blur();}
 return <div className="recorder-caption" data-expanded={expanded} data-following={!holding}>
  <button ref={viewport} className="recorder-caption-viewport" aria-label="查看当前识别字幕；点击展开或收起" aria-expanded={expanded}
   onClick={()=>{hold();setExpanded(!expanded);}}
   onPointerEnter={()=>{hold();setHovered(true);}} onPointerLeave={()=>setHovered(false)}
   onFocus={event=>{if(event.currentTarget.matches(':focus-visible')){hold();setFocused(true);}}} onBlur={()=>setFocused(false)}>
   <span ref={track} className="recorder-caption-track" style={{transform:expanded?'none':`translateX(-${offset}px)`}}>
    {shown.map(item=><Segment key={item.id} item={item}/>)}
   </span>
  </button>
  {expanded&&<div className="recorder-caption-reading" aria-hidden={!newCaptions}>{newCaptions&&<><span>有新字幕 · 阅读位置已保留</span><button type="button" onClick={resume}>返回实时</button></>}</div>}
 </div>;
}
