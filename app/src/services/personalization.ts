import type {AppWritingScene} from '../appWriting';
import type {ModeSnapshot} from './contracts';

export interface PersonalizationStyle {
 sentenceStyle:'default'|'concise'|'complete';tone:'natural'|'formal';
 keepEnglishTerms:boolean;fewerCourtesies:boolean;paragraphs:boolean;lists:boolean;
}
export interface PersonalizationProfile {
 id:string;name:string;profession:string;domain:string;description:string;
 tasks:string[];audience:string;scenes:AppWritingScene[];instructions:string;
}
export interface PersonalizationConfig {
 version:number;enabled:boolean;activeProfileId:string|null;profiles:PersonalizationProfile[];
 preferences:PersonalizationStyle;sceneOverrides:{scene:AppWritingScene;preferences:PersonalizationStyle}[];
}
export interface PersonalizationReference {
 version:number;profileId?:string|null;profileName?:string|null;scene:AppWritingScene;
 background?:{profession:string;domain:string;description:string;tasks:string[];audience:string}|null;
 rules:{key:string;text:string;origin:string}[];omitted:string[];
 referenceCleared?:boolean;
}
export interface PersonalizationSide {status:'success'|'failed'|'timeout'|'cancelled'|'fallback';text?:string|null;errorCode?:string|null}
export interface PersonalizationComparison {
 id:string;comparisonId:string;sourceKind:string;sourceRevisionId:string;modeSnapshot:ModeSnapshot;scene:AppWritingScene;
 baseline:PersonalizationSide;personalized:PersonalizationSide;valid:boolean;identical:boolean;
 reference:PersonalizationReference|null;provider:string;model:string;
}
export type PersonalizationVerdict='better'|'same'|'worse';
export const defaultStyle:PersonalizationStyle={sentenceStyle:'default',tone:'natural',keepEnglishTerms:false,fewerCourtesies:false,paragraphs:false,lists:false};
export const sceneNames:Record<AppWritingScene,string>={general:'通用',chat:'聊天',document:'文档',email:'邮件',technical:'技术'};
export const scenes=Object.keys(sceneNames) as AppWritingScene[];
export const styleOptions=[
 {key:'keepEnglishTerms',label:'保留常见英文术语',help:'口述已有的英文术语优先保留，不自动补入专业词。',example:'我主要做 backend，也负责一点前端。'},
 {key:'fewerCourtesies',label:'减少额外客套',help:'不额外添加寒暄，保留你已经表达的礼貌和态度。',example:'请把报告发我，周五之前需要。'},
 {key:'paragraphs',label:'按话题分段',help:'连续解释按话题分成自然段，不补标题或新内容。',example:'登录页已完成。\n\n支付页仍在联调，预计周五完成。'},
 {key:'lists',label:'明确列举时分点',help:'有明确列举意图时分点，连续叙述仍按原有关系表达。',example:'需要核对三件事：\n• 日期\n• 预算\n• 负责人'},
] as const;
export const templates=[
 {name:'软件开发',profession:'软件开发',domain:'',scenes:['document','technical'],tasks:['技术讨论','项目进展'],audience:'同事',suggestion:{keepEnglishTerms:true,sentenceStyle:'concise'}},
 {name:'金融服务',profession:'金融服务',domain:'',scenes:['email','document'],tasks:['客户沟通','工作汇报'],audience:'客户',suggestion:{tone:'formal',sentenceStyle:'complete'}},
 {name:'产品／运营',profession:'产品／运营',domain:'',scenes:['document','email'],tasks:['产品讨论','工作汇报'],audience:'同事',suggestion:{paragraphs:true}},
 {name:'学生／研究',profession:'学生／研究',domain:'',scenes:['document','technical'],tasks:['学习笔记','研究写作'],audience:'同学／研究伙伴',suggestion:{keepEnglishTerms:true,paragraphs:true}},
 {name:'自定义',profession:'',domain:'',scenes:[],tasks:[],audience:'',suggestion:{}},
] satisfies {name:string;profession:string;domain:string;scenes:AppWritingScene[];tasks:string[];audience:string;suggestion:Partial<PersonalizationStyle>}[];

const unicodeLength=(value:string)=>Array.from(value).length;
export function personalizationError(config:PersonalizationConfig):string {
 if(config.profiles.length>3)return '最多保存 3 个身份。';
 if(config.activeProfileId&&!config.profiles.some(profile=>profile.id===config.activeProfileId))return '请选择当前身份，或选择仅使用表达习惯。';
 for(const profile of config.profiles){
  if(!profile.name.trim())return '请为每个身份填写名称。';
  for(const [key,label,max] of [['name','身份名称',80],['profession','职业',80],['domain','细分领域',120],['description','工作描述',300],['audience','沟通对象',120],['instructions','补充要求',500]] as const){
   if(unicodeLength(profile[key])>max)return `${label}最多 ${max} 字，已保留草稿，请缩短后保存。`;
   if(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u.test(profile[key]))return `${label}含无效字符，请重新输入。`;
   if(/[\p{Cf}\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/u.test(profile[key]))return `${label}含控制或不可见格式字符，请移除后保存。`;
  }
  if(profile.tasks.length>8||profile.tasks.some(task=>!task.trim()||unicodeLength(task)>80))return '常见任务最多 8 项，每项最多 80 字。';
 }
 const seen=new Set<string>();
 for(const item of config.sceneOverrides){if(seen.has(item.scene))return '每个场合只能保存一组表达习惯。';seen.add(item.scene);}
 return '';
}
export function isPersonalizationConfig(value:unknown):value is PersonalizationConfig {
 if(!value||typeof value!=='object')return false;
 const config=value as PersonalizationConfig;
 const style=(item:PersonalizationStyle)=>item&&['default','concise','complete'].includes(item.sentenceStyle)&&['natural','formal'].includes(item.tone)&&styleOptions.every(option=>typeof item[option.key]==='boolean');
 return Number.isInteger(config.version)&&config.version>0&&typeof config.enabled==='boolean'&&(config.activeProfileId===null||typeof config.activeProfileId==='string')&&Array.isArray(config.profiles)&&config.profiles.length<=3&&config.profiles.every(profile=>profile&&['id','name','profession','domain','description','audience','instructions'].every(key=>typeof profile[key as keyof PersonalizationProfile]==='string')&&Array.isArray(profile.tasks)&&profile.tasks.every(task=>typeof task==='string')&&Array.isArray(profile.scenes)&&profile.scenes.every(scene=>scenes.includes(scene)))&&Boolean(style(config.preferences))&&Array.isArray(config.sceneOverrides)&&config.sceneOverrides.every(item=>item&&scenes.includes(item.scene)&&style(item.preferences));
}
export function personalizationConflict(value:unknown):boolean {
 if(typeof value==='string'){try{return personalizationConflict(JSON.parse(value));}catch{return false;}}
 return Boolean(value&&typeof value==='object'&&(value as {code?:string}).code==='revisionConflict');
}
/** UI availability hint only; the native freeze remains authoritative. */
export function hasPersonalizationForScene(config:PersonalizationConfig,scene:AppWritingScene):boolean {
 if(!config.enabled)return false;
 const style=config.sceneOverrides.find(item=>item.scene===scene)?.preferences??config.preferences;
 if(style.sentenceStyle!=='default'||style.tone!=='natural'||styleOptions.some(option=>style[option.key]))return true;
 const profile=config.profiles.find(item=>item.id===config.activeProfileId&&item.scenes.includes(scene));
 return Boolean(profile&&(profile.tasks.length||[profile.profession,profile.domain,profile.description,profile.audience,profile.instructions].some(text=>text.trim())));
}
