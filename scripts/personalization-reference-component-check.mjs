// Production provider + history view with synthetic hooks/IPC. No browser,
// native application, SQLite, credentials, network or model requests.
import assert from 'node:assert/strict';
import {readFile,mkdtemp,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join,resolve,dirname,basename} from 'node:path';
import {pathToFileURL} from 'node:url';
import ts from '../app/node_modules/typescript/lib/typescript.js';

const temporaryRoot=resolve(tmpdir());
const directory=await mkdtemp(join(temporaryRoot,'speakmode-personalization-reference-'));
if(dirname(resolve(directory))!==temporaryRoot||!basename(directory).startsWith('speakmode-personalization-reference-'))throw new Error('Refusing cleanup outside this fresh temporary fixture');
try {
 const url=name=>pathToFileURL(join(directory,name+'.mjs')).href;
 const stub=join(directory,'stubs.mjs');
 await writeFile(stub,`
let current,desktop,handler;
const same=(a,b)=>a&&b&&a.length===b.length&&a.every((value,index)=>value===b[index]);
export const setDesktop=value=>{desktop=value;},setHandler=value=>{handler=value;};
export function instance(component,{effects=false}={}){
 const scope={slots:[],cursor:0,effects:[],dirty:false};
 return {render(props={}){current=scope;scope.cursor=0;scope.effects=[];scope.dirty=false;const tree=component(props);if(effects)for(const run of scope.effects)run();return tree;},unmount(){for(const value of scope.slots)value?.cleanup?.();},scope};
}
export const useState=initial=>{const scope=current,index=scope.cursor++;if(!(index in scope.slots))scope.slots[index]=typeof initial==='function'?initial():initial;return [scope.slots[index],value=>{const next=typeof value==='function'?value(scope.slots[index]):value;if(next!==scope.slots[index]){scope.slots[index]=next;scope.dirty=true;}}];};
export const useRef=initial=>{const scope=current,index=scope.cursor++;return scope.slots[index]??(scope.slots[index]={current:initial});};
export const useMemo=(run,deps)=>{const scope=current,index=scope.cursor++,old=scope.slots[index];if(!old||!same(deps,old.deps))scope.slots[index]={deps,value:run()};return scope.slots[index].value;};
export const useCallback=(run,deps)=>useMemo(()=>run,deps);
export const useEffect=(run,deps)=>{const scope=current,index=scope.cursor++,old=scope.slots[index];if(!old||!same(deps,old.deps)){scope.effects.push(()=>{old?.cleanup?.();scope.slots[index]={deps,cleanup:run()};});}};
export const createContext=()=>({Provider:'context-provider'}),useContext=()=>desktop;
export const desktopAvailable=()=>true,desktopCommand=(name,payload)=>handler(name,payload),subscribeDesktop=async()=>({sequence:0}),errorMessage=error=>error.message??String(error);
export const initialPreferences={version:1,defaultModeId:'technical',appWriting:{outputLanguage:'asSpoken'}},initialProvider={configured:true};
export const fromDetail=(detail,temporary=false)=>({id:detail.session.id,title:detail.output.blocks[0].text,source:detail.source.items.map(item=>item.text),current:detail.output.blocks.map(block=>block.text),revision:detail.output.id,sourceRevisionId:detail.source.id,group:'今天',mode:'技术沟通',detail,temporary});
export const fromSummary=item=>({id:item.sessionId,title:item.title,revision:item.currentRevisionId,sourceRevisionId:item.sourceRevisionId,group:'今天',current:[item.preview],source:[],previewOnly:true});
export const mergeLiveLevel=(_old,next)=>next,receiveLevel=value=>value,useDesktop=()=>desktop,useNavigationGuard=()=>({request:run=>run()});
export const Button='button',IconButton='button',Spinner='spinner',TextField={Root:'input',Slot:'slot'},Cross2Icon='svg',MagnifyingGlassIcon='svg',ReaderIcon='svg',PageHeading='page-heading',HistoryListItem='history-item',HistoryDetail='history-detail';
export const element=(type,props,...children)=>({type,props:{...props,children:children.length===1?children[0]:children}});
`);
 const transpile=async(relative,name,imports={})=>{
  const source=await readFile(new URL('../'+relative,import.meta.url),'utf8');
  const transformer=context=>node=>ts.visitNode(node,function visit(current){
   if(ts.isImportDeclaration(current)&&ts.isStringLiteral(current.moduleSpecifier)){
    if(current.importClause?.isTypeOnly||current.moduleSpecifier.text.endsWith('.css'))return undefined;
    const replacement=imports[current.moduleSpecifier.text];assert(replacement,'Unmapped dependency '+current.moduleSpecifier.text);
    return ts.factory.updateImportDeclaration(current,current.modifiers,current.importClause,ts.factory.createStringLiteral(replacement),current.attributes);
   }
   return ts.visitEachChild(current,visit,context);
  });
  const output=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext,jsx:ts.JsxEmit.React,jsxFactory:'element'},transformers:{before:[transformer]}}).outputText;
  await writeFile(join(directory,name+'.mjs'),`import {element} from ${JSON.stringify(url('stubs'))};\n`+output);
 };
 await transpile('app/src/services/personalization.ts','personalization');
 await transpile('app/src/services/personalizationReferences.ts','references');
 await transpile('app/src/services/DesktopProvider.tsx','provider',Object.fromEntries([
  ...['react','./bridge','./contracts','../model','./liveLevel'].map(name=>[name,url('stubs')]),
  ['./personalization',url('personalization')],['./personalizationReferences',url('references')]
 ]));
 await transpile('app/src/pages/HistoryPage.tsx','history',Object.fromEntries([
  ...['react','@radix-ui/themes','@radix-ui/react-icons','../components/ui','../components/HistoryListItem','../components/HistoryDetail','../services/DesktopProvider','../services/bridge','../components/NavigationGuard','../model'].map(name=>[name,url('stubs')]),
  ['../services/personalizationReferences',url('references')]
 ]));
 const hooks=await import(url('stubs')),{DesktopProvider}=await import(url('provider')),{HistoryPage}=await import(url('history'));
 const {defaultStyle}=await import(url('personalization'));
 const {clearDeletedPersonalizationReferences}=await import(url('references'));
 globalThis.location={search:''};
 const tick=()=>new Promise(resolve=>setImmediate(resolve));
 const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return {promise,resolve,reject};};
 const profile=id=>({id,name:id,profession:'软件开发',domain:'',description:'背景哨兵-'+id,tasks:[],audience:'',scenes:['technical'],instructions:''});
 const config=(version=2,ids=['deleted','kept'],enabled=true)=>({version,enabled,activeProfileId:ids[0]??null,profiles:ids.map(profile),preferences:{...defaultStyle},sceneOverrides:[]});
 const detail=(id,profileId='deleted')=>({session:{id,storagePolicy:'history',status:'ready'},source:{id:'source-'+id,items:[{id:'item-'+id,text:'源文-'+id}]},output:{id:'output-'+id,sourceRevisionId:'source-'+id,blocks:[{id:'block-'+id,text:'正文-'+id}],writingReceipt:{scene:'technical',language:'asSpoken',contextUsed:false,contractVersion:'fixture',personalization:{version:2,profileId,profileName:'身份哨兵-'+profileId,scene:'technical',background:{profession:'背景哨兵-'+profileId},rules:[{key:'instructions',text:'规则哨兵-'+profileId,origin:'profile'}],omitted:['background.tasks.7']}}},canUndo:true});
 const reference=record=>record.detail.output.writingReceipt.personalization;
 const summarize=value=>({sessionId:value.session.id,title:value.output.blocks[0].text,preview:value.output.blocks[0].text,currentRevisionId:value.output.id,sourceRevisionId:value.source.id});
 function fixture(){
  let native=config(),override;
  const history=Array.from({length:4},(_,index)=>detail('h'+(index+1),index===0?'kept':'deleted'));
  hooks.setHandler(async(name,payload)=>{
   const special=override?.(name,payload);if(special!==undefined)return special;
   if(name==='personalization.get')return structuredClone(native);
   if(name==='personalization.save'){native=structuredClone(payload.config);return structuredClone(native);}
   if(name==='history.list')return {items:history.map(summarize),total:4,nextCursor:null};
   if(name==='history.get')return structuredClone(history.find(item=>item.session.id===payload.sessionId));
   if(name==='state.snapshot')return {sequence:1,temporaryResult:{...structuredClone(history[3]),session:{...history[3].session,id:'temporary',storagePolicy:'memoryOnly'}}};
   if(name==='audio.listDevices')return [];
   throw new Error('Unexpected IPC '+name);
  });
  const component=hooks.instance(DesktopProvider);
  const context=()=>{const value=component.render().props.value;hooks.setDesktop(value);return value;};
  context();
  return {context,history,setNative:value=>{native=value;},override:handler=>{override=handler;},save:value=>context().command('personalization.save',{config:value})};
 }
 const tests=[];
 const test=async(name,run)=>{try{await run();tests.push({name,status:'PASS'});}catch(error){tests.push({name,status:'FAIL',error:error.stack??String(error)});}};
 const assertCleared=record=>{const ref=reference(record);assert.equal(ref.referenceCleared,true);assert.equal(ref.profileId,'deleted');assert.equal(ref.version,2);assert.equal(ref.profileName,null);assert.equal(ref.background,null);assert.deepEqual(ref.rules,[]);assert.deepEqual(ref.omitted,[]);assert.equal(record.revision,'output-'+record.id);assert.equal(record.sourceRevisionId,'source-'+record.id);assert.deepEqual(record.current,['正文-'+record.id]);assert.deepEqual(record.source,['源文-'+record.id]);};
 await test('Successful deletion clears fourth cached detail without changing text/revision or another identity',async()=>{
  const f=fixture();await f.context().refresh();await f.context().loadRecord('h4');const before=f.context().records.find(item=>item.id==='h4');assert.match(JSON.stringify(before),/背景哨兵-deleted/);
  await f.save(config(3,['kept']));const next=f.context();assertCleared(next.records.find(item=>item.id==='h4'));assert.equal(reference(next.records.find(item=>item.id==='h1')).profileName,'身份哨兵-kept');
  assert.equal(next.temporaryResult.output.writingReceipt.personalization.referenceCleared,true);
 });
 await test('Late history.get cannot restore deleted payload in returned record or provider cache',async()=>{
  const f=fixture();await f.context().refresh();const pending=deferred();f.override(name=>name==='history.get'?pending.promise:undefined);const read=f.context().loadRecord('h4');await f.save(config(3,['kept']));pending.resolve(detail('h4'));assertCleared(await read);assertCleared(f.context().records.find(item=>item.id==='h4'));
 });
 await test('Failed save never clears still-valid cached profile references',async()=>{
  const f=fixture();await f.context().refresh();await f.context().loadRecord('h4');f.override(name=>name==='personalization.save'?Promise.reject(new Error('save failed')):undefined);await assert.rejects(f.save(config(3,['kept'])),/save failed/);assert.equal(reference(f.context().records.find(item=>item.id==='h4')).profileName,'身份哨兵-deleted');
 });
 await test('Ordinary edit and disable keep historical references',async()=>{
  const f=fixture();await f.context().refresh();await f.context().loadRecord('h4');const edited=config(3);edited.profiles[0].description='新描述';await f.save(edited);await f.save(config(4,['deleted','kept'],false));assert.equal(reference(f.context().records.find(item=>item.id==='h4')).profileName,'身份哨兵-deleted');
 });
 await test('Late pre-deletion config get cannot roll back authoritative IDs/version',async()=>{
  const f=fixture();await f.context().refresh();const pending=deferred();f.override(name=>name==='personalization.get'?pending.promise:undefined);const read=f.context().command('personalization.get');await f.save(config(3,['kept']));pending.resolve(config(2));await assert.rejects(read,error=>error.code==='revisionConflict');assert.equal(f.context().personalizationReferences.version,3);assert.deepEqual(f.context().personalizationReferences.profileIds,['kept']);
 });
 await test('Late successful older save receipt cannot restore a newer deleted identity',async()=>{
  const f=fixture();await f.context().refresh();const pending=deferred();f.override(name=>name==='personalization.save'?pending.promise:undefined);const save=f.save(config(3));f.setNative(config(4,['kept']));await f.context().command('personalization.get');pending.resolve(config(3));await assert.rejects(save,error=>error.code==='revisionConflict');assert.deepEqual(f.context().personalizationReferences.profileIds,['kept']);assert.equal(f.context().personalizationReferences.version,4);
 });
 await test('Refresh observes deletion saved elsewhere and late snapshot cannot revive temporary reference',async()=>{
  const f=fixture();await f.context().refresh();await f.context().loadRecord('h4');const pending=deferred();f.override(name=>name==='state.snapshot'?pending.promise:undefined);const refresh=f.context().refresh();await f.save(config(3,['kept']));pending.resolve({sequence:2,temporaryResult:detail('temporary')});await refresh;assert.equal(f.context().temporaryResult.output.writingReceipt.personalization.referenceCleared,true);
  f.override(undefined);f.setNative(config(4,[]));await f.context().refresh();assert.equal(reference(f.context().records.find(item=>item.id==='h1')).referenceCleared,true);
 });
 await test('No config authority does not treat unknown identity as deleted; an authoritative first get does',async()=>{
  const f=fixture();const record=await f.context().loadRecord('h4');assert.equal(reference(record).referenceCleared,undefined);f.setNative(config(3,['kept']));await f.context().command('personalization.get');assert.equal(reference(clearDeletedPersonalizationReferences(record,f.context().personalizationReferences)).referenceCleared,true);
 });
 await test('HistoryPage local fetched response is sanitized even if received after deletion',async()=>{
  const f=fixture(),pending=deferred();await f.context().command('personalization.get');let desktop={...f.context(),records:[],loadRecord:()=>pending.promise};hooks.setDesktop(desktop);
  const page=hooks.instance(HistoryPage,{effects:true});page.render({selectedId:'outside-list',onSelect:()=>{}});await f.save(config(3,['kept']));desktop={...desktop,personalizationReferences:f.context().personalizationReferences};hooks.setDesktop(desktop);
  const old={id:'outside-list',revision:'output-outside-list',sourceRevisionId:'source-outside-list',current:['正文-outside-list'],source:['源文-outside-list'],detail:detail('outside-list')};pending.resolve(old);await tick();const tree=page.render({selectedId:'outside-list',onSelect:()=>{}});
  const find=value=>{if(Array.isArray(value))return value.map(find).find(Boolean);if(!value||typeof value!=='object')return null;if(value.type==='history-detail')return value;return find(value.props?.children);};const shown=find(tree);assert(shown,'local fetched record must render');assert.equal(reference(shown.props.record).referenceCleared,true);assert.equal(reference(old).profileName,'身份哨兵-deleted','render sanitization must not mutate retained response');
 });
 await test('Unmounted history view ignores a late local fetched response',async()=>{
  const f=fixture(),pending=deferred();hooks.setDesktop({...f.context(),records:[],loadRecord:()=>pending.promise});const page=hooks.instance(HistoryPage,{effects:true});page.render({selectedId:'h4',onSelect:()=>{}});page.unmount();pending.resolve({id:'h4',detail:detail('h4')});await tick();assert.equal(page.scope.slots[3],null,'fetched local state remains empty after cleanup');
 });
 await test('Comparison and live/recovery details retain bodies while deleted reference is cleared',async()=>{
  const f=fixture();await f.context().command('personalization.get');await f.save(config(3,['kept']));const policy=f.context().personalizationReferences,ref=detail('h4').output.writingReceipt.personalization;
  const comparison={reference:ref,baseline:{text:'原文成稿'},personalized:{text:'画像成稿'},valid:true};const clean=clearDeletedPersonalizationReferences(comparison,policy);assert.equal(clean.reference.referenceCleared,true);assert.deepEqual(clean.baseline,comparison.baseline);assert.deepEqual(clean.personalized,comparison.personalized);
  const snapshot={session:{result:detail('live')},recovery:{session:{result:detail('recover')}}};const next=clearDeletedPersonalizationReferences(snapshot,policy);assert.equal(next.session.result.output.writingReceipt.personalization.referenceCleared,true);assert.equal(next.recovery.session.result.output.writingReceipt.personalization.referenceCleared,true);
 });
 const failed=tests.filter(test=>test.status!=='PASS');
 console.log(JSON.stringify({status:failed.length?'FAIL':'PASS',scope:'Production provider/history view with synthetic hooks and IPC only; no native storage or model quality evidence.',tests},null,2));
 process.exitCode=failed.length?1:0;
} finally {
 // Only this script's freshly created, absolute temporary directory is removed.
 await rm(directory,{recursive:true,force:true});
}
