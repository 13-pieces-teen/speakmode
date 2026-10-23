export interface MacKey {key:string;code:string;ctrlKey:boolean;altKey:boolean;shiftKey:boolean;metaKey:boolean;nativeEvent:{isComposing?:boolean}}
export type MacShortcutResult={value:string;error?:never}|{value?:never;error:string};
const reserved=new Set(['A','C','V','X','Z','F','R','W','T','N','L','Q','H','M','O','P','S']);
export function readMacShortcut(event:MacKey):MacShortcutResult{
 const {key,code,ctrlKey,altKey,shiftKey,metaKey}=event;
 if(event.nativeEvent.isComposing)return{error:'请结束输入法组合后重新按键。'};
 if(code==='AltRight'&&!ctrlKey&&!shiftKey&&!metaKey)return{value:'右 Option'};
 if(!ctrlKey&&!altKey&&!shiftKey&&!metaKey&&/^F(?:2|3|[6-9]|1[0-2])$/.test(code||key))return{value:code||key};
 const letter=/^Key[A-Z]$/.test(code)?code.slice(3):null;
 const space=code==='Space';
 if(!letter&&!space)return{error:'请使用右 Option、F2/F3/F6–F12，或 Control / Option / Command 加字母。'};
 if(!ctrlKey&&!altKey&&!metaKey)return{error:'字母或空格需要配合 Control、Option 或 Command，避免影响正常输入。'};
 if(space&&((metaKey&&!ctrlKey)||(ctrlKey&&!metaKey)))return{error:'这组空格快捷键用于系统搜索或切换输入法，请换一组。'};
 if(letter&&((metaKey&&!ctrlKey&&reserved.has(letter))||(!metaKey&&ctrlKey&&!altKey&&reserved.has(letter))))return{error:'这组按键是常用编辑或应用快捷键，请换一组。'};
 return{value:[ctrlKey&&'Ctrl',altKey&&'Option',shiftKey&&'Shift',metaKey&&'⌘',letter??'Space'].filter(Boolean).join(' + ')};
}
