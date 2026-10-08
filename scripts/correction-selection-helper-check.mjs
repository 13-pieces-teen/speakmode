// Only synthetic text and bounded explicit selections. No user data or native I/O.
import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {fileURLToPath,pathToFileURL} from 'node:url';
import ts from '../app/node_modules/typescript/lib/typescript.js';
const root=fileURLToPath(new URL('../',import.meta.url)),out=root+'/.validation/correction-selection';
await mkdir(out,{recursive:true});
for(const name of ['restoreSelection','correctionSelection']){
 const source=await readFile(root+'/app/src/components/'+name+'.ts','utf8');
 await writeFile(out+'/'+name+'.mjs',ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText);
}
const {captureRestoreSelection}=await import(pathToFileURL(out+'/restoreSelection.mjs'));
const {buildCorrectionPreview,moveCorrectionSelection}=await import(pathToFileURL(out+'/correctionSelection.mjs'));
const tests=[];
const test=(name,run)=>{try{run();tests.push({name,status:'PASS'});}catch(error){tests.push({name,status:'FAIL',error:String(error)});}};
test('Correct only the selected occurrence and preserve Unicode scalar offsets',()=>{
 const target={id:'block-a',text:'🙂飞格玛和飞格玛',revisionId:'revision-a'};
 const preview=buildCorrectionPreview(target,captureRestoreSelection(target,2,5),'Figma');
 assert.equal(preview?.text,'🙂Figma和飞格玛');
 assert.deepEqual(preview.patch,{blockId:'block-a',start:1,end:4,expectedText:'飞格玛',replacement:'Figma'});
});
test('Stale revision, block identity and text cannot authorize a correction',()=>{
 const target={id:'b',text:'旧名',revisionId:'r1'},selection=captureRestoreSelection(target,0,2);
 for(const changed of [{...target,id:'elsewhere'},{...target,revisionId:'r2'},{...target,text:'新文本'}])assert.equal(buildCorrectionPreview(changed,selection,'SpeakMode'),null);
});
test('Collapsed, surrogate-split, invalid and oversized ranges cannot produce a patch',()=>{
 const target={id:'b',text:'🙂'+ 'x'.repeat(100),revisionId:'r1'};
 for(const [start,end] of [[0,0],[0,1],[1,2],[3,2],[-1,1],[0,1000],[2,84]])assert.equal(buildCorrectionPreview(target,captureRestoreSelection(target,start,end),'Figma'),null);
});
test('Empty, unchanged, control, hidden, ill-formed and oversized replacements are rejected',()=>{
 const target={id:'b',text:'旧名',revisionId:'r1'},selection=captureRestoreSelection(target,0,2);
 for(const replacement of ['', ' ', '旧名','Figma\n','Fig\u200bma','Fig\u2028ma','\ud800','x'.repeat(81)])assert.equal(buildCorrectionPreview(target,selection,replacement),null);
});
test('Keyboard selection skips surrogate interiors and extends or shrinks around a stable anchor',()=>{
 assert.deepEqual(moveCorrectionSelection('🙂旧名',0,0,'none','ArrowRight',false),{start:2,end:2,direction:'forward'});
 assert.deepEqual(moveCorrectionSelection('🙂旧名',2,2,'none','ArrowRight',true),{start:2,end:3,direction:'forward'});
 assert.deepEqual(moveCorrectionSelection('🙂旧名',2,3,'forward','ArrowLeft',true),{start:2,end:2,direction:'forward'});
 assert.deepEqual(moveCorrectionSelection('🙂旧名',2,2,'forward','ArrowLeft',true),{start:0,end:2,direction:'backward'});
 for(const key of ['Home','End','ArrowUp','ArrowDown','a','c'])assert.equal(moveCorrectionSelection('line\nline',2,2,'none',key,false),null);
 assert.equal(moveCorrectionSelection('🙂旧名',0,0,'none','ArrowRight',false,true),null);
});
const receipt={status:tests.every(t=>t.status==='PASS')?'PASS':'FAIL',tests};
await writeFile(out+'/receipt.json',JSON.stringify(receipt,null,2));
console.log(JSON.stringify(receipt,null,2));
process.exitCode=receipt.status==='PASS'?0:1;
