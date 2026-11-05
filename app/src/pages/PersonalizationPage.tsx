import {Fragment,useCallback,useEffect,useMemo,useRef,useState} from 'react';
import {AlertDialog,Button,Checkbox,Select,Switch,TextArea,TextField} from '@radix-ui/themes';
import {ArrowRightIcon,CheckIcon,ChevronDownIcon,MixerHorizontalIcon,PersonIcon,ReaderIcon,ReloadIcon,TrashIcon} from '@radix-ui/react-icons';
import {useDesktop} from '../services/DesktopProvider';
import {useNavigationGuard} from '../components/NavigationGuard';
import {PageHeading} from '../components/ui';
import {PersonalizationReference} from '../components/PersonalizationReference';
import {candidateDiff} from '../components/candidateDiff';
import {errorMessage} from '../services/bridge';
import {hasPersonalizationForScene,isPersonalizationConfig,personalizationConflict,personalizationError,sceneNames,scenes,styleOptions,templates,type PersonalizationComparison,type PersonalizationConfig,type PersonalizationProfile,type PersonalizationStyle,type PersonalizationVerdict} from '../services/personalization';
import type {AppWritingScene} from '../appWriting';
import {clearDeletedPersonalizationReferences} from '../services/personalizationReferences';
import './personalization.css';

interface ReferenceCounts {personalized:number;selected:number;legacyUnknown:number;interruptedUnknown:number;pending:number}
interface Statistics {
 window:'week'|'all';since:string|null;normal:ReferenceCounts&{total:number};preview:ReferenceCounts&{total:number};
 comparisons:{total:number;valid:number;failed:number;fallback:number;cancelled:number;pending:number;feedback:{better:number;same:number;worse:number;total:number}};
 scenes:(ReferenceCounts&{scene:AppWritingScene;count:number})[];
 records:{taskId:string;kind:'normal'|'preview'|'comparison';status:string;scene:AppWritingScene;profileId:string|null;profileVersion:number;personalized:boolean;referenceDispatch?:'notStarted'|'started'|'unknown'|null;createdAt:string;durationMs:number;feedback:PersonalizationVerdict|null}[];
}
type ConfigRead=PersonalizationConfig&{capability?:{supported:boolean;protocol:string;modelQuality:string}};
const copy=<T,>(value:T):T=>structuredClone(value);
const count=(value:string)=>Array.from(value).length;

export function PersonalizationPage({onDictionary,onSettings,onSkip}:{onDictionary:()=>void;onSettings:()=>void;onSkip:()=>void}){
 const desktop=useDesktop(),navigation=useNavigationGuard();
 const [saved,setSaved]=useState<ConfigRead|null>(null),[draft,setDraft]=useState<PersonalizationConfig|null>(null);
 const [loading,setLoading]=useState(desktop.available),[saving,setSaving]=useState(false),[readError,setReadError]=useState(''),[saveError,setSaveError]=useState(''),[notice,setNotice]=useState('');
 const [stats,setStats]=useState<Statistics|null>(null),[statsError,setStatsError]=useState(''),[window,setWindow]=useState<'week'|'all'>('week'),[statsLoading,setStatsLoading]=useState(false);
 const [editing,setEditing]=useState(false),[tab,setTab]=useState<'background'|'style'|'scope'>('background'),[profileId,setProfileId]=useState<string|null>(null),[styleScene,setStyleScene]=useState<AppWritingScene|'global'>('global');
 const [deleteProfile,setDeleteProfile]=useState<string|null>(null),[conflictVersion,setConflictVersion]=useState<number|null>(null),[latestConflict,setLatestConflict]=useState<ConfigRead|null>(null);
 const [comparison,setComparison]=useState<PersonalizationComparison|null>(null),[comparing,setComparing]=useState(false),[cancelPending,setCancelPending]=useState(false),[compareError,setCompareError]=useState(''),[feedback,setFeedback]=useState<PersonalizationVerdict|null>(null),[feedbackBusy,setFeedbackBusy]=useState(false);
 const [sourceKind,setSourceKind]=useState<'sample'|'paste'|'history'>('sample'),[text,setText]=useState('今天同步三件事。登录页已经完成。支付页还在联调，预计周五完成。发布前先补一轮回归测试，通过以后再上线。'),[historyId,setHistoryId]=useState(''),[modeId,setModeId]=useState(desktop.preferences.defaultModeId);
 const mounted=useRef(false),readRun=useRef(0),statsRun=useRef(0),operation=useRef<string|null>(null),cancelRequested=useRef(false),savingRef=useRef(false),compareRef=useRef(false),draftRef=useRef(draft),savedRef=useRef(saved);
 useEffect(()=>{setComparison(previous=>clearDeletedPersonalizationReferences(previous,desktop.personalizationReferences));},[desktop.personalizationReferences]);
 draftRef.current=draft;savedRef.current=saved;
 const dirty=Boolean(saved&&draft&&JSON.stringify(savedWithoutCapability(saved))!==JSON.stringify(draft));
 const invalid=draft?personalizationError(draft):'';
 const active=draft?.profiles.find(profile=>profile.id===draft.activeProfileId);
 const edited=draft?.profiles.find(profile=>profile.id===profileId);
 const read=useCallback(async()=>{
  if(!desktop.available)return;
  const run=++readRun.current;setLoading(true);setReadError('');
  try{const next=await desktop.command<ConfigRead>('personalization.get');if(!isPersonalizationConfig(next))throw new Error('未收到有效的画像设置，请重新读取。');if(mounted.current&&run===readRun.current){setSaved(next);setDraft(savedWithoutCapability(next));setConflictVersion(null);}}
  catch(error){if(mounted.current&&run===readRun.current)setReadError(errorMessage(error));}
  finally{if(mounted.current&&run===readRun.current)setLoading(false);}
 },[desktop.available,desktop.command]);
 const readStats=useCallback(async()=>{
  if(!desktop.available)return;const run=++statsRun.current;setStatsLoading(true);setStats(null);setStatsError('');
  try{const next=await desktop.command<Statistics>('personalization.stats',{window});if(mounted.current&&run===statsRun.current)setStats(next);}
  catch(error){if(mounted.current&&run===statsRun.current){const message=errorMessage(error);setStatsError(message==='personalization_statistics_incomplete'?'部分个性化统计未能保存，本次运行暂不展示汇总。统计失败不会删除识别稿或成稿，可以继续正常使用。':message);}}
  finally{if(mounted.current&&run===statsRun.current)setStatsLoading(false);}
 },[desktop.available,desktop.command,window]);
 useEffect(()=>{mounted.current=true;void read();return()=>{mounted.current=false;readRun.current++;statsRun.current++;const id=operation.current;if(id)void desktop.command('personalization.cancel',{operationId:id}).catch(()=>{});};},[read,desktop.command]);
 useEffect(()=>{void readStats();},[readStats]);
 const save=useCallback(async()=>{
  const current=draftRef.current,before=savedRef.current;if(!current||!before||savingRef.current)return;
  if(compareRef.current)throw new Error('请先等待对照结束或取消，再保存画像。');
  const error=personalizationError(current);if(error){setSaveError(error);throw new Error(error);}
  savingRef.current=true;setSaving(true);setSaveError('');setNotice('');
  try{
   const next=await desktop.command<ConfigRead>('personalization.save',{config:current,expectedVersion:current.version});
   if(!isPersonalizationConfig(next))throw new Error('保存结果未能确认。草稿已保留，请重新读取后核对。');
   if(!mounted.current)return;setSaved({...next,capability:next.capability??savedRef.current?.capability});setDraft(savedWithoutCapability(next));setConflictVersion(null);setComparison(null);setFeedback(null);setNotice(!next.enabled?'已停用画像，原有语言、词典与场合设置仍生效。':'已保存，下一次整理使用新设置；进行中的普通任务保持原版本。');void readStats();
  }catch(error){
   if(mounted.current){setSaveError(personalizationConflict(error)?'画像已在别处更新。草稿已保留；请读取最新版本后核对，再保存。':errorMessage(error));
    if(personalizationConflict(error)){try{const next=await desktop.command<ConfigRead>('personalization.get');if(isPersonalizationConfig(next)&&mounted.current){setConflictVersion(next.version);setLatestConflict(next);}}catch{/* Draft stays intact. */}}}
   throw error;
  }finally{savingRef.current=false;if(mounted.current)setSaving(false);}
 },[desktop.command,readStats]);
 useEffect(()=>{navigation.register({dirty:dirty||comparing||saving,pending:comparing||saving,save,discard:()=>{if(savedRef.current)setDraft(savedWithoutCapability(savedRef.current));}});return()=>navigation.register(null);},[navigation.register,dirty,comparing,saving,save]);
 useEffect(()=>{if(!dirty)return;const guard=(event:BeforeUnloadEvent)=>{event.preventDefault();};globalThis.addEventListener('beforeunload',guard);return()=>globalThis.removeEventListener('beforeunload',guard);},[dirty]);
 function update(mutator:(config:PersonalizationConfig)=>void){setDraft(before=>{if(!before)return before;const next=copy(before);mutator(next);return next;});setNotice('');setSaveError('');}
 async function disableDuringComparison(){
  const current=savedRef.current;if(!current||savingRef.current)return;
  savingRef.current=true;setSaving(true);setSaveError('');cancelRequested.current=true;
  try{
   const config={...savedWithoutCapability(current),enabled:false};
   const next=await desktop.command<ConfigRead>('personalization.save',{config,expectedVersion:current.version});
   if(!isPersonalizationConfig(next))throw new Error('停用结果未能确认，请重新读取后核对。');
   if(mounted.current){setSaved({...next,capability:next.capability??current.capability});setDraft(savedWithoutCapability(next));setComparison(null);setFeedback(null);setNotice('已停用画像，未完成的个性化任务正在取消。');void readStats();}
  }catch(error){cancelRequested.current=false;if(mounted.current)setSaveError(errorMessage(error));}
  finally{savingRef.current=false;if(mounted.current)setSaving(false);}
 }
 function updateProfile(patch:Partial<PersonalizationProfile>){if(!profileId)return;update(config=>{const index=config.profiles.findIndex(profile=>profile.id===profileId);if(index>=0)config.profiles[index]={...config.profiles[index],...patch};});}
 function addProfile(templateIndex:number){if(!draft||draft.profiles.length>=3)return;const template=templates[templateIndex],id=crypto.randomUUID();update(config=>{config.profiles.push({id,name:template.name==='自定义'?'我的身份':template.name,profession:template.profession,domain:template.domain,description:'',tasks:[...template.tasks],audience:template.audience,scenes:[...template.scenes],instructions:''});config.activeProfileId=id;config.enabled=true;});const preferred=template.scenes.find(scene=>desktop.modes.some(mode=>mode.id===scene));if(preferred)setModeId(preferred);setProfileId(id);setEditing(true);setTab('background');}
 function setStyle(next:PersonalizationStyle){update(config=>{if(styleScene==='global')config.preferences=next;else{const index=config.sceneOverrides.findIndex(item=>item.scene===styleScene);if(index<0)config.sceneOverrides.push({scene:styleScene,preferences:next});else config.sceneOverrides[index].preferences=next;}});}
 async function compare(){
  if(!saved||!draft||dirty||!draft.enabled||compareRef.current||savingRef.current)return;
  compareRef.current=true;cancelRequested.current=false;const id=crypto.randomUUID();operation.current=id;setComparing(true);setCompareError('');setComparison(null);setFeedback(null);setNotice('');
  try{
   let source:Record<string,unknown>={kind:'text',text};
   if(sourceKind==='history'){const record=await desktop.loadRecord(historyId);if(!record.detail?.source.complete)throw new Error('这条记录的识别原文不完整，请选择其他记录。');source={kind:'history',sessionId:historyId,sourceRevisionId:record.detail.source.id};}
   if(operation.current!==id||cancelRequested.current){if(mounted.current)setNotice('已取消对照。');return;}
   const result=await desktop.command<PersonalizationComparison>('personalization.compare',{operationId:id,source,modeId});
   if(mounted.current&&operation.current===id){if(cancelRequested.current){setNotice('对照已取消，迟到结果不会展示。');return;}setComparison(result);if(!result.valid)setCompareError('这次未形成有效对照。请查看两组状态；失败、取消和原文回退不计为收益。');}
  }catch(error){if(mounted.current&&operation.current===id){if(cancelRequested.current)setNotice('对照已取消，源文仍保留。');else setCompareError(errorMessage(error));}}
  finally{if(operation.current===id){operation.current=null;compareRef.current=false;if(mounted.current){setComparing(false);setCancelPending(false);void readStats();}}}
 }
 async function cancel(){const id=operation.current;if(!id||cancelPending)return;cancelRequested.current=true;setCancelPending(true);try{await desktop.command('personalization.cancel',{operationId:id});if(mounted.current)setNotice('已请求取消，正在等待两组任务结束。');}catch(error){if(mounted.current){setCompareError(errorMessage(error));cancelRequested.current=false;setCancelPending(false);}}}
 async function rate(verdict:PersonalizationVerdict){if(!comparison?.valid||feedbackBusy)return;setFeedbackBusy(true);try{await desktop.command('personalization.feedback',{comparisonId:comparison.comparisonId,verdict});if(mounted.current){setFeedback(verdict);setNotice('已保存你的评价；评价不会自动修改画像。');void readStats();}}catch(error){if(mounted.current)setCompareError(errorMessage(error));}finally{if(mounted.current)setFeedbackBusy(false);}}
 const style=styleScene==='global'?draft?.preferences:draft?.sceneOverrides.find(item=>item.scene===styleScene)?.preferences;
 const contentReady=Boolean(saved&&draft&&!loading),blocked=!desktop.available||!contentReady||saving||comparing;
 const selectedHistory=desktop.records.find(record=>record.id===historyId);
 const trialScene:AppWritingScene=scenes.includes(modeId as AppWritingScene)?modeId as AppWritingScene:'general';
 const sceneApplicable=saved?hasPersonalizationForScene(saved,trialScene):false;
 const compareAllowed=Boolean(sceneApplicable&&saved?.enabled&&!dirty&&desktop.provider.configured&&saved?.capability?.supported!==false&&(sourceKind==='history'?historyId:text.trim())&&!blocked);
 return <div className="personalization-page">
  <header className="personalization-heading"><PageHeading title="个性化" description="了解你的工作与习惯，让成稿更符合你。"/><label className="personalization-master">使用我的画像<Switch checked={draft?.enabled??false} disabled={!desktop.available||!contentReady||saving} onCheckedChange={enabled=>{if(!enabled&&comparing)void disableDuringComparison();else update(config=>{config.enabled=enabled;});}}/></label></header>
  <p className="personalization-notice" role="status" aria-live="polite">{notice||(!desktop.available?'请在桌面应用中建立画像。你可以随时跳过，正常录音和整理不受影响。':loading?'正在读取画像…':'')}</p>
  {readError&&<div className="personalization-error" role="alert"><p>{readError}</p><Button variant="soft" onClick={()=>navigation.request(()=>void read())}>重新读取</Button></div>}
  {saved?.capability?.supported===false&&<p role="status" className="personalization-warning">当前整理配置尚不支持画像。你仍可保存设置，使用基础整理；请在服务设置中选择支持的配置。</p>}
  {!contentReady?<section className="personalization-empty"><PersonIcon/><h2>{loading?'正在连接你的设置':'你的写作习惯，由你来定义'}</h2><p>添加工作背景，或直接选择表达习惯。所有信息都可以修改、停用或删除。</p></section>:draft&&<>
   {!draft.profiles.length&&!editing?<section className="personalization-intro surface"><div><span className="personalization-kicker">从你熟悉的工作开始</span><h2>你负责说清楚想法，<br/>让整理更懂你的表达。</h2><p>职业帮助理解领域词；表达习惯决定句子、语气和分段。先选一个起点，所有建议都由你决定。</p></div><div className="personalization-templates">{templates.map((template,index)=><button disabled={blocked} key={template.name} onClick={()=>addProfile(index)}>{template.name}<ArrowRightIcon/></button>)}<Button variant="ghost" disabled={blocked} onClick={()=>{setEditing(true);setTab('style');}}>先只设置表达习惯</Button><Button variant="ghost" color="gray" onClick={onSkip}>稍后设置</Button></div></section>:<section className="personalization-current surface"><span className="personalization-avatar"><PersonIcon/></span><div><span className="personalization-caption">当前身份</span><Select.Root value={draft.activeProfileId??'none'} onValueChange={id=>update(config=>{config.activeProfileId=id==='none'?null:id;})} disabled={blocked}><Select.Trigger aria-label="当前身份"/><Select.Content><Select.Item value="none">仅使用表达习惯</Select.Item>{draft.profiles.map(profile=><Select.Item value={profile.id} key={profile.id}>{profile.name}</Select.Item>)}</Select.Content></Select.Root><p>{active?`职业背景用于：${active.scenes.map(scene=>sceneNames[scene]).join('、')||'尚未选择场合'}。`:'无需填写职业，通用表达习惯也可以单独生效。'}{!draft.enabled?'画像已停用。':''}</p></div><Button variant="soft" color="gray" disabled={blocked} onClick={()=>{setProfileId(draft.activeProfileId??draft.profiles[0]?.id??null);setEditing(!editing);}}>{editing?'收起编辑':'编辑画像'}</Button></section>}
   {editing&&<section className="personalization-editor surface" aria-label="编辑个性化设置"><div className="personalization-tabs" role="tablist" aria-label="画像设置">{([['background','工作背景'],['style','表达习惯'],['scope','作用范围']] as const).map(([value,label])=><button key={value} id={`personalization-tab-${value}`} role="tab" aria-controls={`personalization-panel-${value}`} aria-selected={tab===value} tabIndex={tab===value?0:-1} onKeyDown={event=>{if(event.key==='ArrowLeft'||event.key==='ArrowRight'){event.preventDefault();const keys=['background','style','scope'] as const,index=keys.indexOf(tab),next=keys[(index+(event.key==='ArrowRight'?1:2))%3];setTab(next);document.getElementById(`personalization-tab-${next}`)?.focus();}}} onClick={()=>setTab(value)}>{label}</button>)}</div>
    <fieldset disabled={blocked} className="personalization-fieldset"><div id={`personalization-panel-${tab}`} role="tabpanel" aria-labelledby={`personalization-tab-${tab}`}>
    {tab==='background'&&<><div className="personalization-profile-toolbar"><label>编辑身份<Select.Root value={profileId??'none'} onValueChange={setProfileId}><Select.Trigger aria-label="要编辑的身份"/><Select.Content><Select.Item value="none" disabled>选择身份</Select.Item>{draft.profiles.map(profile=><Select.Item value={profile.id} key={profile.id}>{profile.name}</Select.Item>)}</Select.Content></Select.Root></label><Select.Root value="" onValueChange={value=>addProfile(Number(value))} disabled={draft.profiles.length>=3||blocked}><Select.Trigger placeholder="添加身份" aria-label="添加身份"/><Select.Content>{templates.map((template,index)=><Select.Item key={template.name} value={String(index)}>{template.name}</Select.Item>)}</Select.Content></Select.Root><small>{draft.profiles.length} / 3</small></div>
     {edited?<><div className="personalization-form-grid"><Field label="身份名称" value={edited.name} limit={80} onChange={name=>updateProfile({name})}/><Field label="职业" value={edited.profession} limit={80} placeholder="例如：软件开发、银行客户经理" onChange={profession=>updateProfile({profession})}/><Field label="细分领域" value={edited.domain} limit={120} placeholder="例如：AI／后端、信贷分析" onChange={domain=>updateProfile({domain})}/><Field label="主要沟通对象" value={edited.audience} limit={120} placeholder="例如：同事、客户、研究伙伴" onChange={audience=>updateProfile({audience})}/></div><Field label="工作描述（可选）" value={edited.description} multiline limit={300} placeholder="用一两句话描述你的工作方向，无需填写完整履历。" onChange={description=>updateProfile({description})}/><label className="personalization-field">常见任务<span className="personalization-caption">最多 8 项，用顿号分隔。用于说明常见用途，不代替本次选择。</span><TaskInput key={edited.id} value={edited.tasks} onChange={tasks=>updateProfile({tasks})}/></label><Field label="补充表达要求（可选）" value={edited.instructions} multiline limit={500} placeholder="例如：工作文档里的独立事项可以分点。不要在这里写要自动补入的经历或事实。" onChange={instructions=>updateProfile({instructions})}/><div className="personalization-profile-footer"><p>领域背景帮助理解你说过的内容，具体事实仍以本次口述为准。</p><Button variant="ghost" color="red" onClick={()=>setDeleteProfile(edited.id)}><TrashIcon/>删除身份</Button></div></>:<p className="personalization-caption">可以添加一个身份，也可以直接设置表达习惯。</p>}</>}
    {tab==='style'&&<><div className="personalization-style-toolbar"><label>编辑范围<Select.Root value={styleScene} onValueChange={value=>setStyleScene(value as AppWritingScene|'global')}><Select.Trigger aria-label="表达习惯适用范围"/><Select.Content><Select.Item value="global">通用表达习惯</Select.Item>{scenes.map(scene=><Select.Item value={scene} key={scene}>{sceneNames[scene]}场合覆盖</Select.Item>)}</Select.Content></Select.Root></label><p>{styleScene==='global'?'用于所有场合；单独设置的场合覆盖优先。':'场合覆盖保存一套完整习惯；未开启的选项不继承通用设置。'}</p></div>
     {!style?<div className="personalization-empty-inline"><p>此场合沿用通用表达习惯。</p><Button variant="soft" onClick={()=>setStyle({...draft.preferences})}>为{sceneNames[styleScene as AppWritingScene]}单独设置</Button></div>:<><TemplateSuggestion profile={active} onAccept={suggestion=>setStyle({...style,...suggestion})}/><StyleEditor value={style} onChange={setStyle}/>{styleScene!=='global'&&<Button variant="ghost" color="gray" onClick={()=>update(config=>{config.sceneOverrides=config.sceneOverrides.filter(item=>item.scene!==styleScene);})}>移除覆盖，恢复通用习惯</Button>}</>}
     <p className="personalization-caption">所有示例均为示意写法，未调用模型。场合原有规则和本次明确要求优先；紧凑不会省略实质信息，完整不会补入背景。</p></>}
    {tab==='scope'&&<><h3>身份背景在哪些场合使用？</h3><p className="personalization-caption">通用表达习惯独立生效。职业背景只用于当前身份选中的场合；普通聊天默认不勾选。</p>{draft.profiles.length>0?<><Select.Root value={profileId??draft.profiles[0]?.id} onValueChange={setProfileId}><Select.Trigger aria-label="设置作用范围的身份"/><Select.Content>{draft.profiles.map(profile=><Select.Item value={profile.id} key={profile.id}>{profile.name}</Select.Item>)}</Select.Content></Select.Root>{(edited??draft.profiles[0])&&<div className="personalization-scenes">{scenes.map(scene=>{const profile=edited??draft.profiles[0];return <label key={scene}><Checkbox checked={profile.scenes.includes(scene)} onCheckedChange={checked=>update(config=>{const target=config.profiles.find(item=>item.id===profile.id);if(target)target.scenes=checked?[...target.scenes,scene]:target.scenes.filter(value=>value!==scene);})}/>{sceneNames[scene]}</label>;})}</div>}</>:<p>尚未添加身份；当前只使用表达习惯。</p>}<div className="personalization-priority"><strong>遇到不同要求时</strong><p>保留事实与原意 → 本次明确要求 → 已有场合规则 → 此场合习惯 → 通用习惯。</p><p>自由补充要求可能与已有规则冲突，试写可以帮助你检查；当前不会自动识别所有文字冲突。</p></div></>}
    </div></fieldset>
   </section>}
   {(dirty||saveError)&&<div className="personalization-savebar"><div><strong>{saving?'正在保存…':'有尚未保存的修改'}</strong><span>保存后用于下一次整理。停用或删除会取消引用画像的未完成任务。</span>{(saveError||invalid)&&<p role="alert">{saveError||invalid}</p>}{latestConflict&&conflictVersion!==null&&<details><summary>查看最新设置，再决定是否覆盖</summary><p>最新 v{latestConflict.version} · {latestConflict.enabled?'已启用':'已停用'} · 当前身份：{latestConflict.profiles.find(profile=>profile.id===latestConflict.activeProfileId)?.name??'仅表达习惯'}</p><ul>{latestConflict.profiles.map(profile=><li key={profile.id}><strong>{profile.name}</strong><p>职业：{profile.profession||'未填写'} · 领域：{profile.domain||'未填写'} · 对象：{profile.audience||'未填写'}</p><p>描述：{profile.description||'未填写'}；任务：{profile.tasks.join('、')||'未填写'}；补充：{profile.instructions||'无'}；场合：{profile.scenes.map(scene=>sceneNames[scene]).join('、')||'未选择'}</p></li>)}</ul><p>最新通用习惯：{({default:'按场合决定',concise:'更紧凑',complete:'完整句子'} as const)[latestConflict.preferences.sentenceStyle]}／{latestConflict.preferences.tone==='formal'?'正式':'自然'}；单独覆盖 {latestConflict.sceneOverrides.length} 个场合。以下操作将以当前完整草稿覆盖最新设置。</p><StyleSummary value={latestConflict.preferences}/>{latestConflict.sceneOverrides.map(item=><div key={item.scene}><strong>{sceneNames[item.scene]}场合覆盖</strong><StyleSummary value={item.preferences}/></div>)}</details>}{conflictVersion!==null&&<Button variant="ghost" color="gray" onClick={()=>{update(config=>{config.version=conflictVersion;});setConflictVersion(null);setSaveError('草稿仍保留，已更新保存版本。请核对所有字段后再次保存。');}}>确认用当前草稿覆盖 v{conflictVersion}</Button>}</div><Button variant="soft" color="gray" disabled={saving||comparing} onClick={()=>{setDraft(savedWithoutCapability(saved!));setSaveError('');setConflictVersion(null);}}>放弃修改</Button><Button disabled={!dirty||Boolean(invalid)||comparing} loading={saving} onClick={()=>void save().catch(()=>{})}><CheckIcon/>保存修改</Button></div>}
   <section className="personalization-linked surface"><div><ReaderIcon/><h2>常用词汇与语言</h2></div><p>沿用你已经保存的词典和语言设置。日期跟随正文语言：中文“2026年10月23日”，英文完整日期“2026-10-23”。</p><div className="personalization-actions"><Button variant="soft" color="gray" onClick={onDictionary}>打开个人词典<ArrowRightIcon/></Button><Button variant="ghost" color="gray" onClick={onSettings}>成稿语言：{desktop.preferences.appWriting.outputLanguage==='english'?'英文':'跟随口述'}<ArrowRightIcon/></Button></div></section>
   <section className="personalization-trial surface" aria-labelledby="personalization-trial-title"><header className="section-heading"><div><span className="personalization-kicker">用自己的话验证</span><h2 id="personalization-trial-title">看看画像能帮到哪里</h2></div><MixerHorizontalIcon/></header><p className="personalization-caption">固定同一段原文、场合、语言和当前服务，基础组关闭画像，另一组使用保存的画像。试写不读取其他应用中的输入内容，结果只留在这里。</p><fieldset disabled={comparing||saving||!desktop.available} className="personalization-fieldset"><div className="personalization-trial-controls"><Select.Root value={sourceKind} onValueChange={value=>{setSourceKind(value as typeof sourceKind);setComparison(null);}}><Select.Trigger aria-label="试写内容来源"/><Select.Content><Select.Item value="sample">示例口述</Select.Item><Select.Item value="paste">粘贴文字</Select.Item><Select.Item value="history">从历史选择</Select.Item></Select.Content></Select.Root><Select.Root value={modeId} onValueChange={value=>{setModeId(value);setComparison(null);}}><Select.Trigger aria-label="试写场合"/><Select.Content>{desktop.modes.map(mode=><Select.Item key={mode.id} value={mode.id}>{mode.name}</Select.Item>)}</Select.Content></Select.Root></div>{sourceKind==='history'?<><Select.Root value={historyId||'none'} onValueChange={value=>{setHistoryId(value);setComparison(null);}}><Select.Trigger placeholder="选择一条历史记录" aria-label="用于对照的历史记录"/><Select.Content><Select.Item value="none" disabled>选择历史记录</Select.Item>{desktop.records.map(record=><Select.Item key={record.id} value={record.id}>{record.title.slice(0,50)}</Select.Item>)}</Select.Content></Select.Root>{selectedHistory&&<p className="personalization-caption">使用这条记录完整的识别原文，不使用修改后的成稿。此处显示已加载的最近记录。</p>}</>:<TextArea value={text} onChange={event=>{setText(event.target.value);setComparison(null);}} aria-label="试写口述原文" rows={4} placeholder="粘贴一段你真正想表达的话…"/>}</fieldset>
    {saved?.enabled&&!sceneApplicable&&<p className="personalization-warning">{sceneNames[trialScene]}场合目前没有可参考的画像内容。可选择身份已启用的场合，或保存一条通用表达习惯。</p>}<div className="personalization-trial-run"><p>将使用当前文字服务运行两组整理，每组可能包含复核或修复调用。{saved?.capability?.modelQuality==='notEvaluated'?'当前模型已接入协议，画像效果尚未实测。':''}</p>{comparing?<Button color="gray" variant="soft" disabled={cancelPending} onClick={()=>void cancel()}>{cancelPending?'正在取消…':'取消对照'}</Button>:<Button disabled={!compareAllowed} onClick={()=>void compare()}>对比整理<ArrowRightIcon/></Button>}</div>
    {(!saved?.enabled||dirty)&&<p className="personalization-caption">{dirty?'请先保存画像修改，再运行对照。':'启用并保存“使用我的画像”，即可试写；无需填写职业。'}</p>}
    {compareError&&<p className="personalization-error" role="alert">{compareError}</p>}
    {comparing&&<p role="status" className="personalization-running"><ReloadIcon/>正在整理两份真实结果，原文已固定…</p>}
    {comparison&&<Comparison result={clearDeletedPersonalizationReferences(comparison,desktop.personalizationReferences)} feedback={feedback} busy={feedbackBusy} onFeedback={verdict=>void rate(verdict)}/>}
    {!comparison&&!comparing&&<div className="personalization-compare-empty"><span>基础整理</span><span>应用我的画像</span><p>运行后查看两份真实结果和差异。相同的结果也会如实显示。</p></div>}
   </section>
   <section className="personalization-statistics surface" aria-labelledby="personalization-stats-title"><header className="section-heading"><h2 id="personalization-stats-title">使用记录</h2><Select.Root value={window} onValueChange={value=>setWindow(value as 'week'|'all')}><Select.Trigger aria-label="统计时间范围"/><Select.Content><Select.Item value="week">最近 7 天</Select.Item><Select.Item value="all">全部时间</Select.Item></Select.Content></Select.Root></header>{statsError?<div className="personalization-error" role="alert"><p>{statsError}</p><Button variant="soft" onClick={()=>void readStats()}>重新读取统计</Button></div>:stats?<StatisticsView stats={stats}/>:<p role="status" className="personalization-caption">{statsLoading?'正在读取统计…':'还没有使用记录。'}</p>}</section>
  </>}
  <AlertDialog.Root open={deleteProfile!==null} onOpenChange={open=>{if(!open)setDeleteProfile(null);}}><AlertDialog.Content maxWidth="460px"><AlertDialog.Title>删除这个身份？</AlertDialog.Title><AlertDialog.Description>删除后点击“保存修改”才会生效，并取消引用此身份的未完成任务。个人词典与历史正文保留，已保存的参考详情将按删除策略清除。</AlertDialog.Description><div className="personalization-actions"><AlertDialog.Cancel><Button variant="soft" color="gray">保留身份</Button></AlertDialog.Cancel><Button color="red" onClick={()=>{update(config=>{config.profiles=config.profiles.filter(profile=>profile.id!==deleteProfile);if(config.activeProfileId===deleteProfile)config.activeProfileId=null;});setProfileId(null);setDeleteProfile(null);}}>从草稿删除</Button></div></AlertDialog.Content></AlertDialog.Root>
 </div>;
}

function savedWithoutCapability(config:ConfigRead):PersonalizationConfig {const {version,enabled,activeProfileId,profiles,preferences,sceneOverrides}=config;return copy({version,enabled,activeProfileId,profiles,preferences,sceneOverrides});}
function Field({label,value,limit,onChange,placeholder,multiline=false}:{label:string;value:string;limit:number;onChange:(value:string)=>void;placeholder?:string;multiline?:boolean}){
 const tooLong=count(value)>limit;return <label className="personalization-field">{label}{multiline?<TextArea value={value} rows={3} aria-invalid={tooLong} onChange={event=>onChange(event.target.value)} placeholder={placeholder}/>:<TextField.Root value={value} aria-invalid={tooLong} onChange={event=>onChange(event.target.value)} placeholder={placeholder}/>}<small data-invalid={tooLong}>{count(value)} / {limit} 字{tooLong?' · 请缩短，草稿不会被截断':''}</small></label>;
}
function TaskInput({value,onChange}:{value:string[];onChange:(value:string[])=>void}){
 const [text,setText]=useState(value.join('、')),focused=useRef(false);
 useEffect(()=>{if(!focused.current)setText(value.join('、'));},[value]);
 return <TextField.Root aria-label="常见任务" value={text} onFocus={()=>{focused.current=true;}} onBlur={()=>{focused.current=false;setText(value.join('、'));}} onChange={event=>{setText(event.target.value);onChange(event.target.value.split(/[、,，]/u).map(item=>item.trim()).filter(Boolean));}} placeholder="技术讨论、项目进展、客户沟通"/>;
}
function TemplateSuggestion({profile,onAccept}:{profile?:PersonalizationProfile;onAccept:(value:Partial<PersonalizationStyle>)=>void}){
 const template=templates.find(item=>item.profession&&item.profession===profile?.profession);
 if(!template)return null;
 const labels=Object.entries(template.suggestion).map(([key,value])=>key==='sentenceStyle'?value==='concise'?'句子更紧凑':'完整句子':key==='tone'?'正式语气':styleOptions.find(option=>option.key===key)?.label).filter(Boolean).join('、');
 return <div className="personalization-template-suggestion"><div><strong>{template.name}的可选建议</strong><p>{labels}。不会自动启用，也不会添加职业词汇。</p></div><Button variant="soft" color="gray" onClick={()=>onAccept(template.suggestion)}>将建议加入当前草稿</Button></div>;
}
function StyleSummary({value}:{value:PersonalizationStyle}){
 return <p>句子：{({default:'按场合决定',concise:'更紧凑',complete:'完整句子'} as const)[value.sentenceStyle]}；语气：{value.tone==='formal'?'正式':'自然'}；{styleOptions.map(option=>`${option.label}：${value[option.key]?'开':'关'}`).join('；')}。</p>;
}
function StyleEditor({value,onChange}:{value:PersonalizationStyle;onChange:(value:PersonalizationStyle)=>void}){
 return <div className="personalization-style-options"><fieldset><legend>句子写法</legend><div className="personalization-style-choices">{([['default','按场合决定','沿用当前场合的句子写法。'],['concise','更紧凑','支付页仍在联调，预计周五完成。'],['complete','完整句子','支付页目前仍在联调中，预计周五完成。']] as const).map(([id,label,example])=><label data-selected={value.sentenceStyle===id} key={id}><input type="radio" name="personalization-sentence" value={id} checked={value.sentenceStyle===id} onChange={()=>onChange({...value,sentenceStyle:id})}/><strong>{label}</strong><span>{example}</span></label>)}</div></fieldset><div className="personalization-tone"><label>语气<Select.Root value={value.tone} onValueChange={tone=>onChange({...value,tone:tone as PersonalizationStyle['tone']})}><Select.Trigger aria-label="表达语气"/><Select.Content><Select.Item value="natural">自然</Select.Item><Select.Item value="formal">正式</Select.Item></Select.Content></Select.Root></label><p>{value.tone==='natural'?'“材料齐了以后，可能还要三个工作日，具体看审批。”':'“材料齐全后，可能还需要三个工作日，具体以审批情况为准。”'}</p></div>{styleOptions.map(option=><div className="personalization-style-row" key={option.key}><label><Checkbox checked={value[option.key]} onCheckedChange={checked=>onChange({...value,[option.key]:checked===true})}/><span><strong>{option.label}</strong><small>{option.help}</small></span></label><details><summary>示意写法<ChevronDownIcon/></summary><p>{option.example}</p></details></div>)}</div>;
}
function Comparison({result,feedback,busy,onFeedback}:{result:PersonalizationComparison;feedback:PersonalizationVerdict|null;busy:boolean;onFeedback:(verdict:PersonalizationVerdict)=>void}){
 const diff=useMemo(()=>candidateDiff(result.baseline.text??'',result.personalized.text??''),[result.baseline.text,result.personalized.text]);
 const render=(side:'before'|'after')=>diff.parts.map((part,index)=>part.kind==='same'?<Fragment key={index}>{part.text}</Fragment>:<mark key={index}>{part[side]}</mark>);
 return <div className="personalization-comparison"><p className="personalization-caption">服务：{result.provider} · {result.model} · 场合：{sceneNames[result.scene]}。高亮只表示文本差异，不证明由某条规则造成。</p><div className="personalization-result-grid">{(['baseline','personalized'] as const).map(key=>{const side=result[key];return <article key={key} data-personalized={key==='personalized'}><header><h3>{key==='baseline'?'基础整理':'应用我的画像'}</h3><span>{({success:'已完成',failed:'失败',timeout:'超时',cancelled:'已取消',fallback:'已回退原文'} as Record<string,string>)[side.status]??side.status}</span></header>{side.text?<div className="personalization-result-text">{side.status==='success'&&result.baseline.status==='success'&&result.personalized.status==='success'?render(key==='baseline'?'before':'after'):side.text}</div>:<p className="personalization-caption">这组没有可展示的结果。{side.errorCode?`原因：${side.errorCode}`:''}</p>}</article>;})}</div>{result.identical&&result.valid&&<p className="personalization-identical">这段内容没有明显差异。可以换一段口述，或调整适用的表达习惯。</p>}<PersonalizationReference reference={result.reference}/>{result.valid&&<div className="personalization-rating"><span>哪份更符合你的表达？</span>{([['better','画像更符合我'],['same','差不多'],['worse','画像变差了']] as const).map(([value,label])=><Button key={value} aria-pressed={feedback===value} variant={feedback===value?'solid':'soft'} color={feedback===value?'blue':'gray'} disabled={busy} onClick={()=>onFeedback(value)}>{feedback===value&&<CheckIcon/>}{label}</Button>)}<small>只记录你的评价，不自动改变设置。</small></div>}</div>;
}
function StatisticsView({stats}:{stats:Statistics}){
 const comparisons=stats.comparisons;
 return <>
  <div className="personalization-metrics">
   <div><span>已发起含画像请求的普通整理</span><strong>{stats.normal.personalized}<small> / {stats.normal.total} 次</small></strong></div>
   <div><span>有效对照</span><strong>{comparisons.valid}<small> / {comparisons.total} 次</small></strong></div>
   <div><span>已评价的有效对照</span><strong>{comparisons.feedback.total}<small> 次</small></strong></div>
  </div>
  <p className="personalization-caption">分母为已记录的全部普通整理任务，按任务去重，复核和重试不重复计数。已发起表示客户端开始发送含画像内容的请求，不代表服务端收到、模型采用或质量提升。</p>
  <p className="personalization-caption">普通整理：已选择画像 {stats.normal.selected??0} 次，其中旧记录发送状态无法核验 {stats.normal.legacyUnknown??0} 次，中断任务发送状态无法核验 {stats.normal.interruptedUnknown??0} 次，进行中发送状态待确认 {stats.normal.pending??0} 次。</p>
  <p className="personalization-caption">预览单独统计：已发起含画像请求 {stats.preview.personalized} / {stats.preview.total} 次；已选择画像 {stats.preview.selected??0} 次，旧记录发送状态无法核验 {stats.preview.legacyUnknown??0} 次，中断任务发送状态无法核验 {stats.preview.interruptedUnknown??0} 次，进行中发送状态待确认 {stats.preview.pending??0} 次。</p>
  {comparisons.feedback.total>0?<div className="personalization-feedback-counts"><span>更符合 {comparisons.feedback.better}</span><span>持平 {comparisons.feedback.same}</span><span>变差 {comparisons.feedback.worse}</span><small>分母：{comparisons.feedback.total} 次明确评价</small></div>:<p className="personalization-stats-empty">还没有对照评价。试写一段话，再告诉我们哪份更符合你。</p>}
  <p className="personalization-caption">对照状态：失败 {comparisons.failed} · 回退 {comparisons.fallback} · 取消 {comparisons.cancelled} · 进行中 {comparisons.pending}。收益只统计有效对照中的明确评价。</p>
  {stats.scenes.length>0&&<details className="personalization-reference"><summary>按场合查看普通整理数量</summary><ul>{stats.scenes.map(item=><li key={item.scene}>{sceneNames[item.scene]}<span>{item.personalized} / {item.count} 次已发起含画像请求</span><small>已选择画像 {item.selected??0} 次 · 旧记录发送状态无法核验 {item.legacyUnknown??0} 次 · 中断任务发送状态无法核验 {item.interruptedUnknown??0} 次 · 进行中发送状态待确认 {item.pending??0} 次</small></li>)}</ul></details>}
  {stats.records.length>0&&<details className="personalization-reference"><summary>近期任务记录</summary><div className="personalization-records">{stats.records.map((record,index)=><div key={`${record.taskId}-${index}`}><span>{({normal:'普通整理',preview:'预览',comparison:'对照'} as const)[record.kind]} · {sceneNames[record.scene]} · {({started:'进行中',success:'完成',fallback:'原文回退',failed:'失败',cancelled:'取消'} as Record<string,string>)[record.status]??record.status}</span><small>{record.createdAt} · 画像 v{record.profileVersion} · {referenceDispatchLabel(record)}</small></div>)}</div></details>}
 </>;
}
function referenceDispatchLabel(record:Statistics['records'][number]){
 if(!record.personalized)return '未选择画像';
 if(record.referenceDispatch==='started')return '已发起含画像请求';
 if(record.referenceDispatch==='notStarted')return record.status==='started'?'已选画像，发送状态待确认':'已选画像，未发起含画像请求';
 if(record.referenceDispatch==='unknown')return '已选画像，中断后发送状态无法核验';
 return '已选画像，发送状态无法核验（旧记录）';
}
