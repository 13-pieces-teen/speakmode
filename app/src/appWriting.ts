export type AppWritingScene = 'general' | 'chat' | 'document' | 'technical';
export interface AppWritingOverride { application: string; scene: AppWritingScene }
export interface AppWritingPreferences { enabled: boolean; overrides: AppWritingOverride[] }
export const defaultAppWriting: AppWritingPreferences = { enabled: true, overrides: [] };
export const writingSceneLabels: Record<AppWritingScene, string> = {
  general: '通用 · 按内容决定', chat: '聊天 · 自然短句', document: '文档 · 清晰条目', technical: '技术 · 保留条目与代码',
};
export const writingApplications = [
  { id: 'wechat.exe', name: '微信', scene: 'chat' }, { id: 'weixin.exe', name: '微信（Weixin）', scene: 'chat' },
  { id: 'feishu.exe', name: '飞书', scene: 'chat' }, { id: 'slack.exe', name: 'Slack', scene: 'chat' },
  { id: 'ms-teams.exe', name: 'Teams', scene: 'chat' }, { id: 'winword.exe', name: 'Word', scene: 'document' },
  { id: 'code.exe', name: 'VS Code', scene: 'technical' }, { id: 'cursor.exe', name: 'Cursor', scene: 'technical' },
  { id: 'msedge.exe', name: 'Edge', scene: 'general' }, { id: 'chrome.exe', name: 'Chrome', scene: 'general' },
] satisfies { id: string; name: string; scene: AppWritingScene }[];
export function applicationKey(value: string): string | null {
  const key = value.trim().toLowerCase();
  return key.length > 4 && key.length <= 80 && /^[a-z0-9_-][a-z0-9._ -]*\.exe$/.test(key) && !key.includes('..') ? key : null;
}
export function applicationLabel(key: string): string {
  return writingApplications.find(app => app.id === key.toLowerCase())?.name ?? key;
}
export function appWritingIsDefault(value: AppWritingPreferences): boolean { return value.enabled && value.overrides.length === 0; }
