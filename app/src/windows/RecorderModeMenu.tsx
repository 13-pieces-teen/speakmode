import {Fragment,useEffect,useRef,useState} from 'react';
import {Popover} from '@radix-ui/themes';
import {CheckIcon} from '@radix-ui/react-icons';
import {useDesktop} from '../services/DesktopProvider';
import {useModes} from '../hooks/useModes';
import {ModeIcon} from '../components/ui';
import {fromModeId,toModeId} from '../model';
import {isPersonalMode,scenarioOptions} from '../modes';

function focusModeChoice(button:HTMLButtonElement|undefined|null){
 if(!button)return;button.focus({preventScroll:true});
 const panel=button.closest<HTMLElement>('.recorder-mode-options');if(!panel)return;
 const item=button.getBoundingClientRect(),bounds=panel.getBoundingClientRect();
 // Scroll the bounded menu, never the transparent desktop window/document.
 if(item.top<bounds.top)panel.scrollTop+=item.top-bounds.top;
 else if(item.bottom>bounds.bottom)panel.scrollTop+=item.bottom-bounds.bottom;
}
interface Props {identity:string;canSelect:boolean;busy:boolean;open:boolean;onOpenChange:(open:boolean)=>void;element:HTMLDivElement|null;setElement:(element:HTMLDivElement|null)=>void;ready:boolean;onSelect:(modeId:string,revision:number)=>void}
export function RecorderModeMenu({identity,canSelect,busy,open:menuOpen,onOpenChange:setMenu,element:menuElement,setElement,ready:menuReady,onSelect:selectMode}:Props){
 const desktop=useDesktop(),catalog=useModes(),session=desktop.session;
 const handledMenuRequest=useRef(-1),focusedChoice=useRef<HTMLButtonElement|null>(null);
 const [previewMode,setPreviewMode]=useState<string|null>(null);
 const current=fromModeId(session?.modeSnapshot?.modeId??session?.modeId??'auto');
 useEffect(()=>{setMenu(false);setPreviewMode(null);},[identity,setMenu]);
 useEffect(()=>{if(!canSelect)setMenu(false);},[canSelect,setMenu]);
 useEffect(()=>{if(!menuReady||!menuElement)return;const previous=focusedChoice.current;focusModeChoice(previous&&menuElement.contains(previous)?previous:menuElement.querySelector<HTMLButtonElement>('button[aria-checked="true"]'));},[menuReady,menuElement]);
 useEffect(()=>{if(!menuOpen)setPreviewMode(null);},[menuOpen]);
 useEffect(()=>{const request=desktop.recorderModeRequest;if(!request||request.sequence<=handledMenuRequest.current)return;handledMenuRequest.current=request.sequence;if(!canSelect||request.sessionId!==(session?.sessionId??session?.id)||request.generation!==session?.generation)return;setMenu(true);},[desktop.recorderModeRequest,canSelect,session,setMenu]);
 useEffect(()=>{if(!menuOpen)return;const closeMenu=(event:Event)=>{if(event.target===window)setMenu(false);};window.addEventListener('blur',closeMenu);return()=>window.removeEventListener('blur',closeMenu);},[menuOpen,setMenu]);
 const options=scenarioOptions(current,catalog.modes).map(option=>({...option,revision:desktop.modes.find(mode=>mode.id===toModeId(option.id))?.revision??option.revision}));
 const preview=options.find(option=>option.id===previewMode)??options.find(option=>option.id===current)??options[0];
 return <Popover.Root open={menuOpen} onOpenChange={setMenu}>
  <Popover.Anchor className="recorder-mode-anchor" aria-hidden="true" />
    <Popover.Content ref={setElement} avoidCollisions={false} side="top" align="start" sideOffset={10}
     className="recorder-mode-menu" data-motion={'instant'} role="menu" aria-label="选择本次场合"
     onFocusCapture={event=>{const target=event.target;if(target instanceof HTMLButtonElement&&target.classList.contains('recorder-mode-option')){focusedChoice.current=target;setPreviewMode(target.dataset.modeId??null);}}}
     onKeyDownCapture={event=>{if(event.key==='Tab')setMenu(false);}}
     onPointerLeave={()=>setPreviewMode(focusedChoice.current?.dataset.modeId??current)}
     onOpenAutoFocus={event=>event.preventDefault()}>
     <div className="recorder-mode-heading"><span>本次场合</span><span className="recorder-mode-scope">仅本次</span></div>
     {<p className="recorder-mode-notice" role="status">结束前请切回输入窗口，无法确认位置时会保留成稿。</p>}
     <div className="recorder-mode-options" role="group" aria-label="本次使用场合" onKeyDown={event=>{
      if(!["ArrowDown","ArrowUp","Home","End"].includes(event.key))return;
      event.preventDefault();
      const buttons=Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
      const index=buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next=event.key==="Home"?0:event.key==="End"?buttons.length-1:(index+(event.key==="ArrowDown"?1:-1)+buttons.length)%buttons.length;
      focusModeChoice(buttons[next]);
     }}>
      {options.map((option,index)=><Fragment key={option.id}>
       {index===1&&<div className="recorder-mode-separator" role="separator"/>}
       {isPersonalMode(option.id)&&!isPersonalMode(options[index-1]?.id??'自动')&&<div className="recorder-mode-group-label">个人场合</div>}
       <button type="button" className="recorder-mode-option" role="menuitemradio" data-mode-id={option.id}
        aria-checked={option.id===current} aria-label={option.name} aria-description={option.caption}
        tabIndex={option.id===current?0:-1} title={`${option.name} · ${option.caption}`} disabled={busy}
        onPointerMove={()=>setPreviewMode(option.id)} onClick={()=>selectMode(toModeId(option.id),option.revision)}>
        <span className="recorder-mode-icon"><ModeIcon mode={option.id}/></span>
        <span className="recorder-mode-name"><span className="recorder-mode-label">{option.name}</span>{option.id==='自动'&&<span className="recorder-mode-recommended" aria-hidden="true">推荐</span>}</span>
        <span className="recorder-mode-check" aria-hidden="true">{option.id===current&&<CheckIcon/>}</span>
       </button>
      </Fragment>)}
     </div>
     <div className="recorder-mode-preview" aria-hidden="true"><ModeIcon mode={preview.id}/><span>{isPersonalMode(preview.id)?preview.name:preview.caption}</span></div>
    </Popover.Content>
 </Popover.Root>;
}
