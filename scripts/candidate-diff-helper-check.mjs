// Synthetic edit-choice invariants, without provider/user-history access.
import assert from 'node:assert/strict';
import {readFile,mkdir,writeFile} from 'node:fs/promises';
import {fileURLToPath,pathToFileURL} from 'node:url';
import ts from '../app/node_modules/typescript/lib/typescript.js';
const root=fileURLToPath(new URL('../',import.meta.url)),out=root+'/.validation/candidate-diff';
await mkdir(out,{recursive:true});
const source=await readFile(root+'/app/src/components/candidateDiff.ts','utf8');
const compiled=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText.replace("from 'diff'",`from '${pathToFileURL(root+'/app/node_modules/diff/libesm/index.js').href}'`);
const path=out+'/candidate-diff.mjs';await writeFile(path,compiled);
const {candidateDiff,composeCandidate}=await import(pathToFileURL(path));
const rows=[];
function check(name,run){try{run();rows.push({name,status:'PASS'});}catch(error){rows.push({name,status:'FAIL',error:String(error)});}}
function roundtrip(before,after){const plan=candidateDiff(before,after),changes=plan.parts.filter(p=>p.kind==='change');assert.equal(composeCandidate(plan,new Set()),after);assert.equal(composeCandidate(plan,new Set(changes.map(p=>p.id))),before);return plan;}
check('Unchanged source has no invented choices',()=>{const d=roundtrip('时间未确认。','时间未确认。');assert.equal(d.granularity,'unchanged');});
check('Selective numeric restoration preserves other cleanup',()=>{
 const before='预算三千五百元。\n\n如果时间没确认，先不要发邀请。',after='预算五千元。\n\n如果时间没确认，先发邀请。';
 const plan=roundtrip(before,after),changes=plan.parts.filter(p=>p.kind==='change');assert.equal(changes.length,2);
 assert.equal(composeCandidate(plan,new Set([changes[0].id])),'预算三千五百元。\n\n如果时间没确认，先发邀请。');
 assert.equal(composeCandidate(plan,new Set([changes[1].id])),'预算五千元。\n\n如果时间没确认，先不要发邀请。');
});
check('Unicode, combining marks and newline bytes stay exact',()=>roundtrip('e\u0301👩‍💻\r\n否定🙂保留。','é👩‍💻\n保留🙂否定。'));
check('Insert-only and delete-only choices restore exact text',()=>{roundtrip('','新增文本');roundtrip('旧文本','');});
check('Long common prose supports localized edits',()=>{const before='保留每个条件和对象。'.repeat(120)+'不要删除。',after=before.replace('不要删除。','删除。');const p=roundtrip(before,after);assert.equal(p.granularity,'characters');});
check('Oversized comparisons fall back explicitly with no truncation',()=>{const p=roundtrip('长'.repeat(80000),'短'.repeat(70000));assert.equal(p.granularity,'whole');});
check('Many unrelated edits remain bounded and retain both texts',()=>{roundtrip('abc。'.repeat(1000),'xyz！'.repeat(1000));});
check('Deterministic Unicode challenge reconstructs both sides and every mixed selection',()=>{
 let seed=20261012;const next=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed;};
 const units=['甲','乙','不','🙂','👩‍💻','\n','\r','é','e\u0301',' ','1','；'];
 for(let n=0;n<400;n++){
  const a=Array.from({length:8+next()%80},()=>units[next()%units.length]).join('');
  const b=Array.from({length:8+next()%80},()=>units[next()%units.length]).join('');
  const plan=roundtrip(a,b),kept=new Set(plan.parts.filter(p=>p.kind==='change'&&p.id%2===0).map(p=>p.id));
  const expected=plan.parts.map(p=>p.kind==='same'?p.text:kept.has(p.id)?p.before:p.after).join('');
  assert.equal(composeCandidate(plan,kept),expected);
 }
});
await writeFile(out+'/receipt.json',JSON.stringify({scope:'Edit-choice invariants, not semantic correctness',rows},null,2));
console.log(JSON.stringify(rows,null,2));if(rows.some(r=>r.status==='FAIL'))process.exitCode=1;
