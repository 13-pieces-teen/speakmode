import {useCallback,useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {Button} from '@radix-ui/themes';
import {desktopAvailable,errorMessage} from '../services/bridge';
import {isMacOS} from '../platform';
interface Permissions {platform:string;microphone:string;accessibility:boolean;inputMonitoring:boolean;secureInput:boolean;shortcutsReady:boolean;shortcutsStarting:boolean}
export function MacPermissions(){
 const [permissions,setPermissions]=useState<Permissions|null>(null),[error,setError]=useState(''),[busy,setBusy]=useState(false);
 const mounted=useRef(false),inFlight=useRef(false),restarted=useRef(false);
 const available=isMacOS&&desktopAvailable();
 const refresh=useCallback(async()=>{
  if(!available||inFlight.current)return;
  inFlight.current=true;
  try{
   const next=await invoke<Permissions>('native_permissions',{action:'status'});
   if(!mounted.current)return;
   setPermissions(next);setError('');
   if(next.accessibility&&!next.shortcutsReady&&!next.shortcutsStarting&&!restarted.current){
    restarted.current=true;
    try{await invoke('native_permissions',{action:'restartShortcuts'});}catch(e){restarted.current=false;if(mounted.current)setError(errorMessage(e));}
   }
   if(!next.accessibility)restarted.current=false;
  }catch(e){if(mounted.current)setError(errorMessage(e));}
  finally{inFlight.current=false;}
 },[available]);
 useEffect(()=>{
  mounted.current=true;
  if(!available)return()=>{mounted.current=false;};
  void refresh();
  const timer=setInterval(()=>{if(document.visibilityState==='visible')void refresh();},3000);
  const focus=()=>void refresh();window.addEventListener('focus',focus);
  return()=>{mounted.current=false;clearInterval(timer);window.removeEventListener('focus',focus);};
 },[available,refresh]);
 async function request(action:string){
  setBusy(true);setError('');
  try{await invoke('native_permissions',{action});await refresh();}catch(e){if(mounted.current)setError(errorMessage(e));}
  finally{if(mounted.current)setBusy(false);}
 }
 if(!available)return null;
 if(permissions?.microphone==='granted'&&permissions.accessibility&&permissions.shortcutsReady&&!permissions.secureInput&&!error)return null;
 return <section className="environment-banner" aria-label="macOS 系统权限">
  <div><strong>允许 SpeakMode 在 Mac 上录音与输入</strong>
   <p>麦克风用于识别口述；辅助功能用于全局快捷键和向输入位置粘贴成稿。授权后返回此窗口，状态会自动更新。</p>
   {permissions?.secureInput&&<p role="status">系统安全输入正在启用。退出密码输入位置后再使用全局口述。</p>}
   {permissions?.microphone==='restricted'&&<p>麦克风访问受系统策略限制，请检查设备管理设置。</p>}
   {error&&<p role="alert">{error}</p>}
  </div>
  <div className="mac-permission-actions">
   {permissions?.microphone!=='granted'&&<Button disabled={busy} onClick={()=>void request('microphone')}>允许麦克风</Button>}
   {!permissions?.accessibility&&<Button disabled={busy} onClick={()=>void request('accessibility')}>打开辅助功能设置</Button>}
   {permissions?.accessibility&&!permissions.shortcutsReady&&<Button disabled={busy||permissions.shortcutsStarting} onClick={()=>void request('restartShortcuts')}>{permissions.shortcutsStarting?'正在启用快捷键…':'重新启用快捷键'}</Button>}
   {permissions&&!permissions.inputMonitoring&&<Button variant="soft" disabled={busy} onClick={()=>void request('inputMonitoring')}>检查输入监控权限</Button>}
   <Button variant="soft" disabled={busy} onClick={()=>void refresh()}>重新检查</Button>
  </div>
 </section>;
}
