import type { BuiltinMode, ResolvedMode } from './model';
export interface PersonalMode {id:string;name:string;base:ResolvedMode;instructions:string;version:number}
export const builtinModes:{id:BuiltinMode;caption:string;detail:string}[]=[
 {id:'自动',caption:'根据表达选择整理方式',detail:'连贯叙述用轻整理，多个主题或步骤用结构；不确定时优先轻整理。'},
 {id:'轻整理',caption:'整理口语，保留叙述顺序',detail:'合并口头重复与改口，让句子更自然，保留你的表达顺序和语气。'},
 {id:'结构',caption:'按主题归组，合并补充说明',detail:'把同一主题的补充、条件放在一起；多个要点清楚分组，保留必要顺序。'},
];
export const instructionTemplates=[{name:'自然表达',base:'轻整理' as const,text:'清理口头重复与改口，保留我的叙述顺序和语气。按自然段输出，不添加标题或总结。'},{name:'清晰分点',base:'结构' as const,text:'按主题分点组织，把同一主题的补充和条件放在一起。保留全部实质信息和必要顺序，不添加额外总结。'},{name:'项目记录',base:'结构' as const,text:'按“已完成、待处理、尚未确定”分组。未提到的栏目不出现，不补写负责人和日期。'}];
export const projectSample='登录界面已经完成。邮件提醒还没开始，部署时间也没确定。对了，登录页还差手机端测试。邮件提醒先别发，要等同学确认，小陈是否负责也还没定。';
export const isBuiltinMode=(mode:string):mode is BuiltinMode=>mode==='自动'||mode==='轻整理'||mode==='结构';
// Called with UI IDs after fromModeId. Never infer native personal identity from a prefix.
export const isPersonalMode=(mode:string):boolean=>!isBuiltinMode(mode);
