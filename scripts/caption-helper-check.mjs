// Deterministic protocol/display invariants, no browser/native/audio/provider.
import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {pathToFileURL,fileURLToPath} from 'node:url';
import ts from '../app/node_modules/typescript/lib/typescript.js';
const root=fileURLToPath(new URL('../',import.meta.url)),out=root+'/.validation/caption';
await mkdir(out,{recursive:true});
const source=await readFile(root+'/app/src/windows/captions.ts','utf8');
await writeFile(out+'/captions-check.mjs',ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText);
const {mergeCaptions,captionSnapshot,captionTail,followCaptionOffset}=await import(pathToFileURL(out+'/captions-check.mjs'));
const tests=[];const test=(name,run)=>{try{run();tests.push({name,status:'PASS'});}catch(error){tests.push({name,status:'FAIL',error:String(error)});}};
const item=(id,text,revision=1,final=false,captureOrder=0)=>({id,text,revision,final,captureOrder});
test('Revision replacements do not repeat text; stale updates are ignored',()=>{
 let value=[];for(let revision=1;revision<=20;revision++)value=mergeCaptions(value,[item('a','修订'+revision,revision)]);
 assert.equal(value.length,1);assert.equal(value[0].text,'修订20');assert.equal(mergeCaptions(value,[item('a','迟到',3)]),value);assert.equal(mergeCaptions(value,[item('a','相同 revision 的不同文字',20)]),value);
});
test('Final content is immutable while predecessor order can resolve late',()=>{
 const final=item('a','不能删除这个不',2,true,2),value=mergeCaptions([final],[item('a','被错误覆盖',99,false,1),item('b','前一句',1,true,0)]);
 assert.deepEqual(value.map(i=>i.id),['b','a']);assert.equal(value[1].text,final.text);assert.equal(value[1].revision,2);assert.equal(value[1].final,true);
});
test('Shared final object identity is retained during tentative revisions',()=>{
 const final=item('a','稳定句',2,true),before=[final,item('b','正在',1,false,1)],after=mergeCaptions(before,[final,item('b','正在修订',2,false,1)]);
 assert.equal(after[0],before[0]);assert.notEqual(after[1],before[1]);
});
test('Capture order is authoritative even when snapshots arrive out of order',()=>{
 const value=mergeCaptions([],[item('z','第三',1,false,2),item('a','第一',1,true,0),item('m','第二',1,true,1)]);
 assert.deepEqual(value.map(i=>i.text),['第一','第二','第三']);
});
test('Collapsed DOM tail is bounded but full source data is retained',()=>{
 const all=Array.from({length:360},(_,i)=>item(String(i),'第'+i+'段 '+('长文本'.repeat(30)),1,true,i)),tail=captionTail(all);
 assert(tail.length<=8);assert(tail.map(i=>Array.from(i.text).length).reduce((a,b)=>a+b,0)<=1201);assert.equal(all.length,360);assert.equal(tail.at(-1).id,'359');
});
test('Long Unicode item clipping preserves code points',()=>{
 const tail=captionTail([item('a','🙂'.repeat(1500))]);assert.equal(Array.from(tail[0].text).length,1201);assert(!tail[0].text.includes('\uFFFD'));
});
test('Short tail corrections do not pull the prefix right; major corrections keep text visible',()=>{
 assert.equal(followCaptionOffset(500,430,400),500);assert.equal(followCaptionOffset(500,40,400),180);assert.equal(followCaptionOffset(500,0,400),0);
});
test('Legacy full text is readable with versioned updates and typed-empty DTO does not resurrect old text',()=>{
 const old=captionSnapshot({generation:1,state:'recording',partial:'旧版'},[]);const next=captionSnapshot({generation:1,state:'recording',partial:'旧版更新'},old);
 assert(next[0].revision>old[0].revision);assert.equal(captionSnapshot({generation:1,state:'recording',partial:'旧版残留',captions:[]},old).length,0);
});
const receipt={scope:'Deterministic display/protocol invariants. No real ASR accuracy, latency or user task results.',status:tests.every(t=>t.status==='PASS')?'PASS':'FAIL',tests,sourceSha256:createHash('sha256').update(source).digest('hex')};
await writeFile(out+'/helper-receipt.json',JSON.stringify(receipt,null,2)+'\n');console.log(JSON.stringify(receipt,null,2));process.exitCode=receipt.status==='PASS'?0:1;
