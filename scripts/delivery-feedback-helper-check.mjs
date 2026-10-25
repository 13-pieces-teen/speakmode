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
const tests=[],session=(state,deliveryState,deliveryFailureReason)=>({sessionId:'synthetic-result',generation:1,state,deliveryState,deliveryFailureReason,workerReleased:true});
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
await test('Interaction veto and unavailable observer explain recovery without adding a user mode',()=>{
 for(const [reason,words] of [['inputActivityChanged','点击、滚动或键盘'],['inputActivityUnavailable','未能确认输入状态'],['compatibilityTargetChanged','窗口或键鼠']]){
  const feedback=deliveryFeedback(session('ready','skipped',reason));
  assert(feedback.warning&&feedback.canCopyOutput&&feedback.canViewOutput);
  assert(feedback.message.includes(words));assert(!feedback.message.includes(reason));
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
await test('Partial input dispatch remains an actionable exception without implying a successful paste',()=>{
 const feedback=deliveryFeedback(session('ready','unconfirmed','partialInputUnconfirmed'));
 assert(feedback.warning&&feedback.canCopyOutput&&feedback.canViewOutput);
 assert(feedback.message.includes('先核对')&&!/已粘贴|已成功输入|目标已收到|已插入/.test(feedback.title+feedback.message));
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
const modesSource=await readFile(root+'/app/src/modes.ts','utf8');
await writeFile(out+'/modes-check.mjs',ts.transpileModule(modesSource,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText);
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
export const useRecorderSize=()=>({error:'',menuReady:true});
export const errorMessage=error=>String(error);
export const fromModeId=value=>value,toModeId=value=>value,builtinModes=[];
export const isPersonalMode=()=>false,scenarioOptions=()=>[];
export const Button='button',Popover={Root:'popoverRoot',Trigger:'popoverTrigger',Content:'popoverContent'};
export const CheckIcon='check',ChevronDownIcon='chevron',Cross2Icon='cross',ModeIcon='mode';
export const CaptionStrip='caption',RecorderWaveform='waveform',fragment='fragment',Fragment='fragment';
export const element=(type,props,...children)=>({type,props:{...props,children:children.length===1?children[0]:children}});
`);
const imports=new Map([
 ['../services/deliveryFeedback',pathToFileURL(out+'/delivery-feedback-check.mjs').href],
 ['../modes',pathToFileURL(out+'/modes-check.mjs').href],
 ...['react','@radix-ui/themes','@radix-ui/react-icons','../services/DesktopProvider','../hooks/useModes','../components/ui','../model','../services/bridge','./useRecorderSize','./CaptionStrip','./RecorderWaveform'].map(name=>[name,pathToFileURL(stubsPath).href]),
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
const copyButtons=(tree)=>{
 const found=[];
 const visit=node=>{if(Array.isArray(node))return node.forEach(visit);if(!node||typeof node!=='object')return;if(node.type==='button'&&['复制文本','复制已识别文字'].includes(node.props.children))found.push(node);visit(node.props?.children);};
 visit(tree);return found;
};
const renderRecorder=(deliveryState,{available=true,reject=false,extra={}}={})=>{
 const calls=[];
 setDesktop({available,session:{...session('ready',deliveryState),...extra},modes:[],recorderModeRequest:null,error:'',command:async(name,payload)=>{calls.push({name,payload});if(reject)throw new Error('synthetic navigation failure');}});
 return {buttons:copyButtons(RecorderWindow()),calls};
};
await test('Unconfirmed recorder never asks to open main or repeats delivery',()=>{
 const {buttons,calls}=renderRecorder('unconfirmed');assert.equal(buttons.length,0);assert.equal(calls.length,0);
});
await test('Unsent result offers inline copy without navigating or retrying paste',async()=>{
 for(const state of ['skipped','failed']){
  const {buttons,calls}=renderRecorder(state);assert.equal(buttons.length,1);assert.equal(buttons[0].props.disabled,false);
  buttons[0].props.onClick();await new Promise(resolve=>setImmediate(resolve));
  assert.deepEqual(calls,[{name:'result.copy',payload:{sessionId:'synthetic-result',expectedGeneration:1}}]);
 }
});
await test('Pending dispatch exposes no competing copy and browser copy is disabled',()=>{
 for(const state of ['prepared','dispatched'])assert.equal(renderRecorder(state).buttons.length,0);
 assert.equal(renderRecorder('skipped',{available:false}).buttons[0].props.disabled,true);
 assert.equal(renderRecorder('skipped',{extra:{workerReleased:false}}).buttons[0].props.disabled,true);
});
await test('Copy failure never opens a panel or retries input',async()=>{
 const {buttons,calls}=renderRecorder('skipped',{reject:true});buttons[0].props.onClick();await new Promise(resolve=>setImmediate(resolve));
 assert.equal(calls.length,1);assert.equal(calls[0].name,'result.copy');
});
await test('Incomplete ASR can copy recovery directly without claiming a complete result',async()=>{
 const {buttons,calls}=renderRecorder(null,{extra:{state:'incomplete',error:'识别未完整',canCopyRecovery:true,recoverableContentId:'synthetic-result'}});
 assert.equal(buttons.length,1);assert.equal(buttons[0].props.children,'复制已识别文字');
 buttons[0].props.onClick();await new Promise(resolve=>setImmediate(resolve));
 assert.deepEqual(calls,[{name:'recoverable.copy',payload:{sessionId:'synthetic-result',expectedGeneration:1,recoverableContentId:'synthetic-result'}}]);
});
await test('Fallback is disclosed independently of actual native delivery outcome',()=>{
 for(const reason of ['timeout','serviceUnavailable','invalidOutput','meaningRisk']){
  for(const state of ['prepared','unconfirmed','skipped']){
   const feedback=deliveryFeedback({...session('ready',state),rewriteFallback:reason});
   assert(feedback.message.startsWith('已使用识别稿。'));assert(!/已成功输入|已粘贴/.test(feedback.message));
  }
 }
});

// Render the actual main-window recovery component using the same synthetic
// hooks. Quiet completion is a presentation choice, never a native retry.
const recoverySource=await readFile(root+'/app/src/components/SessionRecovery.tsx','utf8');
const recoveryOutput=ts.transpileModule(recoverySource,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext,jsx:ts.JsxEmit.React,jsxFactory:'element',jsxFragmentFactory:'fragment'},transformers:{before:[replaceImports]}}).outputText;
await writeFile(out+'/session-recovery-check.mjs',`import {element,fragment} from ${JSON.stringify(pathToFileURL(stubsPath).href)};\n`+recoveryOutput);
const {SessionRecovery}=await import(pathToFileURL(out+'/session-recovery-check.mjs'));
const nodes=(tree,predicate)=>{
 const found=[];
 const visit=node=>{if(Array.isArray(node))return node.forEach(visit);if(!node||typeof node!=='object')return;if(predicate(node))found.push(node);visit(node.props?.children);};
 visit(tree);return found;
};
const renderRecovery=(live)=>{
 const calls=[],opened=[];
 setDesktop({session:live,command:async(name,payload)=>calls.push({name,payload}),refresh:async()=>calls.push({name:'state.snapshot'})});
 return {tree:SessionRecovery({onOpen:id=>opened.push(id)}),calls,opened};
};
await test('Main window stays quiet after normal delivery without clearing results, navigating, or retrying',()=>{
 for(const state of [undefined,'prepared','dispatched','unconfirmed','confirmed']){
  for(const extra of [{},{rewriteFallback:'timeout'}]){
   const {tree,calls,opened}=renderRecovery({...session('ready',state,state==='unconfirmed'?'inputDispatchedUnconfirmed':undefined),...extra});
   assert.equal(tree,null);assert.deepEqual(calls,[]);assert.deepEqual(opened,[]);
  }
 }
});
await test('Failed or partial delivery retains explicit copy and history actions without automatic side effects',async()=>{
 for(const [state,reason] of [['skipped','targetUnavailableAtStop'],['failed','nativeDispatchFailed'],['unconfirmed','partialInputUnconfirmed']]){
  const {tree,calls,opened}=renderRecovery(session('ready',state,reason));
  assert(tree);assert.deepEqual(calls,[]);assert.deepEqual(opened,[]);
  const copy=nodes(tree,node=>node.type==='button'&&node.props.children==='复制成稿');
  const view=nodes(tree,node=>node.type==='button'&&node.props.children==='查看成稿');
  assert.equal(copy.length,1);assert.equal(view.length,1);
  copy[0].props.onClick();await new Promise(resolve=>setImmediate(resolve));
  assert.deepEqual(calls.map(call=>call.name),['result.copy','state.snapshot']);
  assert.deepEqual(opened,[]);
  view[0].props.onClick();assert.deepEqual(opened,['synthetic-result']);
 }
});
await test('Main window preserves recording controls and recoverable recognition failures',()=>{
 for(const state of ['starting','recording','finalizingAsr','rewriting','failed','incomplete']){
  const {tree,calls,opened}=renderRecovery({...session(state),error:state==='failed'?'合成识别失败':undefined,canCopyRecovery:true});
  assert(tree,`${state} should remain actionable`);assert.deepEqual(calls,[]);assert.deepEqual(opened,[]);
  if(['failed','incomplete'].includes(state))assert.equal(nodes(tree,node=>node.type==='button'&&node.props.children==='复制已有文字').length,1);
 }
});
await test('A real saved-result error remains visible even when the dispatch receipt is unconfirmed',()=>{
 const message='本机输入回执未保存；不会再次自动输入，请核对目标后手动复制';
 const {tree,calls}=renderRecovery({...session('ready','unconfirmed','inputDispatchedUnconfirmed'),error:message});
 assert(tree);assert.equal(nodes(tree,node=>node.type==='p'&&node.props.children===message).length,1);assert.deepEqual(calls,[]);
});
const receipt={status:tests.every(test=>test.status==='PASS')?'PASS':'FAIL',scope:'Presentation logic and actual recorder/main-window recovery render and handlers with synthetic hooks and command spies. No DOM, native acceptance, automatic paste, microphone, provider, or user data.',sourceSha256:createHash('sha256').update(source).digest('hex'),recorderSourceSha256:createHash('sha256').update(recorderSource).digest('hex'),modesSourceSha256:createHash('sha256').update(modesSource).digest('hex'),sessionRecoverySourceSha256:createHash('sha256').update(recoverySource).digest('hex'),tests};
await writeFile(out+'/helper-receipt.json',JSON.stringify(receipt,null,2)+'\n');console.log(JSON.stringify(receipt,null,2));process.exitCode=receipt.status==='PASS'?0:1;
