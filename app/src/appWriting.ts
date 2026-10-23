import {isMacOS} from './platform';
export type AppWritingScene = 'general' | 'chat' | 'document' | 'email' | 'technical';
export interface AppWritingOverride { application: string; scene: AppWritingScene }
export interface AppWritingPreferences { enabled: boolean; overrides: AppWritingOverride[]; inputContextEnabled:boolean; excludedApplications:string[]; outputLanguage:'asSpoken'|'english' }
export const defaultAppWriting: AppWritingPreferences = { enabled: true, overrides: [], inputContextEnabled:false, excludedApplications:[], outputLanguage:'asSpoken' };
export function normalizeAppWriting(value?:Partial<AppWritingPreferences>):AppWritingPreferences { return {...defaultAppWriting,...value}; }
export const writingSceneLabels: Record<AppWritingScene, string> = {
  general: '通用 · 按内容决定', chat: '聊天 · 自然短句', document: '文档 · 连贯自然段', email: '邮件 · 来意与请求', technical: '技术 · 保留术语与代码',
};
export const writingApplications = isMacOS ? [
  {id:'com.tencent.xinwechat',name:'微信',scene:'chat'},
  {id:'com.electron.lark',name:'飞书',scene:'chat'},
  {id:'com.tinyspeck.slackmacgap',name:'Slack',scene:'chat'},
  {id:'com.microsoft.word',name:'Word',scene:'document'},
  {id:'com.apple.iwork.pages',name:'Pages',scene:'document'},
  {id:'com.apple.mail',name:'邮件',scene:'email'},
  {id:'com.microsoft.vscode',name:'VS Code',scene:'technical'},
  {id:'com.todesktop.230313mzl4w4u92',name:'Cursor',scene:'technical'},
  {id:'com.apple.safari',name:'Safari',scene:'general'},
  {id:'com.google.chrome',name:'Chrome',scene:'general'},
] satisfies { id:string;name:string;scene:AppWritingScene }[] : [
  { id: 'wechat.exe', name: '微信', scene: 'chat' }, { id: 'weixin.exe', name: '微信（Weixin）', scene: 'chat' },
  { id: 'feishu.exe', name: '飞书', scene: 'chat' }, { id: 'slack.exe', name: 'Slack', scene: 'chat' },
  { id: 'ms-teams.exe', name: 'Teams', scene: 'chat' }, { id: 'winword.exe', name: 'Word', scene: 'document' },
  { id: 'code.exe', name: 'VS Code', scene: 'technical' }, { id: 'cursor.exe', name: 'Cursor', scene: 'technical' },
  { id: 'msedge.exe', name: 'Edge', scene: 'general' }, { id: 'chrome.exe', name: 'Chrome', scene: 'general' },
] satisfies { id: string; name: string; scene: AppWritingScene }[];
export function applicationKey(value: string): string | null {
  const key = value.trim().toLowerCase();
  const executable=key.length>4&&/^[a-z0-9_-][a-z0-9._ -]*\.exe$/.test(key)&&!key.includes('..');
  const bundle=/^[a-z0-9-]+(?:\.[a-z0-9-]+)+$/.test(key);
  return key.length<=160&&(executable||bundle)?key:null;
}
export function applicationLabel(key: string): string {
  return writingApplications.find(app => app.id === key.toLowerCase())?.name ?? key;
}
export function appWritingIsDefault(value: AppWritingPreferences): boolean { return value.enabled && value.overrides.length === 0 && !value.inputContextEnabled && value.excludedApplications.length === 0 && value.outputLanguage === 'asSpoken'; }
