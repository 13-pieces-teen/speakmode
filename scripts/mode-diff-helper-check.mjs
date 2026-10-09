// Real display-diff helper; synthetic Unicode only, no network or user history.
import assert from 'node:assert/strict';
import {readFile,mkdir,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {fileURLToPath,pathToFileURL} from 'node:url';
import ts from '../app/node_modules/typescript/lib/typescript.js';
const root=fileURLToPath(new URL('../',import.meta.url)),out=root+'/.validation/mode-diff';
await mkdir(out,{recursive:true});
const source=await readFile(root+'/app/src/model.ts','utf8');
const compiled=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText.replace("from 'diff'",`from '${pathToFileURL(root+'/app/node_modules/diff/libesm/index.js').href}'`);
const path=out+'/model.mjs';await writeFile(path,compiled);
const {characterDiff}=await import(pathToFileURL(path));
const tests=[];
function check(name,run){try{run();tests.push({name,status:'PASS'});}catch(error){tests.push({name,status:'FAIL',error:String(error)});}}
function preserved(a,b,result){assert.equal(result.source.map(x=>x.text).join(''),a);assert.equal(result.current.map(x=>x.text).join(''),b);}
check('Unchanged short source is explicit and has no fabricated edits',()=>{const r=characterDiff('今天下午再确认。','今天下午再确认。');assert.equal(r.granularity,'unchanged');assert.equal(r.source[0].changed,false);});
check('Negation change is visible; Unicode source and target reconstruct exactly',()=>{const a='🙂如果时间没确认，先不要发邀请。',b='🙂如果时间没确认，先发邀请。';const r=characterDiff(a,b);assert.equal(r.granularity,'characters');assert.equal(r.source.filter(x=>x.changed).map(x=>x.text).join(''),'不要');preserved(a,b,r);});
check('Combining characters, emoji and line breaks are not normalized',()=>{const a='e\u0301👩‍💻\n预算：三千五百元。',b='é👩‍💻\n设备预算：三千五百元。';preserved(a,b,characterDiff(a,b));});
check('Large asymmetric input clearly downgrades without losing either text',()=>{const a='大'.repeat(9000),b='短句';const r=characterDiff(a,b);assert.equal(r.granularity,'whole');preserved(a,b,r);});
check('Product-bound fallback exposes its granularity',()=>{const a='甲'.repeat(510),b='甲'.repeat(509)+'乙';const r=characterDiff(a,b);assert.equal(r.granularity,'whole');preserved(a,b,r);});
check('Empty-side comparison preserves complete addition/deletion',()=>{for(const [a,b] of [['','新文本'],['旧文本','']]){const r=characterDiff(a,b);assert.equal(r.granularity,'whole');preserved(a,b,r);}});
await writeFile(out+'/receipt.json',JSON.stringify({scope:'Display helper only; no semantic correctness claim',sourceSha256:createHash('sha256').update(source).digest('hex'),tests},null,2));
console.log(JSON.stringify(tests,null,2));if(tests.some(t=>t.status!=='PASS'))process.exitCode=1;
