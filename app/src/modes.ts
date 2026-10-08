import type { BuiltinMode, ResolvedMode } from './model';
export interface PersonalMode {id:string;name:string;base:ResolvedMode;instructions:string;version:number}
export const builtinModes:{id:BuiltinMode;caption:string}[]=[{id:'自动',caption:'自动选择轻整理或结构'},{id:'轻整理',caption:'清理口语冗余，保留顺序'},{id:'结构',caption:'分点组织，保留全部信息'}];
export const instructionTemplates=[{name:'自然表达',base:'轻整理' as const,text:'清理口头重复，保留我的叙述顺序和语气。按自然段输出，不添加标题或总结。'},{name:'清晰分点',base:'结构' as const,text:'按主题分点组织，每点表达一个完整意思。保留全部信息，不添加额外总结。'},{name:'项目记录',base:'结构' as const,text:'按“已完成、待处理、尚未确定”分组。未提到的栏目不出现，不补写负责人和日期。'}];
export const projectSample='登录界面已经完成，邮件提醒还没开始。部署时间还没确定，小陈是否负责提醒模块也还没确认。';
export const isBuiltinMode=(mode:string):mode is BuiltinMode=>mode==='自动'||mode==='轻整理'||mode==='结构';
// Called with UI IDs after fromModeId. Never infer native personal identity from a prefix.
export const isPersonalMode=(mode:string):boolean=>!isBuiltinMode(mode);

