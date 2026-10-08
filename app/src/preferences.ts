import type { NativePreferences } from './services/contracts';
export type RecognitionLanguage='auto'|'zh'|'en';
export type DeliveryMode='insert'|'preview';
export interface Preferences {version:number;shortcut:string;microphoneId:string|null;language:RecognitionLanguage;delivery:DeliveryMode;keepHistory:boolean;launchOnStartup:boolean}
export const defaultPreferences:Readonly<Preferences>=Object.freeze({version:1,shortcut:'右 Alt',microphoneId:null,language:'auto',delivery:'insert',keepHistory:true,launchOnStartup:false});
export const languageLabels:Record<RecognitionLanguage,string>={auto:'自动检测',zh:'中文',en:'English'};
export const shortcutLabel=(value:string)=>value==='alt_right'?'右 Alt':value.split('+').map(part=>({ctrl:'Ctrl',alt:'Alt',shift:'Shift',space:'Space'}[part.toLowerCase()]??part.toUpperCase())).join(' + ');
export const shortcutCanonical=(value:string)=>value==='右 Alt'?'alt_right':value.replace(/\s+/g,'').toLowerCase();
export function fromNativePreferences(value:NativePreferences):Preferences{return {version:value.version,shortcut:shortcutLabel(value.shortcut),microphoneId:value.microphoneId,language:value.language,delivery:value.deliveryMode==='direct'?'insert':'preview',keepHistory:value.keepHistory,launchOnStartup:value.launchAtLogin};}
export function toNativePatch(value:Preferences):Record<string,unknown>{return {shortcut:shortcutCanonical(value.shortcut),microphoneId:value.microphoneId,language:value.language,deliveryMode:value.delivery==='insert'?'direct':'preview',keepHistory:value.keepHistory,launchAtLogin:value.launchOnStartup};}


