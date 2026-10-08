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
const test=(name,run)=>{try{run();tests.push({name,status:'PASS'});}catch(error){tests.push({name,status:'FAIL',error:String(error)});}};
test('Ready without receipt is not reported as pasted or failed',()=>{
 assert.equal(deliveryFeedback(null),null);
 assert.equal(deliveryFeedback(session('ready')),null);
});
test('Historical receipt fields do not create current delivery feedback during recording or failed ASR',()=>{
 for(const phase of ['starting','recording','finalizingAsr','rewriting','failed','incomplete','cancelled'])assert.equal(deliveryFeedback(session(phase,'skipped','targetUnavailableAtStop')),null);
});
test('Every stable failure reason gives an actionable visible explanation and permits final-output copy',()=>{
 const codes={targetUnavailableAtStop:'输入的位置',targetChangedBeforeDelivery:'位置已改变',modifiersNotReleased:'尚未松开',clipboardUnavailable:'剪贴板',nativeDispatchFailed:'先核对',deliveryLedgerUnavailable:'准备'};
 for(const [code,words] of Object.entries(codes)){
  const feedback=deliveryFeedback(session('ready',code==='nativeDispatchFailed'?'failed':'skipped',code));
  assert(feedback.warning&&feedback.canCopyOutput);assert(feedback.message.includes(words));assert(feedback.message.includes('成稿已保留'));assert(!feedback.message.includes(code));
 }
});
test('Unknown future failure reasons remain readable without showing internal codes',()=>{
 for(const state of ['skipped','failed']){
  const feedback=deliveryFeedback(session('ready',state,'opaque_future_reason'));
  assert(feedback.warning&&feedback.canCopyOutput);assert(!feedback.message.includes('opaque_future_reason'));
 }
});
test('Native dispatch is an unconfirmed receipt and never described as insertion success',()=>{
 const feedback=deliveryFeedback(session('ready','unconfirmed'));
 assert.equal(feedback.title,'输入已派发');assert(!feedback.warning&&!feedback.canCopyOutput);assert(feedback.message.includes('不会自动重复输入'));
 assert(!/已粘贴|已成功输入|目标已收到|已插入/.test(feedback.title+feedback.message));
});
test('Prepared and dispatched stages do not invite competing clipboard copy',()=>{
 for(const state of ['prepared','dispatched']){
  const feedback=deliveryFeedback(session('ready',state));assert(!feedback.warning&&!feedback.canCopyOutput);assert(feedback.message.includes('正在发送'));
 }
});
const receipt={status:tests.every(test=>test.status==='PASS')?'PASS':'FAIL',scope:'Pure presentation logic using synthetic session states. No native acceptance, automatic paste, microphone, provider, or user data.',sourceSha256:createHash('sha256').update(source).digest('hex'),tests};
await writeFile(out+'/helper-receipt.json',JSON.stringify(receipt,null,2)+'\n');console.log(JSON.stringify(receipt,null,2));process.exitCode=receipt.status==='PASS'?0:1;
