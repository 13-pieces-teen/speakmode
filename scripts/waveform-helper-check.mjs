// Synthetic time/RMS display evaluation. Does not open a microphone or provider.
import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {pathToFileURL,fileURLToPath} from 'node:url';
import ts from '../app/node_modules/typescript/lib/typescript.js';
const root=fileURLToPath(new URL('../',import.meta.url)),out=root+'/.validation/waveform';
await mkdir(out,{recursive:true});
const source=await readFile(root+'/app/src/windows/waveform.ts','utf8');
await writeFile(out+'/waveform-check.mjs',ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText);
const {loudnessFromRms,initialWaveform,stepWaveform,waveformScale,waveformQuiet,WAVEFORM_BASE_SCALE,WAVEFORM_BARS}=await import(pathToFileURL(out+'/waveform-check.mjs'));
const tests=[],metrics=[];const test=(name,run)=>{try{run();tests.push({name,status:'PASS'});}catch(error){tests.push({name,status:'FAIL',error:String(error)});}};
test('Ordinary synthetic RMS is visible where the old linear floor masks it',()=>{
 const levels=[.005,.01,.04];for(const rms of levels){const old=Array.from({length:13},(_,i)=>Math.max(.13,rms*(.36+.64*Math.sin((i+1)/14*Math.PI))));const newHeight=24*waveformScale(loudnessFromRms(rms));assert(old.every(v=>v===.13));assert(newHeight>8);metrics.push({syntheticRms:rms,oldMaxHeightPx:24*Math.max(...old),newTargetHeightPx:newHeight});}
});
test('Silence, noise below -60 dBFS, and invalid inputs remain at baseline',()=>{
 for(const rms of [0,.0005,.001,-1,NaN,Infinity,-Infinity])assert.equal(loudnessFromRms(rms),0);
 let state=initialWaveform(0);for(let t=16;t<1000;t+=16)state=stepWaveform(state,{rms:.0005,receivedAt:t},t);
 assert.equal(state.envelope,0);assert(state.history.every(v=>waveformScale(v)===WAVEFORM_BASE_SCALE));
});
test('Display level is bounded and monotonic across microphone gain',()=>{
 const values=[.001,.002,.005,.01,.03,.06,.13,.5,1,9].map(loudnessFromRms);
 values.forEach((value,index)=>{assert(value>=0&&value<=1);if(index)assert(value>=values[index-1]);});assert.equal(values.at(-1),1);
});
test('Attack follows voice quickly; release decays more gently without fake oscillation',()=>{
 let state=initialWaveform(0);for(let t=15;t<=90;t+=15)state=stepWaveform(state,{rms:.03,receivedAt:t},t);
 const peak=state.envelope,target=loudnessFromRms(.03);assert(peak>.85*target);
 const falling=stepWaveform(state,{rms:0,receivedAt:105},105);assert(falling.envelope<peak&&falling.envelope>.9*peak);
 const quiet=stepWaveform(falling,{rms:0,receivedAt:120},120);assert(quiet.envelope<=falling.envelope);
});
test('History contains only the measured envelope and remains bounded at 13 bars',()=>{
 let state=initialWaveform(0);for(let t=20;t<=1400;t+=20)state=stepWaveform(state,{rms:t<450?.01:t<850?.04:.005,receivedAt:t},t);
 assert.equal(state.history.length,WAVEFORM_BARS);assert(state.history.every(v=>v>=0&&v<=1));assert(new Set(state.history.map(v=>v.toFixed(3))).size>3);
});
test('Missing events decay to silence and release the animation loop',()=>{
 let state=initialWaveform(0);const sample={rms:.03,receivedAt:0};for(let t=16;t<=2400;t+=16)state=stepWaveform(state,sample,t);
 assert.equal(state.envelope,0);assert(state.history.every(v=>v===0));assert(waveformQuiet(state,sample,2400));
});
test('A fresh constant-valued heartbeat sustains a true steady level without a random pulse',()=>{
 let state=initialWaveform(0);for(let t=16;t<=3000;t+=16)state=stepWaveform(state,{rms:.02,receivedAt:t-t%80},t);
 const target=loudnessFromRms(.02);assert(Math.abs(state.envelope-target)<.001);assert(Math.max(...state.history)-Math.min(...state.history)<.001);
});
const receipt={status:tests.every(t=>t.status==='PASS')?'PASS':'FAIL',scope:'Synthetic RMS/monotonic time, presentation only. No microphone activation, real cloud speech, ASR accuracy or user-task result.',sourceSha256:createHash('sha256').update(source).digest('hex'),tests,metrics};
await writeFile(out+'/helper-receipt.json',JSON.stringify(receipt,null,2)+'\n');console.log(JSON.stringify(receipt,null,2));process.exitCode=receipt.status==='PASS'?0:1;
