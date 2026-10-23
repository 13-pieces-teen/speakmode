// UI presentation only; native permissions and IPC remain authoritative.
export const isMacOS = typeof navigator !== 'undefined' && /Mac/i.test(navigator.platform);
