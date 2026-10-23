// Production component events with synthetic hooks/bridge. No browser/native/provider access.
import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
import ts from '../app/node_modules/typescript/lib/typescript.js';
const root=fileURLToPath(new URL('../',import.meta.url)),out=root+'/.validation/dictionary-capacity-component';
await mkdir(out,{recursive:true});
const helper=await readFile(root+'/app/src/dictionary.ts','utf8');
await writeFile(out+'/dictionary.mjs',ts.transpileModule(helper,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText);
const stubPath=out+'/stubs.mjs';
await writeFile(stubPath,`
let cursor=0,slots=[],effects=[],dirty=false,desktop,guard;
const same=(a,b)=>a&&b&&a.length===b.length&&a.every((value,index)=>value===b[index]);
const navigation={register:value=>{guard=value;},request:action=>action()};
export const environment=value=>{desktop=value;};
export const getGuard=()=>guard;
export const reset=()=>{cursor=0;slots=[];effects=[];dirty=false;guard=null;};
export const begin=()=>{cursor=0;dirty=false;effects=[];};
export const flush=()=>{for(const effect of effects)effect();return dirty;};
export const useState=initial=>{const index=cursor++;if(!(index in slots))slots[index]=typeof initial==='function'?initial():initial;return [slots[index],value=>{const next=typeof value==='function'?value(slots[index]):value;if(next!==slots[index]){slots[index]=next;dirty=true;}}];};
export const useRef=initial=>{const index=cursor++;return slots[index]??(slots[index]={current:initial});};
export const useEffect=(effect,deps)=>{const index=cursor++,previous=slots[index];if(!previous||!same(deps,previous.deps)){effects.push(()=>{previous?.cleanup?.();slots[index]={deps,cleanup:effect()};});}};
export const useMemo=(fn,deps)=>{const index=cursor++,previous=slots[index];if(!previous||!same(deps,previous.deps))slots[index]={deps,value:fn()};return slots[index].value;};
export const useCallback=(fn,deps)=>useMemo(()=>fn,deps);
export const useDesktop=()=>desktop,useNavigationGuard=()=>navigation;
export const EmptyStateNote='emptyStateNote',DownloadIcon='svg';
export const focusInView=()=>{},errorMessage=value=>value.message??String(value);
export const Button='button',IconButton='button',TextArea='textarea',TextField={Root:'input',Slot:'slot'};
export const Dialog={Root:'dialog-root',Content:'dialog',Title:'h2',Description:'p',Close:'close'},AlertDialog={Root:'alert-root',Content:'alertdialog',Title:'h2',Description:'p',Cancel:'cancel',Action:'action'},DropdownMenu={Root:'menu-root',Trigger:'trigger',Content:'menu',Item:'menuitem',Separator:'hr'};
export const CheckIcon='svg',Cross2Icon='svg',MagnifyingGlassIcon='svg',Pencil2Icon='svg',PlusIcon='svg',ReaderIcon='svg',ReloadIcon='svg',TrashIcon='svg',DotsHorizontalIcon='svg',ChevronDownIcon='svg';
export const Fragment='fragment',element=(type,props,...children)=>({type,props:{...props,children:children.length===1?children[0]:children}});
`);
const source=await readFile(root+'/app/src/pages/DictionaryPage.tsx','utf8');
const imports=new Map([
 ['../dictionary',pathToFileURL(out+'/dictionary.mjs').href],
 ...['react','@radix-ui/themes','@radix-ui/react-icons','../services/DesktopProvider','../services/bridge','../components/NavigationGuard','../components/focusInView','../components/EmptyStateNote'].map(name=>[name,pathToFileURL(stubPath).href])
]);
const rewriteImports=context=>node=>ts.visitNode(node,function visit(current){
 if(ts.isImportDeclaration(current)&&ts.isStringLiteral(current.moduleSpecifier)){
  if(current.importClause?.isTypeOnly||current.moduleSpecifier.text.endsWith('.css'))return undefined;
  const name=current.moduleSpecifier.text,replacement=imports.get(name);assert(replacement,'Unmapped component dependency '+name);
  return ts.factory.updateImportDeclaration(current,current.modifiers,current.importClause,ts.factory.createStringLiteral(replacement),current.attributes);
 }
 return ts.visitEachChild(current,visit,context);
});
const component=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext,jsx:ts.JsxEmit.React,jsxFactory:'element',jsxFragmentFactory:'Fragment'},transformers:{before:[rewriteImports]}}).outputText;
await writeFile(out+'/component.mjs',`import {element,Fragment} from ${JSON.stringify(pathToFileURL(stubPath).href)};\n`+component);
const {DictionaryPage}=await import(pathToFileURL(out+'/component.mjs')),hooks=await import(pathToFileURL(stubPath));
globalThis.requestAnimationFrame=fn=>fn();
const nodes=tree=>{const values=[];const visit=value=>{if(Array.isArray(value))return value.forEach(visit);if(!value||typeof value!=='object')return;values.push(value);visit(value.props?.children);};visit(tree);return values;};
const text=tree=>{const parts=[];const visit=value=>{if(typeof value==='string'||typeof value==='number')parts.push(value);else if(Array.isArray(value))value.forEach(visit);else if(value&&typeof value==='object')visit(value.props?.children);};visit(tree);return parts.join(' ').replace(/\s+/g,' ');};
const find=(tree,test)=>nodes(tree).find(test),button=(tree,label)=>find(tree,node=>node.type==='button'&&(text(node)===label||node.props['aria-label']===label));
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function setup(terms=[],handler) {
 const calls=[];
 const desktop={available:true,dictionary:{version:1,entries:terms.map((term,index)=>({id:String(index),term,createdAt:'2026-10-09',updatedAt:'2026-10-09'}))},acceptDictionary:next=>{desktop.dictionary=next;},command:async(name,payload)=>{calls.push({name,payload});if(name==='dictionary.list')return desktop.dictionary;if(handler)return handler(name,payload,desktop);throw new Error('Synthetic unexpected '+name);}};
 hooks.reset();hooks.environment(desktop);
 const render=()=>{for(let attempt=0;attempt<8;attempt++){hooks.begin();const tree=DictionaryPage();if(!hooks.flush())return tree;}throw Error('Unsettled synthetic render');};
 const settle=async()=>{render();await tick();return render();};
 const fill=value=>find(render(),node=>node.props.id==='dictionary-term').props.onChange({target:{value}});
 return {render,settle,fill,calls,desktop,save:()=>hooks.getGuard().save()};
}
const tests=[];
async function test(name,run){try{await run();tests.push({name,status:'PASS'});}catch(error){tests.push({name,status:'FAIL',error:String(error)});}}
await test('Saved library displays serialized UTF8 bytes and remaining budget, not character count or provider capacity',async()=>{
 const env=setup(['A','术语','😀','"\\']);const tree=await env.settle();
 assert.match(text(tree),/已用 43 \/ 4000 字节/);assert.match(text(tree),/剩余 3957 字节/);
 const meter=find(tree,node=>node.type==='meter');assert.equal(meter.props.value,43);assert.equal(meter.props.max,4000);assert.equal(meter.props['aria-label'],'词库容量');
 assert.equal(env.calls.filter(call=>call.name!=='dictionary.list').length,0);
});
await test('Editing previews a replacement before save, warns at 4001 bytes and frees the old word budget when shortened',async()=>{
 const terms=Array.from({length:48},(_,i)=>'x'.repeat(78)+String(i).padStart(2,'0')),env=setup(terms);await env.settle();
 button(env.render(),'编辑词条 '+terms[47]).props.onClick();env.fill('x'.repeat(78)+'4é');
 let tree=env.render();assert.match(text(tree),/草稿容量上限 4001 \/ 4000 字节/);assert.match(text(tree),/可能超出 1 字节/);assert.equal(button(tree,'保存词条').props.disabled,false);
 env.fill('Short');tree=env.render();assert.match(text(tree),/草稿容量上限 3925 \/ 4000 字节/);assert.match(text(tree),/至少剩余 75 字节/);assert.doesNotMatch(text(tree),/可能超出/);
 assert.equal(find(tree,node=>node.type==='meter').props.value,4000);assert.equal(env.calls.filter(call=>call.name!=='dictionary.list').length,0);
 env.fill('x'.repeat(81));assert.doesNotMatch(text(env.render()),/草稿容量上限/);
});
await test('A full dictionary still submits Unicode-equivalent bulk terms unchanged for authoritative native deduplication',async()=>{
 const terms=['Straße',...Array.from({length:199},(_,i)=>'term'+i)],env=setup(terms,async(name,payload,desktop)=>{
  assert.equal(name,'dictionary.import');assert.deepEqual(payload,{terms:['STRASSE'],expectedVersion:1});return desktop.dictionary;
 });await env.settle();button(env.render(),'新词').props.onClick();button(env.render(),'批量添加').props.onClick();env.fill('STRASSE');
 const tree=env.render();assert.match(text(tree),/重复词尚未合并/);assert.match(text(tree),/合并前 201 条/);assert.equal(button(tree,'添加词条').props.disabled,false);
 await env.save();assert.match(text(env.render()),/已添加 0 个词条，跳过 1 个重复词条/);assert.equal(env.desktop.dictionary.entries.length,200);
 assert.equal(env.calls.filter(call=>call.name==='dictionary.import').length,1);
});
await test('Native capacity rejection retains the whole draft and never claims a partial save',async()=>{
 const env=setup([],async()=>{throw new Error('词库超出容量限制，未保存。');});await env.settle();button(env.render(),'新词').props.onClick();button(env.render(),'批量添加').props.onClick();
 const value=Array.from({length:20},(_,i)=>'术'.repeat(70)+'-'+i).join('\n');env.fill(value);assert.match(text(env.render()),/可能超出/);
 await assert.rejects(env.save(),/容量限制/);const tree=env.render();assert.equal(find(tree,node=>node.type==='textarea').props.value,value);
 assert.match(text(tree),/词库超出容量限制，未保存/);assert.doesNotMatch(text(tree),/已添加 \d/);assert.equal(env.desktop.dictionary.entries.length,0);
});
await test('Conflict refresh preserves an edit and recomputes its budget against the newer library',async()=>{
 const env=setup(['A'],async(name,payload,desktop)=>{desktop.dictionary={version:2,entries:[{id:'0',term:'A',createdAt:'2026-10-09',updatedAt:'2026-10-09'},{id:'1',term:'新词',createdAt:'2026-10-09',updatedAt:'2026-10-09'}]};throw {code:'revisionConflict'};});
 await env.settle();button(env.render(),'编辑词条 A').props.onClick();env.fill('B');await assert.rejects(env.save());const tree=env.render();
 assert.match(text(tree),/草稿仍保留/);assert.match(text(tree),/草稿容量上限 29 \/ 4000 字节/);assert.equal(find(tree,node=>node.props.id==='dictionary-term').props.value,'B');
 assert.equal(button(tree,'核对后重试').props.disabled,false);assert.equal(env.calls.filter(call=>call.name==='dictionary.save').length,1);
});
const receipt={status:tests.every(test=>test.status==='PASS')?'PASS':'FAIL',scope:'Production DictionaryPage callbacks with synthetic hooks and RPC; not DOM, native storage or speech accuracy.',sourceSha256:createHash('sha256').update(source).digest('hex'),helperSha256:createHash('sha256').update(helper).digest('hex'),tests};
await writeFile(out+'/receipt.json',JSON.stringify(receipt,null,2)+'\n');
console.log(JSON.stringify(receipt,null,2));process.exitCode=receipt.status==='PASS'?0:1;
