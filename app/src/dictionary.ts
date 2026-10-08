import type {DictionarySnapshot} from './services/contracts';

export const dictionaryLimits = {entries: 200, termScalars: 80, contextBytes: 4000} as const;

/** SpeakMode's stored dictionary budget, matching DictionarySnapshot.asr_context.
 * This is not a provider limit: individual ASR adapters also validate their payloads.
 */
export function dictionaryContextBytes(terms: readonly string[]): number {
  return terms.length ? new TextEncoder().encode(`常用词汇：${JSON.stringify(terms)}`).byteLength : 0;
}

type CapacityDraft = {kind: 'single'; id?: string; text: string} | {kind: 'bulk'; terms: readonly string[]};

/** Upper bound for valid drafts against the displayed snapshot. Native NFKC +
 * full Unicode case-folding remains authoritative; search normalization is not
 * sufficient to predict duplicates. Never use this estimate to block a save.
 */
export function dictionaryDraftCapacity(entries: readonly Pick<DictionarySnapshot['entries'][number], 'id' | 'term'>[], draft: CapacityDraft) {
  const terms = entries.filter(entry => draft.kind !== 'single' || entry.id !== draft.id).map(entry => entry.term);
  if (draft.kind === 'single') terms.push(draft.text.trim());
  else terms.push(...draft.terms);
  const bytes = dictionaryContextBytes(terms);
  return {bytes, entries: terms.length, remainingBytes: dictionaryLimits.contextBytes - bytes};
}

/** Search normalization never mutates the spelling sent to the recognizer. */
export function dictionarySearchKey(value: string): string {
  return value.normalize('NFKC').toLocaleLowerCase();
}

export function dictionaryTermError(value: string): string {
  const term = value.trim();
  if (!term) return '请输入一个词条。';
  if (/[\u0000-\u001f\u007f-\u009f]/u.test(term)) return '词条不能包含换行或控制字符。';
  // Array.from counts Unicode scalars, except lone surrogates, which IPC cannot encode.
  if (Array.from(term).some(char => char.length === 1 && /[\ud800-\udfff]/u.test(char))) return '词条包含无效字符，请重新输入。';
  if (Array.from(term).length > dictionaryLimits.termScalars) return '每个词条最多 80 个字符。';
  return '';
}

export function parseDictionaryLines(value: string): {terms: string[]; error: string} {
  if (value.length > 33000) return {terms: [], error: '这批文字过长，请分批添加；每行只保留一个词条。'};
  const terms = value.split(/\r?\n/u).map(term => term.trim()).filter(Boolean);
  if (!terms.length) return {terms, error: '每行输入一个词条。'};
  if (terms.length > dictionaryLimits.entries) return {terms, error: '一次最多添加 200 行，请分批添加。'};
  const invalid = terms.findIndex(term => Boolean(dictionaryTermError(term)));
  return {terms, error: invalid < 0 ? '' : `第 ${invalid + 1} 个词条：${dictionaryTermError(terms[invalid])}`};
}

export function isDictionarySnapshot(value: unknown): value is DictionarySnapshot {
  if (!value || typeof value !== 'object') return false;
  const snapshot = value as Partial<DictionarySnapshot>;
  return Number.isSafeInteger(snapshot.version) && (snapshot.version ?? 0) >= 1 && Array.isArray(snapshot.entries)
    && snapshot.entries.every(entry => entry && typeof entry.id === 'string' && typeof entry.term === 'string' && typeof entry.createdAt === 'string' && typeof entry.updatedAt === 'string');
}

export function dictionaryConflict(error: unknown): boolean {
  let value = error;
  if (typeof value === 'string') {try {value = JSON.parse(value);} catch {return false;}}
  return Boolean(value && typeof value === 'object' && 'code' in value && value.code === 'revisionConflict');
}
