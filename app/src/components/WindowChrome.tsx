import {useCallback,useEffect,useRef,useState,type MouseEvent} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {Cross2Icon,MinusIcon,SquareIcon,CopyIcon,MixerHorizontalIcon} from '@radix-ui/react-icons';
import {errorMessage} from '../services/bridge';

type WindowAction='state'|'drag'|'minimize'|'toggleMaximize'|'close';
interface WindowState {maximized:boolean;minimized:boolean}

export function WindowChrome({available}:{available:boolean}) {
 const [maximized,setMaximized]=useState(false),[error,setError]=useState('');
 const mounted=useRef(false),resizeTimer=useRef<ReturnType<typeof setTimeout>|undefined>(undefined);
 const control=useCallback(async(action:WindowAction)=>{
  if(!available)return;
  try {const state=await invoke<WindowState>('control_main_window',{action});if(mounted.current){setMaximized(state.maximized);setError('');}}
  catch(e){if(mounted.current)setError(errorMessage(e));}
 },[available]);
 useEffect(()=>{
  mounted.current=true;void control('state');
  const resized=()=>{clearTimeout(resizeTimer.current);resizeTimer.current=setTimeout(()=>void control('state'),100);};
  window.addEventListener('resize',resized);
  return()=>{mounted.current=false;clearTimeout(resizeTimer.current);window.removeEventListener('resize',resized);};
 },[control]);
 const drag=(event:MouseEvent<HTMLDivElement>)=>{
  if(event.button===0&&event.detail===1)void control('drag');
 };
 const maximize=()=>void control('toggleMaximize');
 return <header className="window-titlebar" aria-label="窗口标题栏">
  <div className="window-brand window-drag-region" onMouseDown={drag} onDoubleClick={maximize}>
   <div className="brand"><span className="brand-mark"><MixerHorizontalIcon aria-hidden="true"/></span><span>Speak<span className="brand-mode">Mode</span></span></div>
  </div>
  <div className="window-drag-space window-drag-region" onMouseDown={drag} onDoubleClick={maximize}>
   {error&&<span className="window-error" role="alert">{error}</span>}
  </div>
  {available&&<div className="window-controls">
   <button type="button" className="window-control" aria-label="最小化窗口" title="最小化" onClick={()=>void control('minimize')}><MinusIcon aria-hidden="true"/></button>
   <button type="button" className="window-control" aria-label={maximized?'还原窗口':'最大化窗口'} title={maximized?'还原':'最大化'} onClick={maximize}>{maximized?<CopyIcon aria-hidden="true"/>:<SquareIcon aria-hidden="true"/>}</button>
   <button type="button" className="window-control window-control-close" aria-label="关闭主面板" title="关闭主面板，保留托盘运行" onClick={()=>void control('close')}><Cross2Icon aria-hidden="true"/></button>
  </div>}
 </header>;
}
