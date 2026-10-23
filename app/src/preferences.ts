import {isMacOS} from './platform';
import { defaultAppWriting, normalizeAppWriting, type AppWritingPreferences } from './appWriting';
import type { NativePreferences } from './services/contracts';
export type RecognitionLanguage='auto'|'zh'|'en';
export type DeliveryMode='insert'|'preview';
export interface Preferences {version:number;shortcut:string;microphoneId:string|null;language:RecognitionLanguage;delivery:DeliveryMode;softVoiceEnabled:boolean;appWriting:AppWritingPreferences;keepHistory:boolean;launchOnStartup:boolean}
export const defaultPreferences:Readonly<Preferences>=Object.freeze({version:1,shortcut:isMacOS?'右 Option':'右 Alt',microphoneId:null,language:'auto',delivery:'insert',softVoiceEnabled:false,appWriting:defaultAppWriting,keepHistory:true,launchOnStartup:false});
export const languageLabels:Record<RecognitionLanguage,string>={auto:'自动检测',zh:'中文',en:'English'};
const shortcutNames:Record<string,string>={ctrl:'Ctrl',alt:isMacOS?'Option':'Alt',option:'Option',shift:'Shift',space:'Space',cmd:'⌘',command:'⌘',command_left:'左 ⌘',command_right:'右 ⌘',option_left:'左 Option',option_right:'右 Option',alt_right:isMacOS?'右 Option':'右 Alt',alt_left:isMacOS?'左 Option':'左 Alt',ctrl_left:'左 Ctrl',ctrl_right:'右 Ctrl',shift_left:'左 Shift',shift_right:'右 Shift',fn:'Fn'};
const canonicalNames:Record<string,string>={'⌘':'command','左 ⌘':'command_left','右 ⌘':'command_right','左 Option':'option_left','右 Option':'alt_right','左 Alt':'alt_left','右 Alt':'alt_right','左 Ctrl':'ctrl_left','右 Ctrl':'ctrl_right','左 Shift':'shift_left','右 Shift':'shift_right'};
export const shortcutLabel=(value:string)=>value.split('+').map(part=>shortcutNames[part.toLowerCase()]??part.toUpperCase()).join(' + ');
export const shortcutCanonical=(value:string)=>value.split('+').map(part=>canonicalNames[part.trim()]??part.trim().replace(/\s+/g,'').toLowerCase()).join('+');
export function fromNativePreferences(value:NativePreferences):Preferences{return {version:value.version,shortcut:shortcutLabel(value.shortcut),microphoneId:value.microphoneId,language:value.language,delivery:value.deliveryMode==='direct'?'insert':'preview',softVoiceEnabled:value.softVoiceEnabled ?? false,appWriting:normalizeAppWriting(value.appWriting),keepHistory:value.keepHistory,launchOnStartup:value.launchAtLogin};}
export function toNativePatch(value:Preferences):Record<string,unknown>{return {shortcut:shortcutCanonical(value.shortcut),microphoneId:value.microphoneId,language:value.language,deliveryMode:value.delivery==='insert'?'direct':'preview',softVoiceEnabled:value.softVoiceEnabled,appWriting:value.appWriting,keepHistory:value.keepHistory,launchAtLogin:value.launchOnStartup};}


