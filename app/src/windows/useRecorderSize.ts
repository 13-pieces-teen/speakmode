import {useLayoutEffect,useRef,useState,type RefObject} from 'react';
import type {useDesktop} from '../services/DesktopProvider';

type Command = ReturnType<typeof useDesktop>['command'];
type RecorderSize = {width:number;height:number;menuOpen:boolean};
const same = (left:RecorderSize|null,right:RecorderSize) => !!left && left.width===right.width && left.height===right.height && left.menuOpen===right.menuOpen;

/** Requests a bounded logical-pixel size; Rust owns the actual recording window and work-area clamp. */
export function useRecorderSize(assembly:RefObject<HTMLElement|null>,menuElement:HTMLElement|null,menuOpen:boolean,available:boolean,command:Command) {
 const [error,setError]=useState('');
 const lastSent=useRef<RecorderSize|null>(null),desired=useRef<RecorderSize|null>(null),sending=useRef(false),mounted=useRef(true);
 useLayoutEffect(()=>{
  mounted.current=true;
  if(!available||!assembly.current)return;
  let frame=0,active=true;
  async function sendLatest(){
   if(sending.current)return;sending.current=true;
   try {
    while(mounted.current&&desired.current&&!same(lastSent.current,desired.current)){
     const next=desired.current;lastSent.current=next;
     try {await command('window.resizeRecorder',next);if(mounted.current)setError('');}
     catch {if(mounted.current)setError('浮层尺寸调整失败，可打开主面板完成操作。');}
    }
   }finally{sending.current=false;}
  }
  function measure(){
   if(!active||!assembly.current)return;
   const surface=assembly.current.parentElement;if(!surface)return;
   const style=getComputedStyle(surface),padding=parseFloat(style.paddingTop)+parseFloat(style.paddingBottom);
   const baseHeight=assembly.current.getBoundingClientRect().height+padding;
   // scrollHeight measures unclipped menu content even before the native window has grown.
   const menuHeight=menuOpen&&menuElement?Math.min(360,menuElement.scrollHeight+2):0;
   desired.current={width:440,height:Math.min(420,Math.max(86,Math.ceil(baseHeight+(menuHeight?menuHeight+10:0)))),menuOpen};
   void sendLatest();
  }
  const schedule=()=>{cancelAnimationFrame(frame);frame=requestAnimationFrame(measure);};
  const observer=new ResizeObserver(schedule);observer.observe(assembly.current);
  if(menuElement)observer.observe(menuElement);
  window.addEventListener('resize',schedule);schedule();
  return()=>{active=false;mounted.current=false;observer.disconnect();cancelAnimationFrame(frame);window.removeEventListener('resize',schedule);};
 },[assembly,menuElement,menuOpen,available,command]);
 return error;
}


