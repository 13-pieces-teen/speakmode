// Synthetic delivery receipts only: no native target, clipboard, microphone, or model.
import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {pathToFileURL,fileURLToPath} from 'node:url';
import ts from '../app/node_modules/typescript/lib/typescript.js';
const root=fileURLToPath(new URL('../',import.meta.url)),out=root+'/.validation/delivery';
await mkdir(out,{recursive:true});
const source=await readFile(root+'/app/src/services/deliveryFeedback.ts','utf8');
await writeFile(out+'/delivery-feedback-check.mjs',ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText);
const {deliveryFeedback}=await import(pathToFileURL(out+'/delivery-feedback-check.mjs'));
const tests=[],session=(state,deliveryState,deliveryFailureReason)=>({sessionId:'synthetic-result',generation:1,state,deliveryState,deliveryFailureReason});
const test=async(name,run)=>{try{await run();tests.push({name,status:'PASS'});}catch(error){tests.push({name,status:'FAIL',error:String(error)});}};
await test('Ready without receipt is not reported as pasted or failed',()=>{
 assert.equal(deliveryFeedback(null),null);
 assert.equal(deliveryFeedback(session('ready')),null);
});
await test('Historical receipt fields do not create current delivery feedback during recording or failed ASR',()=>{
 for(const phase of ['starting','recording','finalizingAsr','rewriting','failed','incomplete','cancelled'])assert.equal(deliveryFeedback(session(phase,'skipped','targetUnavailableAtStop')),null);
});
await test('Every stable failure reason gives an actionable visible explanation and permits final-output copy and viewing',()=>{
 const codes={targetUnavailableAtStop:'输入的位置',targetHigherIntegrity:'管理员权限',targetChangedBeforeDelivery:'位置已改变',modifiersNotReleased:'尚未松开',clipboardUnavailable:'剪贴板',nativeDispatchFailed:'先核对',deliveryLedgerUnavailable:'准备'};
 for(const [code,words] of Object.entries(codes)){
  const feedback=deliveryFeedback(session('ready',code==='nativeDispatchFailed'?'failed':'skipped',code));
  assert(feedback.warning&&feedback.canCopyOutput&&feedback.canViewOutput);assert(feedback.message.includes(words));assert(feedback.message.includes('成稿已保留'));assert(!feedback.message.includes(code));
 }
});
await test('Stop target diagnostics distinguish lost focus, unsupported fields and slow inspection without exposing codes',()=>{
 const codes={stopAnchorUnavailable:'输入窗口',stopFocusChanged:'位置已改变',targetNotEditable:'文本输入框',targetCapabilityUnsupported:'无法确认光标',targetInspectionTimeout:'响应较慢'};
 for(const [code,words] of Object.entries(codes)){
  const feedback=deliveryFeedback(session('ready','skipped',code));
  assert(feedback.warning&&feedback.canCopyOutput&&feedback.canViewOutput);
  assert(feedback.message.includes(words),`${code} needs a specific explanation`);
  assert(feedback.message.includes('成稿已保留'));
  assert(!feedback.message.includes(code));
 }
});
await test('Unknown future failure reasons remain readable without showing internal codes',()=>{
 for(const state of ['skipped','failed']){
  const feedback=deliveryFeedback(session('ready',state,'opaque_future_reason'));
  assert(feedback.warning&&feedback.canCopyOutput&&feedback.canViewOutput);assert(!feedback.message.includes('opaque_future_reason'));
 }
});
await test('Native dispatch allows viewing without copying or claiming insertion success',()=>{
 const feedback=deliveryFeedback(session('ready','unconfirmed'));
 assert.equal(feedback.title,'输入已派发');assert(!feedback.warning&&!feedback.canCopyOutput&&feedback.canViewOutput);assert(feedback.message.includes('不会自动重复输入'));
 assert(!/已粘贴|已成功输入|目标已收到|已插入/.test(feedback.title+feedback.message));
});
await test('Prepared and dispatched stages do not invite competing clipboard copy or result navigation',()=>{
 for(const state of ['prepared','dispatched']){
  const feedback=deliveryFeedback(session('ready',state));assert(!feedback.warning&&!feedback.canCopyOutput&&!feedback.canViewOutput);assert(feedback.message.includes('正在发送'));
 }
});
await test('Confirmed and future unknown receipts do not imply a new copy, retry or view action',()=>{
 for(const state of ['confirmed','future_receipt'])assert.equal(deliveryFeedback(session('ready',state)),null);
});
await test('Higher-integrity target explains manual copy and viewing without elevation or automatic retries',()=>{
 const feedback=deliveryFeedback(session('ready','skipped','targetHigherIntegrity'));
 assert(feedback.warning&&feedback.canCopyOutput&&feedback.canViewOutput);
 assert(feedback.message.includes('本次未自动输入')&&feedback.message.includes('手动粘贴'));
 assert(!/自动提权|以管理员身份运行|重试自动输入|自动重试|再次发送|重新发送/.test(feedback.title+feedback.message));
});

// Exercise the real recorder render and button handler with synthetic hooks and
// a command spy. No React effects, DOM, app window, clipboard or audio is started.
const recorderSource=await readFile(root+'/app/src/windows/RecorderWindow.tsx','utf8');
const stubsPath=out+'/recorder-test-stubs.mjs';
await writeFile(stubsPath,`
let desktop;
export const setDesktop=value=>{desktop=value;};
export const useDesktop=()=>desktop;
export const useCallback=callback=>callback;
export const useEffect=()=>{};
export const useRef=current=>({current});
export const useState=initial=>[typeof initial==='function'?initial():initial,()=>{}];
export const useModes=()=>({label:value=>value,modes:[]});
export const useRecorderSize=()=>'';
export const errorMessage=error=>String(error);
export const fromModeId=value=>value,toModeId=value=>value,builtinModes=[];
export const Button='button',Popover={Root:'popoverRoot',Trigger:'popoverTrigger',Content:'popoverContent'};
export const CheckIcon='check',ChevronDownIcon='chevron',Cross2Icon='cross',ModeIcon='mode';
export const CaptionStrip='caption',RecorderWaveform='waveform',fragment='fragment';
export const element=(type,props,...children)=>({type,props:{...props,children:children.length===1?children[0]:children}});
`);
const imports=new Map([
 ['../services/deliveryFeedback',pathToFileURL(out+'/delivery-feedback-check.mjs').href],
 ...['react','@radix-ui/themes','@radix-ui/react-icons','../services/DesktopProvider','../hooks/useModes','../components/ui','../model','../modes','../services/bridge','./useRecorderSize','./CaptionStrip','./RecorderWaveform'].map(name=>[name,pathToFileURL(stubsPath).href]),
]);
const replaceImports=context=>node=>ts.visitNode(node,function visit(current){
 if(ts.isImportDeclaration(current)&&ts.isStringLiteral(current.moduleSpecifier)){
  const name=current.moduleSpecifier.text;
  if(name.endsWith('.css'))return undefined;
  const replacement=imports.get(name);assert(replacement,`Unregistered recorder test dependency: ${name}`);
  return ts.factory.updateImportDeclaration(current,current.modifiers,current.importClause,ts.factory.createStringLiteral(replacement),current.attributes);
 }
 return ts.visitEachChild(current,visit,context);
});
const recorderOutput=ts.transpileModule(recorderSource,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext,jsx:ts.JsxEmit.React,jsxFactory:'element',jsxFragmentFactory:'fragment'},transformers:{before:[replaceImports]}}).outputText;
await writeFile(out+'/recorder-check.mjs',`import {element,fragment} from ${JSON.stringify(pathToFileURL(stubsPath).href)};\n`+recorderOutput);
const {RecorderWindow}=await import(pathToFileURL(out+'/recorder-check.mjs'));
const {setDesktop}=await import(pathToFileURL(stubsPath));
const viewButtons=(tree)=>{
 const found=[];
 const visit=node=>{if(Array.isArray(node))return node.forEach(visit);if(!node||typeof node!=='object')return;if(node.type==='button'&&node.props.children==='查看成稿')found.push(node);visit(node.props?.children);};
 visit(tree);return found;
};
const renderRecorder=(deliveryState,{available=true,reject=false}={})=>{
 const calls=[];
 setDesktop({available,session:session('ready',deliveryState),modes:[],recorderModeRequest:null,error:'',command:async(name,payload)=>{calls.push({name,payload});if(reject)throw new Error('synthetic navigation failure');}});
 return {buttons:viewButtons(RecorderWindow()),calls};
};
await test('Unconfirmed recorder action opens the main panel once without clipboard or delivery commands',async()=>{
 const {buttons,calls}=renderRecorder('unconfirmed');assert.equal(buttons.length,1);assert.equal(buttons[0].props.disabled,false);
 buttons[0].props.onClick();await new Promise(resolve=>setImmediate(resolve));
 assert.deepEqual(calls,[{name:'app.showMain',payload:{sessionId:'synthetic-result',expectedGeneration:1}}]);
});
await test('Prepared and dispatched recorder feedback exposes no result button; preserved failures still allow viewing',()=>{
 for(const state of ['prepared','dispatched'])assert.equal(renderRecorder(state).buttons.length,0);
 for(const state of ['skipped','failed'])assert.equal(renderRecorder(state).buttons.length,1);
});
await test('Browser recorder view action is disabled and failed navigation never retries input',async()=>{
 assert.equal(renderRecorder('unconfirmed',{available:false}).buttons[0].props.disabled,true);
 const {buttons,calls}=renderRecorder('unconfirmed',{reject:true});buttons[0].props.onClick();await new Promise(resolve=>setImmediate(resolve));
 assert.equal(calls.length,1);assert.equal(calls[0].name,'app.showMain');
});
const receipt={status:tests.every(test=>test.status==='PASS')?'PASS':'FAIL',scope:'Presentation logic and actual recorder button handler with synthetic hooks and command spies. No DOM, native acceptance, automatic paste, microphone, provider, or user data.',sourceSha256:createHash('sha256').update(source).digest('hex'),recorderSourceSha256:createHash('sha256').update(recorderSource).digest('hex'),tests};
await writeFile(out+'/helper-receipt.json',JSON.stringify(receipt,null,2)+'\n');console.log(JSON.stringify(receipt,null,2));process.exitCode=receipt.status==='PASS'?0:1;
