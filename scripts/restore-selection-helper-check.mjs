// Synthetic text/selection tests. Never access native storage, user content,
// clipboard, microphone, ASR or rewrite services.
import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {pathToFileURL,fileURLToPath} from 'node:url';
import ts from '../app/node_modules/typescript/lib/typescript.js';

const root=fileURLToPath(new URL('../',import.meta.url)),out=root+'/.validation/restore-selection';
await mkdir(out,{recursive:true});
const helperSource=await readFile(root+'/app/src/components/restoreSelection.ts','utf8');
const helperPath=out+'/restore-selection-check.mjs';
await writeFile(helperPath,ts.transpileModule(helperSource,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText);
const {captureRestoreSelection,buildRestorePreview}=await import(pathToFileURL(helperPath));
const tests=[];
const test=async(name,run)=>{try{await run();tests.push({name,status:'PASS'});}catch(error){tests.push({name,status:'FAIL',error:String(error)});}};
const text=(id,value,revisionId='frozen-r1')=>({id,text:value,revisionId});

await test('A zero-width caret inserts a missing qualifier without deleting adjacent text',()=>{
 const source=text('s','今天'),target=text('b','进行测试。');
 const preview=buildRestorePreview(source,target,captureRestoreSelection(source,0,2),captureRestoreSelection(target,0,0));
 assert.equal(preview.kind,'insert');assert.equal(preview.text,'今天进行测试。');
 assert.deepEqual(preview.patch,{blockId:'b',start:0,end:0,sourceSelections:[{itemId:'s',start:0,end:2}]});
});
await test('Collapsed source selection invalidates the previous recovery text',()=>{
 const source=text('s','不要发邀请'),target=text('b','发邀请');
 assert(buildRestorePreview(source,target,captureRestoreSelection(source,0,2),captureRestoreSelection(target,0,0)));
 assert.equal(buildRestorePreview(source,target,captureRestoreSelection(source,1,1),captureRestoreSelection(target,0,0)),null);
});
await test('Emoji before target selection yields native scalar offsets and leaves other text intact',()=>{
 const source=text('s','不要'),target=text('b','🙂可以发邀请。另一句保持不变。');
 const preview=buildRestorePreview(source,target,captureRestoreSelection(source,0,2),captureRestoreSelection(target,2,4));
 assert.equal(preview.kind,'replace');assert.equal(preview.replacedText,'可以');
 assert.equal(preview.text,'🙂不要发邀请。另一句保持不变。');
 assert.deepEqual([preview.patch.start,preview.patch.end],[1,3]);
});
await test('Combining characters and ZWJ emoji follow the existing Unicode scalar contract without normalization',()=>{
 const source=text('s','e\u0301👩‍💻'),target=text('b','开头🙂结尾');
 const preview=buildRestorePreview(source,target,captureRestoreSelection(source,0,source.text.length),captureRestoreSelection(target,4,4));
 assert.equal(preview.text,'开头🙂e\u0301👩‍💻结尾');
 assert.deepEqual(preview.patch.sourceSelections,[{itemId:'s',start:0,end:5}]);
 assert.equal(preview.patch.start,3);
});
await test('Surrogate splits, invalid DOM offsets and stale revision/text/identity are rejected instead of guessed',()=>{
 const source=text('s','🙂不要'),target=text('b','原文本');
 for(const [start,end] of [[1,2],[0,1],[-1,0],[0,99],[2,1],[0.5,2],[0,NaN]])assert.equal(captureRestoreSelection(source,start,end),null);
 const sourceIntent=captureRestoreSelection(source,2,4),targetIntent=captureRestoreSelection(target,0,0);
 for(const changed of [{...target,revisionId:'r2'},{...target,id:'other'},{...target,text:'发生变化'}])assert.equal(buildRestorePreview(source,changed,sourceIntent,targetIntent),null);
 assert.equal(buildRestorePreview({...source,revisionId:'source-r2'},target,sourceIntent,targetIntent),null);
});

// Exercise the actual HistoryDetail callbacks and renders using synthetic hook
// slots. This is component logic validation, not a browser DOM/selection test.
const componentSource=await readFile(root+'/app/src/components/HistoryDetail.tsx','utf8');
const stubsPath=out+'/component-stubs.mjs';
await writeFile(stubsPath,`
let cursor=0,slots=[],effects=[],dirty=false,desktop,actions;
const navigation={register:()=>{},request:action=>action()};
export const setEnvironment=(d,a)=>{desktop=d;actions=a;};
export const resetHooks=()=>{cursor=0;slots=[];effects=[];dirty=false;};
export const beginRender=()=>{cursor=0;dirty=false;effects=[];};
export const flushEffects=()=>{for(const effect of effects)effect();return dirty;};
export const useState=initial=>{const index=cursor++;if(!(index in slots))slots[index]=typeof initial==='function'?initial():initial;return [slots[index],value=>{const next=typeof value==='function'?value(slots[index]):value;if(next!==slots[index]){slots[index]=next;dirty=true;}}];};
export const useRef=initial=>{const index=cursor++;return slots[index]??(slots[index]={current:initial});};
export const useEffect=(effect,deps)=>{const index=cursor++,previous=slots[index];if(!previous||!deps||deps.some((value,i)=>value!==previous.deps?.[i])){effects.push(()=>{previous?.cleanup?.();slots[index]={deps,cleanup:effect()};});}};
export const useMemo=fn=>fn(),useCallback=fn=>fn;
export const useId=()=>{const index=cursor++;return slots[index]??(slots[index]='synthetic-id-'+index);};
export const useDesktop=()=>desktop,useDictations=()=>actions,useModes=()=>({findMode:()=>null});
export const useNavigationGuard=()=>navigation;
export const focusInView=()=>{},errorMessage=error=>String(error),isPersonalMode=()=>false;
export const characterDiff=(source,current)=>({source:[{text:source,changed:false}],current:[{text:current,changed:false}]});
export const Button='button',TextArea='textarea',Select={Root:'selectRoot',Trigger:'selectTrigger',Content:'selectContent',Item:'selectItem'};
export const ModeBadge='modeBadge',ModeSelector='modeSelector',RecordText='recordText',CopyAction='copyAction';
export const ArrowLeftIcon='icon',ChevronDownIcon='icon',ChevronUpIcon='icon',Pencil2Icon='icon',ReaderIcon='icon',ResetIcon='icon',TrashIcon='icon';
export const Fragment='fragment',element=(type,props,...children)=>({type,props:{...props,children:children.length===1?children[0]:children}});
`);
const imports=new Map([
 ['./restoreSelection',pathToFileURL(helperPath).href],
 ...['react','@radix-ui/themes','@radix-ui/react-icons','../model','./ui','./CopyAction','./focusInView','../hooks/useModes','../modes','../hooks/useDictations','../services/DesktopProvider','../services/bridge','./NavigationGuard'].map(name=>[name,pathToFileURL(stubsPath).href]),
]);
const replaceImports=context=>node=>ts.visitNode(node,function visit(current){
 if(ts.isImportDeclaration(current)&&ts.isStringLiteral(current.moduleSpecifier)){
  const name=current.moduleSpecifier.text;if(name.endsWith('.css'))return undefined;
  const replacement=imports.get(name);assert(replacement,`Unregistered restore component test dependency: ${name}`);
  return ts.factory.updateImportDeclaration(current,current.modifiers,current.importClause,ts.factory.createStringLiteral(replacement),current.attributes);
 }
 return ts.visitEachChild(current,visit,context);
});
const compiled=ts.transpileModule(componentSource,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext,jsx:ts.JsxEmit.React,jsxFactory:'element',jsxFragmentFactory:'Fragment'},transformers:{before:[replaceImports]}}).outputText;
const componentPath=out+'/history-detail-check.mjs';
await writeFile(componentPath,`import {element} from ${JSON.stringify(pathToFileURL(stubsPath).href)};\n`+compiled);
const {HistoryDetail}=await import(pathToFileURL(componentPath));
const hooks=await import(pathToFileURL(stubsPath));
const allNodes=tree=>{const found=[];const visit=node=>{if(Array.isArray(node))return node.forEach(visit);if(!node||typeof node!=='object')return;found.push(node);visit(node.props?.children);};visit(tree);return found;};
const find=(tree,predicate)=>allNodes(tree).find(predicate);
const restoreButton=tree=>find(tree,node=>node.type==='button'&&['确认插入这部分','确认替换这部分'].includes(node.props.children));
const select= (tree,label)=>find(tree,node=>node.type==='selectRoot'&&allNodes(node).some(child=>child.props?.['aria-label']===label));
const textarea=(tree,label)=>find(tree,node=>node.type==='textarea'&&node.props['aria-label']===label);
const selectEvent=(start,end)=>({currentTarget:{selectionStart:start,selectionEnd:end}});
function setup(){
 const calls=[];let record={id:'synthetic-session',revision:'output-r1',sourceRevisionId:'source-r1',title:'合成恢复测试',date:'2026-10-08',time:'12:00',mode:'结构',modeName:'结构',sourceState:'model',current:['发邀请。余下内容。','第二段保持。'],source:['不要','今天'],modified:false,undoAvailable:true,
  detail:{source:{id:'source-r1',complete:true,items:[{id:'source-1',text:'不要'},{id:'source-2',text:'今天'}]},output:{id:'output-r1',blocks:[{id:'block-1',text:'发邀请。余下内容。',sourceRefs:['source-1']},{id:'block-2',text:'第二段保持。',sourceRefs:['source-2']}]}}};
 hooks.resetHooks();hooks.setEnvironment({available:true,command:async()=>{},refresh:async()=>{}},{restore:async(value,patch)=>{calls.push({record:value,patch});},cancelCandidate:async()=>{},edit:async()=>{}});
 const render=()=>{let tree;for(let attempt=0;attempt<5;attempt++){hooks.beginRender();tree=HistoryDetail({record,onBack:()=>{}});if(!hooks.flushEffects())return tree;}throw Error('Synthetic render did not settle');};
 const open=()=>{let tree=render();find(tree,node=>node.type==='button'&&node.props['aria-controls']&&node.props.className==='history-compare-toggle').props.onClick();return render();};
 return {render,open,calls,setRecord:next=>{record=next;},getRecord:()=>record};
}
await test('Choosing a target does not default to whole-block replacement; explicit pointer caret works at position zero',()=>{
 const env=setup();let tree=env.open();select(tree,'选择识别来源').props.onValueChange('source-1');tree=env.render();select(tree,'选择恢复目标').props.onValueChange('block-1');tree=env.render();assert.equal(restoreButton(tree),undefined);
 textarea(tree,'选择当前文本中的插入或替换位置').props.onPointerUp(selectEvent(0,0));tree=env.render();assert.equal(restoreButton(tree).props.children,'确认插入这部分');
});
await test('Actual callbacks collapse a target replacement into an insertion and clear a collapsed source',()=>{
 const env=setup();let tree=env.open();select(tree,'选择识别来源').props.onValueChange('source-1');tree=env.render();select(tree,'选择恢复目标').props.onValueChange('block-1');tree=env.render();
 textarea(tree,'选择当前文本中的插入或替换位置').props.onSelect(selectEvent(0,2));tree=env.render();assert.equal(restoreButton(tree).props.children,'确认替换这部分');
 textarea(tree,'选择当前文本中的插入或替换位置').props.onSelect(selectEvent(0,0));tree=env.render();assert.equal(restoreButton(tree).props.children,'确认插入这部分');
 textarea(tree,'选择识别稿中的恢复范围').props.onSelect(selectEvent(1,1));tree=env.render();assert.equal(restoreButton(tree),undefined);
});
await test('Changing source or target paragraph requires a fresh destination intent',()=>{
 const env=setup();let tree=env.open();select(tree,'选择识别来源').props.onValueChange('source-1');tree=env.render();select(tree,'选择恢复目标').props.onValueChange('block-1');tree=env.render();textarea(tree,'选择当前文本中的插入或替换位置').props.onKeyUp(selectEvent(0,0));tree=env.render();assert(restoreButton(tree));
 select(tree,'选择恢复目标').props.onValueChange('block-2');tree=env.render();assert.equal(restoreButton(tree),undefined);
 textarea(tree,'选择当前文本中的插入或替换位置').props.onPointerUp(selectEvent(0,0));tree=env.render();assert(restoreButton(tree));
 select(tree,'选择识别来源').props.onValueChange('source-2');tree=env.render();assert.equal(restoreButton(tree),undefined);
});
await test('A revision change clears restore intent rather than applying old offsets to new text',()=>{
 const env=setup();let tree=env.open();select(tree,'选择识别来源').props.onValueChange('source-1');tree=env.render();select(tree,'选择恢复目标').props.onValueChange('block-1');tree=env.render();textarea(tree,'选择当前文本中的插入或替换位置').props.onPointerUp(selectEvent(0,0));tree=env.render();assert(restoreButton(tree));
 const old=env.getRecord();env.setRecord({...old,revision:'output-r2',detail:{...old.detail,output:{...old.detail.output,id:'output-r2'}}});tree=env.render();assert.equal(restoreButton(tree),undefined);
});
await test('Closing and reopening comparison does not reuse selections from unmounted textareas',()=>{
 const env=setup();let tree=env.open();select(tree,'选择识别来源').props.onValueChange('source-1');tree=env.render();select(tree,'选择恢复目标').props.onValueChange('block-1');tree=env.render();textarea(tree,'选择当前文本中的插入或替换位置').props.onPointerUp(selectEvent(0,2));tree=env.render();assert(restoreButton(tree));
 find(tree,node=>node.type==='button'&&node.props.className==='history-compare-toggle').props.onClick();tree=env.render();assert.equal(select(tree,'选择恢复目标'),undefined);
 find(tree,node=>node.type==='button'&&node.props.className==='history-compare-toggle').props.onClick();tree=env.render();assert.equal(restoreButton(tree),undefined);assert.equal(select(tree,'选择恢复目标').props.value,'');assert.equal(select(tree,'选择识别来源').props.value,'');
});
await test('Confirmed insertion sends only explicit source/target ranges bound to current record; completion clears intent',async()=>{
 const env=setup();let tree=env.open();select(tree,'选择识别来源').props.onValueChange('source-1');tree=env.render();select(tree,'选择恢复目标').props.onValueChange('block-1');tree=env.render();textarea(tree,'选择当前文本中的插入或替换位置').props.onPointerUp(selectEvent(0,0));tree=env.render();restoreButton(tree).props.onClick();await new Promise(resolve=>setImmediate(resolve));
 assert.equal(env.calls.length,1);assert.equal(env.calls[0].record.revision,'output-r1');assert.deepEqual(env.calls[0].patch,{blockId:'block-1',start:0,end:0,sourceSelections:[{itemId:'source-1',start:0,end:2}]});
 assert.equal(restoreButton(env.render()),undefined);
});
const receipt={scope:'Synthetic Unicode selection/preview and actual HistoryDetail render/callback logic with synthetic hook slots. No browser DOM selection, native Store acceptance, provider, user data or host insertion.',status:tests.every(value=>value.status==='PASS')?'PASS':'FAIL',helperSha256:createHash('sha256').update(helperSource).digest('hex'),componentSha256:createHash('sha256').update(componentSource).digest('hex'),tests};
await writeFile(out+'/helper-receipt.json',JSON.stringify(receipt,null,2)+'\n');console.log(JSON.stringify(receipt,null,2));process.exitCode=receipt.status==='PASS'?0:1;
