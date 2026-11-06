// Platform labels must round-trip through saved preferences; no native mutation.
import assert from 'node:assert/strict';
import {mkdir,readFile,writeFile} from 'node:fs/promises';
import {pathToFileURL,fileURLToPath} from 'node:url';
import ts from '../app/node_modules/typescript/lib/typescript.js';
const root=fileURLToPath(new URL('../',import.meta.url));
const out=root+'.validation/macos-ui';await mkdir(out,{recursive:true});
for(const platform of ['MacIntel','Win32']){
 Object.defineProperty(globalThis,'navigator',{value:{platform},configurable:true});
 const dir=out+'/'+platform;await mkdir(dir,{recursive:true});
 for(const name of ['platform','appWriting','preferences','macosShortcut']){
  const source=await readFile(root+'app/src/'+name+'.ts','utf8');
  const compiled=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText.replace(/from '(\.\/(?:platform|appWriting))'/g,"from '$1.mjs'");
  await writeFile(dir+'/'+name+'.mjs',compiled);
 }
 const prefs=await import(pathToFileURL(dir+'/preferences.mjs'));
 const apps=await import(pathToFileURL(dir+'/appWriting.mjs'));
 assert.equal(prefs.shortcutLabel('alt_right'),platform==='MacIntel'?'右 Option':'右 Alt');
 assert.equal(prefs.defaultPreferences.recorderCaptionsEnabled,false);
 const nativePreferences=prefs.toNativePatch({...prefs.defaultPreferences,recorderCaptionsEnabled:true});
 assert.equal(nativePreferences.shortcut,'alt_right');
 assert.equal(nativePreferences.recorderCaptionsEnabled,true);
 assert.equal(prefs.fromNativePreferences(nativePreferences).recorderCaptionsEnabled,true);
 delete nativePreferences.recorderCaptionsEnabled;
 assert.equal(prefs.fromNativePreferences(nativePreferences).recorderCaptionsEnabled,false);
 for(const shortcut of ['alt_right','ctrl+shift+k','option_left+space','command_right+shift+k']){
  const canonical=prefs.shortcutCanonical(prefs.shortcutLabel(shortcut));
  assert.equal(canonical,shortcut);
 }
 assert.equal(apps.applicationKey('com.microsoft.VSCode'),'com.microsoft.vscode');
 assert.equal(apps.applicationKey('Code.exe'),'code.exe');
 for(const value of ['../code.exe','/Applications/Test.app','com..bad','com.app\nother'])assert.equal(apps.applicationKey(value),null);
 assert.equal(apps.writingApplications[0].id,platform==='MacIntel'?'com.tencent.xinwechat':'wechat.exe');
 const {readMacShortcut}=await import(pathToFileURL(dir+'/macosShortcut.mjs'));
 const event={key:'k',code:'KeyK',ctrlKey:false,altKey:false,shiftKey:false,metaKey:false,nativeEvent:{isComposing:false}};
 assert.equal(readMacShortcut({...event,key:'Alt',code:'AltRight',altKey:true}).value,'右 Option');
 assert.equal(readMacShortcut({...event,metaKey:true,shiftKey:true}).value,'Shift + ⌘ + K');
 assert.ok(readMacShortcut({...event,code:'KeyV',metaKey:true}).error);
 assert.ok(readMacShortcut({...event,code:'Space',metaKey:true}).error);
 assert.ok(readMacShortcut(event).error);
 assert.ok(readMacShortcut({...event,altKey:true,nativeEvent:{isComposing:true}}).error);
}
console.log('PASS: Mac/Windows shortcut and caption preference round-trip, legacy defaults, application identities and Mac shortcut conflicts');
