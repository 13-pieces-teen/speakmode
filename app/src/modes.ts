import type { BuiltinMode, Mode, ResolvedMode } from './model';
export interface PersonalMode {id:string;name:string;base:ResolvedMode;instructions:string;version:number}
export interface BuiltinModeOption {id:BuiltinMode;caption:string;detail:string}
export const builtinModes: BuiltinModeOption[] = [
 {id:'自动',caption:'结合口述内容与输入位置',detail:'结合你的表达、应用与可用的输入位置上下文选择场合；不确定时保留自然叙述。'},
 {id:'聊天消息',caption:'自然直接，便于对话',detail:'保持自然语气和简短段落，保留条件与必要信息，不额外添加客套。'},
 {id:'文档写作',caption:'连贯成稿，按内容分段',detail:'整理为连贯的自然段；只有明确的步骤或并列要点才分点，不强制添加标题。'},
 {id:'邮件沟通',caption:'清楚表达来意与请求',detail:'组织清晰、语气得体；保留原有请求与条件，不补写收件人、署名或承诺。'},
 {id:'技术沟通',caption:'术语准确，步骤清楚',detail:'保留代码、命令、数字与术语的原始含义，按内容组织说明或操作步骤。'},
];
export const legacyModes: BuiltinModeOption[] = [
 {id:'轻整理',caption:'原有表达偏好 · 保留叙述顺序',detail:'清理口语重复与改口，保留原有顺序。兼容已有设置与历史。'},
 {id:'结构',caption:'原有表达偏好 · 按主题分点',detail:'按主题组织内容，保留全部信息。兼容已有设置与历史。'},
];
export const allBuiltinModes = [...builtinModes, ...legacyModes];
/** Shared catalog for main window, settings, history and recorder. */
export function scenarioOptions(current:Mode,modes:PersonalMode[]) {
 const builtins = [...builtinModes,...legacyModes.filter(mode=>mode.id===current)];
 return [...builtins.map(mode=>({id:mode.id as Mode,name:mode.id,caption:mode.caption,revision:1})),
  ...modes.map(mode=>({id:mode.id as Mode,name:mode.name,caption:'自定义场合',revision:mode.version}))];
}
export const instructionTemplates = [
 {name:'自然表达',base:'轻整理' as const,text:'清理口头重复与改口，保留我的叙述顺序和语气。按自然段输出，不添加标题或总结。'},
 {name:'清晰分点',base:'结构' as const,text:'按主题分点组织，把同一主题的补充和条件放在一起。保留全部实质信息和必要顺序，不添加额外总结。'},
 {name:'项目记录',base:'结构' as const,text:'按“已完成、待处理、尚未确定”分组。未提到的栏目不出现，不补写负责人和日期。'},
];
export const layoutInstructionTemplates = [
 {label:'自然段',base:'轻整理' as const,rule:'按口述顺序整理为连贯的自然段，在真实话题变化处换段；同一件事的说明与条件放在一起，不按识别断句逐句分段，不添加标题或总结。'},
 {label:'独立约束分点',base:'结构' as const,rule:'将彼此独立的约束或步骤逐条列出，用编号或项目符号并实际换行；同一主题的补充、条件与否定跟随对应要点，不拆成无条件要求，不把连续叙述强行拆点，不添加总结。'},
];
const managedLayoutRules = new Set([
 ...layoutInstructionTemplates.map(template=>template.rule),
 ...instructionTemplates.slice(0,2).map(template=>template.text),
]);
/** Replace only our exact template lines; free-form requirements remain user-owned. */
export function withLayoutInstruction(instructions:string,base:ResolvedMode):string {
 const rule=layoutInstructionTemplates.find(template=>template.base===base)!.rule;
 const remaining=instructions.split(/\r?\n/).filter(line=>!managedLayoutRules.has(line.trim())).join('\n').trim();
 return remaining?`${rule}\n${remaining}`:rule;
}
export const projectSample='登录界面已经完成。邮件提醒还没开始，部署时间也没确定。对了，登录页还差手机端测试。邮件提醒先别发，要等同学确认，小陈是否负责也还没定。';
export const isBuiltinMode=(mode:string):mode is BuiltinMode=>allBuiltinModes.some(item=>item.id===mode);
// Called with UI IDs after fromModeId. Never infer native personal identity from a prefix.
export const isPersonalMode=(mode:string):boolean=>!isBuiltinMode(mode);
