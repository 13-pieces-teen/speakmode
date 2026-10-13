import {useLayoutEffect,useRef,useState,type RefObject} from 'react';
import type {useDesktop} from '../services/DesktopProvider';

type Command = ReturnType<typeof useDesktop>['command'];
type RecorderSize = {width:number;height:number;menuOpen:boolean};
const same = (left:RecorderSize|null,right:RecorderSize) => !!left && left.width===right.width && left.height===right.height && left.menuOpen===right.menuOpen;

/** Requests a bounded logical-pixel size; Rust owns the actual recording window and work-area clamp. */
export function useRecorderSize(assembly:RefObject<HTMLElement|null>,menuElement:HTMLElement|null,menuOpen:boolean,available:boolean,command:Command) {
 const [error,setError]=useState('');
 const [readyFor,setReadyFor]=useState<HTMLElement|null>(null);
 const lastSent=useRef<RecorderSize|null>(null),desired=useRef<RecorderSize|null>(null),sending=useRef(false),mounted=useRef(true);
 useLayoutEffect(()=>{
  mounted.current=true;
  if(!available||!assembly.current)return;
  let frame=0,active=true,settlingFrames=0;
  let observedPositioner:HTMLElement|null=null;
  async function sendLatest(){
   if(sending.current)return;sending.current=true;
   try {
    while(mounted.current&&desired.current&&!same(lastSent.current,desired.current)){
     const next=desired.current;lastSent.current=next;
     try {await command('window.resizeRecorder',next);if(mounted.current){setError('');schedule();}}
     catch {if(mounted.current)setError('浮层尺寸调整失败，可打开主面板完成操作。');}
    }
   }finally{sending.current=false;}
  }
  function measure(){
   if(!active||!assembly.current)return;
   const surface=assembly.current.parentElement;if(!surface)return;
   const style=getComputedStyle(surface),padding=parseFloat(style.paddingTop)+parseFloat(style.paddingBottom);
   const baseHeight=assembly.current.getBoundingClientRect().height+padding;
   // Reserve the menu above the toolbar, including captions/errors below it. Only
   // the options list scrolls, and measuring layout height ignores the enter transform.
   let menuHeight=0;
   if(menuOpen&&menuElement){
    // Portal refs can be delivered before their final parent is attached.
    const positioner=menuElement.closest<HTMLElement>('[data-radix-popper-content-wrapper]');
    if(positioner&&positioner!==observedPositioner){
     positionObserver.disconnect();positionObserver.observe(positioner,{attributes:true,attributeFilter:['style']});
     observedPositioner=positioner;
    }
    const budget=Math.max(0,Math.min(300,Math.floor(420-baseHeight-10)));
    menuElement.style.setProperty('--recorder-menu-max-height',`${budget}px`);
    menuHeight=menuElement.offsetHeight;
    // The old 86px native window must grow before the portal is painted. Keep
    // layout measurable while hidden, then let the pointer enter transition run.
    const bounds=menuElement.getBoundingClientRect();
    const toolbar=assembly.current.querySelector<HTMLElement>('.recorder-toolbar')?.getBoundingClientRect();
    // Floating UI may update its position one frame after the viewport resize.
    // Do not reveal the old, partly clipped position (including reduced motion).
    const ready=window.innerHeight+1>=Math.ceil(baseHeight+menuHeight+10)
     &&bounds.top>=1&&bounds.left>=0&&bounds.right<=window.innerWidth
     &&!!toolbar&&bounds.bottom<=toolbar.top;
    menuElement.dataset.sizeReady=String(ready);
    setReadyFor(ready?menuElement:null);
    // Cover a position mutation that happened before the observer attached.
    // This short, bounded settle pass ends as soon as the menu is positioned.
    if(ready)settlingFrames=0;
    else if(window.innerHeight+1>=Math.ceil(baseHeight+menuHeight+10)&&settlingFrames++<12)schedule();
   }
   desired.current={width:440,height:Math.min(420,Math.max(86,Math.ceil(baseHeight+(menuHeight?menuHeight+10:0)))),menuOpen};
   void sendLatest();
  }
  const schedule=()=>{cancelAnimationFrame(frame);frame=requestAnimationFrame(measure);};
  const observer=new ResizeObserver(schedule);observer.observe(assembly.current);
  if(menuElement)observer.observe(menuElement);
  const positionObserver=new MutationObserver(schedule);
  window.addEventListener('resize',schedule);schedule();
  return()=>{active=false;mounted.current=false;observer.disconnect();positionObserver.disconnect();cancelAnimationFrame(frame);window.removeEventListener('resize',schedule);};
 },[assembly,menuElement,menuOpen,available,command]);
 return {error,menuReady:menuOpen&&menuElement!==null&&readyFor===menuElement};
}


