// Actual component callbacks with synthetic hooks. No browser/native/ASR access.
import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
import ts from '../app/node_modules/typescript/lib/typescript.js';
const root=fileURLToPath(new URL('../',import.meta.url)),out=root+'/.validation/term-correction-component';
await mkdir(out,{recursive:true});
for(const [name,path] of [['restoreSelection','components/restoreSelection'],['correctionSelection','components/correctionSelection'],['dictionary','dictionary']]) {
 const source=await readFile(root+'/app/src/'+path+'.ts','utf8');
 await writeFile(out+'/'+name+'.mjs',ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText);
}
const stubPath=out+'/stubs.mjs';
await writeFile(stubPath,`
let cursor=0,slots=[],effects=[],dirty=false,desktop,actions,guard;
const navigation={register:value=>{guard=value;}};
export const environment=(d,a)=>{desktop=d;actions=a;};
export const getGuard=()=>guard;
export const reset=()=>{cursor=0;slots=[];effects=[];dirty=false;guard=null;};
export const begin=()=>{cursor=0;dirty=false;effects=[];};
export const flush=()=>{for(const effect of effects)effect();return dirty;};
export const useState=initial=>{const index=cursor++;if(!(index in slots))slots[index]=typeof initial==='function'?initial():initial;return [slots[index],value=>{const next=typeof value==='function'?value(slots[index]):value;if(next!==slots[index]){slots[index]=next;dirty=true;}}];};
export const useRef=initial=>{const index=cursor++;return slots[index]??(slots[index]={current:initial});};
export const useEffect=(effect,deps)=>{const index=cursor++,previous=slots[index];if(!previous||!deps||deps.some((value,i)=>value!==previous.deps?.[i])){effects.push(()=>{previous?.cleanup?.();slots[index]={deps,cleanup:effect()};});}};
export const useCallback=fn=>fn,useId=()=>{const index=cursor++;return slots[index]??(slots[index]='synthetic-'+index);};
export const useDesktop=()=>desktop,useDictations=()=>actions,useNavigationGuard=()=>navigation;
export const focusInView=()=>{},errorMessage=value=>value.message??String(value);
export const Button='button',Checkbox='checkbox',TextArea='textarea',TextField={Root:'input'},Select={Root:'select',Trigger:'selectTrigger',Content:'selectContent',Item:'selectItem'};
export const Fragment='fragment',element=(type,props,...children)=>({type,props:{...props,children:children.length===1?children[0]:children}});
`);
const source=await readFile(root+'/app/src/components/TermCorrectionPanel.tsx','utf8');
const imports=new Map([
 ['./restoreSelection',pathToFileURL(out+'/restoreSelection.mjs').href],['./correctionSelection',pathToFileURL(out+'/correctionSelection.mjs').href],['../dictionary',pathToFileURL(out+'/dictionary.mjs').href],
 ...['react','@radix-ui/themes','../services/DesktopProvider','../hooks/useDictations','../services/bridge','./NavigationGuard','./focusInView'].map(name=>[name,pathToFileURL(stubPath).href])
]);
const rewriteImports=context=>node=>ts.visitNode(node,function visit(current){
 if(ts.isImportDeclaration(current)&&ts.isStringLiteral(current.moduleSpecifier)){
  if(current.importClause?.isTypeOnly)return undefined;
  const name=current.moduleSpecifier.text,replacement=imports.get(name);assert(replacement,'Unmapped component dependency '+name);
  return ts.factory.updateImportDeclaration(current,current.modifiers,current.importClause,ts.factory.createStringLiteral(replacement),current.attributes);
 }
 return ts.visitEachChild(current,visit,context);
});
const component=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext,jsx:ts.JsxEmit.React,jsxFactory:'element',jsxFragmentFactory:'Fragment'},transformers:{before:[rewriteImports]}}).outputText;
await writeFile(out+'/component.mjs',`import {element,Fragment} from ${JSON.stringify(pathToFileURL(stubPath).href)};\n`+component);
const {TermCorrectionPanel}=await import(pathToFileURL(out+'/component.mjs')),hooks=await import(pathToFileURL(stubPath));
const nodes=tree=>{const values=[];const visit=value=>{if(Array.isArray(value))return value.forEach(visit);if(!value||typeof value!=='object')return;values.push(value);visit(value.props?.children);};visit(tree);return values;};
const find=(tree,test)=>nodes(tree).find(test),button=(tree,text)=>find(tree,node=>node.type==='button'&&node.props.children===text);
const text=tree=>{const parts=[];const visit=value=>{if(typeof value==='string')parts.push(value);else if(Array.isArray(value))value.forEach(visit);else if(value&&typeof value==='object')visit(value.props?.children);};visit(tree);return parts.join(' ');};
const flush=()=>new Promise(resolve=>setImmediate(resolve));
function setup({entries=[],handler,temporary=false}={}) {
 const calls=[],messages=[];let cancelled=0;
 let record={id:'session-a',revision:'output-a',temporary,detail:{output:{id:'output-a',blocks:[{id:'block-a',text:'🙂飞格玛和飞格玛'}]}}};
 const desktop={available:true,dictionary:{version:1,entries},refresh:async()=>{},loadRecord:async()=>record};
 const actions={correctTerm:async(...args)=>{calls.push(args);if(handler)return handler(...args);return {dictionaryStatus:args[2]?'added':'notRequested'};}};
 hooks.reset();hooks.environment(desktop,actions);
 const close=()=>{cancelled++;},saved=message=>messages.push(message);
 const render=()=>{for(let attempt=0;attempt<5;attempt++){hooks.begin();const tree=TermCorrectionPanel({record,onClose:close,onSaved:saved});if(!hooks.flush())return tree;}throw Error('Unsettled synthetic render');};
 const select=(start=2,end=5)=>{find(render(),node=>node.type==='textarea').props.onSelect({currentTarget:{selectionStart:start,selectionEnd:end}});};
 const fill=(value='Figma')=>{find(render(),node=>node.type==='input').props.onChange({target:{value}});};
 const remember=()=>{find(render(),node=>node.type==='checkbox').props.onCheckedChange(true);};
 return {render,select,fill,remember,calls,messages,desktop,getCancelled:()=>cancelled,setRecord:next=>{record=next;},record:()=>record};
}
const tests=[];
async function test(name,run){try{await run();tests.push({name,status:'PASS'});}catch(error){tests.push({name,status:'FAIL',error:String(error)});}}
await test('No implicit selection or remembering; one confirmed occurrence yields one version-bound native call',async()=>{
 const env=setup();assert.equal(find(env.render(),node=>node.type==='checkbox').props.checked,false);
 env.fill();assert.equal(button(env.render(),'保存这一处').props.disabled,true);
 env.select();assert.equal(button(env.render(),'保存这一处').props.disabled,false);env.remember();
 button(env.render(),'纠正并记住').props.onClick();await flush();assert.equal(env.calls.length,1);
 assert.equal(env.calls[0][0].revision,'output-a');assert.deepEqual(env.calls[0][1],{blockId:'block-a',start:1,end:4,expectedText:'飞格玛',replacement:'Figma'});assert.equal(env.calls[0][2],true);assert.equal(env.calls[0][3],1);
 assert.match(env.messages[0],/下次口述/);assert.match(env.messages[0],/其他应用中的文本未改变/);
});
await test('Cancel never writes a word or local revision',()=>{
 const env=setup();env.select();env.fill();env.remember();button(env.render(),'取消纠正').props.onClick();assert.equal(env.getCancelled(),1);assert.equal(env.calls.length,0);assert.equal(env.messages.length,0);
});
await test('Stale output preserves draft but requires a fresh selected range before save',()=>{
 const env=setup();env.select();env.fill();env.setRecord({...env.record(),revision:'output-b',detail:{output:{id:'output-b',blocks:[{id:'block-a',text:'另一个位置'}]}}});
 assert.equal(button(env.render(),'保存这一处').props.disabled,true);assert.equal(find(env.render(),node=>node.type==='input').props.value,'Figma');
 button(env.render(),'重新选择当前版本').props.onClick();assert.equal(button(env.render(),'保存这一处').props.disabled,true);assert.equal(env.calls.length,0);
});
await test('Dictionary change requires explicit refresh acknowledgement; full dictionary allows correction without remember',()=>{
 const env=setup();env.select();env.fill();env.remember();env.desktop.dictionary={version:2,entries:[]};
 assert.equal(button(env.render(),'纠正并记住').props.disabled,true);button(env.render(),'使用最新词典状态').props.onClick();assert.equal(button(env.render(),'纠正并记住').props.disabled,false);
 env.desktop.dictionary.entries=Array.from({length:200},(_,i)=>({id:String(i),term:'term'+i}));assert.equal(button(env.render(),'纠正并记住').props.disabled,false);assert.match(text(env.render()),/仍可使用已有词条/);
 find(env.render(),node=>node.type==='checkbox').props.onCheckedChange(false);assert.equal(button(env.render(),'保存这一处').props.disabled,false);
});
await test('Failure retains draft, never reports saved, and exposes the error',async()=>{
 const env=setup({handler:async()=>{throw new Error('词库容量已满，未保存。');}});env.select();env.fill();env.remember();button(env.render(),'纠正并记住').props.onClick();await flush();
 assert.equal(env.calls.length,1);assert.equal(env.messages.length,0);assert.equal(find(env.render(),node=>node.type==='input').props.value,'Figma');assert.match(text(env.render()),/词库容量已满/);
});
await test('Pending save blocks duplicate clicks and navigation discard; memory-only persistence boundary is visible',async()=>{
 let finish;const env=setup({temporary:true,handler:()=>new Promise(resolve=>{finish=resolve;})});env.select();env.fill();env.remember();assert.match(text(env.render()),/本次正文不保存/);
 const save=button(env.render(),'纠正并记住');save.props.onClick();save.props.onClick();assert.equal(env.calls.length,1);env.render();assert.equal(hooks.getGuard().pending,true);assert.equal(button(env.render(),'取消纠正').props.disabled,true);
 finish({dictionaryStatus:'existing'});await flush();assert.equal(env.messages.length,1);assert.match(env.messages[0],/已有此词/);
});
const receipt={scope:'Actual TermCorrectionPanel render/callbacks under synthetic React hooks; not DOM, native transaction or speech accuracy evidence',status:tests.every(test=>test.status==='PASS')?'PASS':'FAIL',componentSha256:createHash('sha256').update(source).digest('hex'),tests};
await writeFile(out+'/receipt.json',JSON.stringify(receipt,null,2));console.log(JSON.stringify(receipt,null,2));process.exitCode=receipt.status==='PASS'?0:1;
