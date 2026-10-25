// Frontend Unicode and configuration admission only. No storage or cloud use.
import assert from 'node:assert/strict';
import {readFile,mkdtemp,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import ts from '../app/node_modules/typescript/lib/typescript.js';

const directory=await mkdtemp(join(tmpdir(),'speakmode-personalization-helpers-'));
try {
 const source=await readFile(new URL('../app/src/services/personalization.ts',import.meta.url),'utf8');
 const module=join(directory,'personalization.mjs');
 await writeFile(module,ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText);
 const {defaultStyle,personalizationError,isPersonalizationConfig,personalizationConflict,hasPersonalizationForScene}=await import(pathToFileURL(module));
 const fresh=()=>({version:1,enabled:true,activeProfileId:'p1',profiles:[{id:'p1',name:'研究',profession:'学生',domain:'',description:'',tasks:[],audience:'',scenes:['document'],instructions:''}],preferences:{...defaultStyle},sceneOverrides:[]});
 const results=[];
 const test=(name,run)=>{try{run();results.push({name,status:'PASS'});}catch(error){results.push({name,status:'FAIL',error:String(error)});}};
 test('Global habits can be saved without any profession or identity',()=>{const config=fresh();config.profiles=[];config.activeProfileId=null;config.preferences.paragraphs=true;assert.equal(personalizationError(config),'');assert(isPersonalizationConfig(config));});
 test('Unicode limit is scalar based and never truncates pasted work descriptions',()=>{const config=fresh();config.profiles[0].description='😀'.repeat(300);assert.equal(personalizationError(config),'');config.profiles[0].description+='😀';assert.match(personalizationError(config),/300/);assert.equal(Array.from(config.profiles[0].description).length,301);});
 test('Name, domain, audience and instructions match backend length limits',()=>{for(const [key,limit] of [['name',80],['domain',120],['audience',120],['instructions',500]]){const config=fresh();config.profiles[0][key]='字'.repeat(limit);assert.equal(personalizationError(config),'');config.profiles[0][key]+='字';assert.notEqual(personalizationError(config),'');}});
 test('Unpaired surrogate and invisible formatting are refused while ordinary line breaks remain valid',()=>{for(const value of ['x\ud800','a\u200bb','x\u0000']){const config=fresh();config.profiles[0].description=value;assert.notEqual(personalizationError(config),'');}const config=fresh();config.profiles[0].description='第一段\n第二段';assert.equal(personalizationError(config),'');});
 test('A missing selected identity cannot be saved',()=>{const config=fresh();config.activeProfileId='deleted';assert.notEqual(personalizationError(config),'');});
 test('Four identities and nine tasks are rejected',()=>{const config=fresh();config.profiles=Array.from({length:4},(_,i)=>({...config.profiles[0],id:`p${i}`}));assert.match(personalizationError(config),/3/);const next=fresh();next.profiles[0].tasks=Array(9).fill('task');assert.match(personalizationError(next),/8/);});
 test('Duplicate scene overrides cannot be accepted',()=>{const config=fresh();config.sceneOverrides=[{scene:'email',preferences:{...defaultStyle}},{scene:'email',preferences:{...defaultStyle}}];assert.notEqual(personalizationError(config),'');});
 test('Malformed bridge replies do not become saved configuration',()=>{const config=fresh();for(const value of [null,{}, {...config,version:0},{...config,profiles:'bad'},{...config,preferences:{...defaultStyle,paragraphs:'true'}},{...config,sceneOverrides:[{scene:'unknown',preferences:defaultStyle}]}])assert.equal(isPersonalizationConfig(value),false);});
 test('CAS retry is offered only for typed native revision conflicts',()=>{assert(personalizationConflict({code:'revisionConflict'}));assert(personalizationConflict('{"code":"revisionConflict"}'));for(const value of [new Error('revisionConflict'),'failed',{code:'other'}])assert.equal(personalizationConflict(value),false);});
 test('Work-only identity is unavailable in personal chat but available in its explicit document scope',()=>{const config=fresh();assert.equal(hasPersonalizationForScene(config,'chat'),false);assert.equal(hasPersonalizationForScene(config,'document'),true);config.enabled=false;assert.equal(hasPersonalizationForScene(config,'document'),false);});
 test('An explicit empty scene override suppresses global style without inventing another profile',()=>{const config=fresh();config.activeProfileId=null;config.preferences.paragraphs=true;assert.equal(hasPersonalizationForScene(config,'chat'),true);config.sceneOverrides=[{scene:'chat',preferences:{...defaultStyle}}];assert.equal(hasPersonalizationForScene(config,'chat'),false);assert.equal(hasPersonalizationForScene(config,'email'),true);});
 const failed=results.filter(test=>test.status!=='PASS');
 console.log(JSON.stringify({status:failed.length?'FAIL':'PASS',scope:'Frontend configuration admission only; not native storage or model quality.',tests:results},null,2));
 process.exitCode=failed.length?1:0;
}finally{
 // Only this script's freshly created, absolute temporary directory is removed.
 await rm(directory,{recursive:true,force:true});
}
