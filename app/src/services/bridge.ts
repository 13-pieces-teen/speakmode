import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import type { AppEvent, NativeSnapshot } from './contracts';
export const desktopAvailable = () => isTauri();
export function errorMessage(error:unknown):string {
  if (typeof error === 'string') { try { const parsed=JSON.parse(error); return parsed.message ?? parsed.messageKey ?? error; } catch { return error; } }
  if (error && typeof error === 'object') { const value=error as Record<string,unknown>; if(typeof value.message==='string')return value.message; if(typeof value.messageKey==='string')return value.messageKey; }
  return error instanceof Error ? error.message : '操作未完成，请重试。';
}
export async function desktopCommand<T>(name:string,payload:Record<string,unknown>={}):Promise<T> {
  if (!desktopAvailable()) throw new Error('请在 SpeakMode 桌面应用中使用此功能。');
  return invoke<T>('app_command',{request:{name,payload:{requestId:crypto.randomUUID(),...payload}}});
}
export function subscribeDesktop(onEvent:(event:AppEvent)=>void):Promise<NativeSnapshot> {
  const channel=new Channel<AppEvent>(); channel.onmessage=onEvent;
  return invoke<NativeSnapshot>('subscribe_state',{channel});
}

