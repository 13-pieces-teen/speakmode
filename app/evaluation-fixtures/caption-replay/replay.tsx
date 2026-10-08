/** TEST ONLY. Old 0.1.1 presentation algorithm and the production increment receive the same React state. */
import {useLayoutEffect,useRef,useState} from 'react';
import {createRoot} from 'react-dom/client';
import {CaptionStrip} from '../../src/windows/CaptionStrip';
import type {CaptionItem,LiveSession} from '../../src/services/contracts';
import '../../src/windows/recorder.css';

function LegacyCaption({text}:{text:string}){
 const [expanded,setExpanded]=useState(false),[paused,setPaused]=useState(false),[offset,setOffset]=useState(0);
 const viewport=useRef<HTMLButtonElement>(null),track=useRef<HTMLSpanElement>(null);
 useLayoutEffect(()=>{
  if(!viewport.current||!track.current||paused||expanded)return;
  const measure=()=>setOffset(Math.max(0,(track.current?.scrollWidth??0)-(viewport.current?.clientWidth??0)));
  measure();const observer=new ResizeObserver(measure);observer.observe(viewport.current);observer.observe(track.current);return()=>observer.disconnect();
 },[text,paused,expanded]);
 if(!text)return null;
 return <div className="recorder-caption" data-expanded={expanded}><button ref={viewport} className="recorder-caption-viewport" aria-label="旧版字幕" aria-expanded={expanded} onClick={()=>setExpanded(!expanded)} onPointerEnter={()=>setPaused(true)} onPointerLeave={()=>setPaused(false)} onFocus={()=>setPaused(true)} onBlur={()=>setPaused(false)}><span ref={track} className="recorder-caption-track" style={{transform:expanded?'none':`translateX(-${offset}px)`}}>{text}</span></button></div>;
}
const style=document.createElement('style');style.textContent=`body{margin:0;padding:20px 14px;background:#f7f9fc;font-family:"Segoe UI","Microsoft YaHei",sans-serif;color:#20354c}h1{font-size:16px;margin:0 0 10px}.case{width:412px;margin:18px 0 28px}.case h2{font-size:12px;font-weight:600;margin:0 0 6px}.legacy .recorder-caption-track{transition:transform 260ms cubic-bezier(.2,.7,.2,1)}@media(prefers-reduced-motion:reduce){.legacy .recorder-caption-track{transition:none}}`;document.head.appendChild(style);
function Replay(){
 const [session,setSession]=useState<LiveSession>({sessionId:'replay',generation:1,state:'recording',captions:[]});
 const timings=useRef<Record<string,unknown>[]>([]),emitted=useRef(0),sentAt=useRef(0);
 (window as any).__CAPTION_REPLAY__={
  emit(captions:CaptionItem[],state='recording',generation=1){emitted.current++;sentAt.current=performance.now();setSession({sessionId:'replay',generation,state,captions,partial:captions.map(item=>item.text).join(' ')});},
  timings:timings.current,
  events:()=>emitted.current,
 };
 useLayoutEffect(()=>{
  const observe=(name:string)=>{
   const root=document.querySelector(`.${name}`)!;
   const observer=new MutationObserver(()=>{
    const text=root.querySelector('.recorder-caption-track')?.textContent??'';
    if(text)timings.current.push({view:name,event:emitted.current,commitProxyMs:performance.now()-sentAt.current,textLength:text.length});
   });observer.observe(root,{subtree:true,childList:true,characterData:true,attributes:true,attributeFilter:['data-caption-revision']});return observer;
  };
  const old=observe('legacy'),next=observe('increment');return()=>{old.disconnect();next.disconnect();};
 },[]);
 return <><h1>同事件字幕回放 · 合成测试</h1><section className="case legacy"><h2>0.1.1 全文宽度跟随算法</h2><LegacyCaption key={session.generation} text={session.partial??''}/></section><section className="case increment"><h2>本版 item / revision 稳定尾段</h2><CaptionStrip key={session.generation} identity={`replay:${session.generation}`} session={session}/></section></>;
}
createRoot(document.getElementById('root')!).render(<Replay/>);
