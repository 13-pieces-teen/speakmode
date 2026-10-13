import type { AppWritingPreferences, AppWritingScene } from '../appWriting';
export type BaseMode = 'auto' | 'light' | 'structure';
export type RewriteFallbackReason = 'timeout' | 'serviceUnavailable' | 'invalidOutput' | 'meaningRisk';
export interface NativePreferences { version:number; shortcut:string; microphoneId:string|null; language:'auto'|'zh'|'en'; defaultModeId:string; deliveryMode:'direct'|'preview'; softVoiceEnabled:boolean; appWriting:AppWritingPreferences; keepHistory:boolean; launchAtLogin:boolean }
export interface ModeRecord { id:string; name:string; baseMode:BaseMode; rules:string; revision:number; builtin:boolean }
export interface ModeSnapshot { modeId:string; name:string; baseMode:BaseMode; rules:string; revision:number }
export interface SourceItem { id:string; providerItemId:string; captureOrder:number; text:string; final:boolean; startMs:number|null; endMs:number|null }
export interface SourceRevision { id:string; sessionId:string; asrAttemptId:string; generation:number; provider:string; model:string; adapterVersion:string; complete:boolean; items:SourceItem[]; createdAt:string }
export interface OutputBlock { id:string; text:string; sourceRefs:string[] }
export interface OutputRevision { rewriteFallback?:RewriteFallbackReason; id:string; sessionId:string; parentRevisionId:string|null; baseRevisionId:string|null; sourceRevisionId:string; modeSnapshot:ModeSnapshot; resolvedMode:BaseMode; blocks:OutputBlock[]; kind:'initial'|'useSource'|'adopt'|'edit'|'restore'|'undo'; creationReason:'normalInitial'|'retry'|'useSource'|'history'|'preview'; createdAt:string }
export type CandidateReviewIssue='valuesChanged'|'meaningChanged'|'literalsChanged';
/** Ephemeral history draft, never an automatic delivery authorization. */
export interface HistoryCandidate extends OutputRevision { reviewIssues?:CandidateReviewIssue[] }
export interface SessionRecord { id:string; generation:number; startedAt:string; endedAt:string; captureDurationMs:number; endLocalDate:string; timeZone:string; storagePolicy:'history'|'memoryOnly'; modeSnapshot:ModeSnapshot; status:'checkpointed'|'ready'|'interrupted'; currentSourceId:string; currentOutputId:string|null; undoTargetId:string|null }
export interface HistoryDetail { session:SessionRecord; source:SourceRevision; output:OutputRevision|null; canUndo:boolean }
export interface HistorySummary { rewriteFallback?:RewriteFallbackReason; sessionId:string; title:string; preview:string; endedAt:string; endLocalDate:string; modeId:string; modeName:string; resolvedMode:BaseMode|null; currentRevisionId:string|null; sourceRevisionId:string; status:string; sourceCharCount:number; captureDurationMs:number }
export interface HistoryPage { items:HistorySummary[]; total:number; nextCursor:number|null }
export interface UsageTotals { count:number; sourceCharCount:number; captureDurationMs:number }
export interface DailyUsage extends UsageTotals { date:string }
export interface ProviderConfig { version:number; provider:string; region:string|null; workspace:string|null; asrModel:string; rewriteModel:string; adapterVersion:string }
export interface ProviderModelOption { id:string; label:string; implemented:boolean; adapterVersion?:string|null; dictionarySupport?:string; protocol?:string; timestamps?:boolean; recognitionLanguages?:string[]; productAudioLimitSeconds?:number; jsonSchema?:boolean }
export interface ProviderModelCatalog { asr:ProviderModelOption[]; rewrite:ProviderModelOption[] }
export interface ProviderStatus { config:ProviderConfig; configured:boolean; credentialConfigured?:boolean; status?:string; message?:string; modelCatalog?:ProviderModelCatalog }
export interface AudioDevice { id:string; name:string; isDefault?:boolean; available?:boolean }
export interface DictionaryEntry { id:string; term:string; createdAt:string; updatedAt:string }
export interface DictionarySnapshot { version:number; entries:DictionaryEntry[] }
/** Provider caption revisions are display data; only final source items authorize rewriting. */
export interface CaptionItem { id:string; text:string; revision:number; final:boolean; captureOrder:number }
export interface LiveSession { writingScene?:AppWritingScene; rewriteFallback?:RewriteFallbackReason; id?:string; sessionId?:string; generation:number; state:string; modeSnapshot?:ModeSnapshot; modeId?:string; level?:number; levelReceivedAt?:number; partial?:string; transcript?:string; items?:SourceItem[]; captions?:CaptionItem[]; captureReady?:boolean; providerReady?:boolean; captionRenderToken?:string|null; error?:string; message?:string; result?:HistoryDetail; deliveryState?:string;deliveryFailureReason?:string|null;recoverableContentId?:string|null;pendingCommitId?:string|null;commitStage?:string|null;workerReleased?:boolean;asrAttemptId?:string|null;sourceRevisionId?:string|null;canRetryAsr?:boolean;canRetryRewrite?:boolean;canRetrySave?:boolean;retryKind?:'asr'|'rewrite'|'save'|null;errorCode?:string|null;canUseSource?:boolean;canCopyRecovery?:boolean;recordingLimitRemainingMs?:number|null }
export interface RetainedRecovery { session:LiveSession; expiresAt:string }
export interface NativeSnapshot { sequence:number; preferences?:NativePreferences; modes?:ModeRecord[]; dictionary?:DictionarySnapshot; usage?:DailyUsage; provider?:ProviderStatus; devices?:AudioDevice[]; session?:LiveSession|null; recovery?:RetainedRecovery|null; temporaryResult?:HistoryDetail|null; capabilities?:Record<string,boolean> }
export type DesktopErrorScope = 'application' | 'session' | 'transport';
export interface DesktopError { scope:DesktopErrorScope; message:string; sessionId?:string; generation?:number; operation?:string }
export interface RecorderModeRequest { sequence:number; sessionId:string; generation:number }
export interface AppEvent { sequence:number; kind:string; payload:unknown; sessionId?:string; generation?:number; scope?:DesktopErrorScope }
export interface RestorePatch { blockId:string; start:number; end:number; sourceSelections:{itemId:string;start:number;end:number}[] }
export interface CorrectionPatch { blockId:string; start:number; end:number; expectedText:string; replacement:string }
export interface CorrectionReceipt { detail:HistoryDetail; dictionary:DictionarySnapshot|null; dictionaryStatus:'added'|'existing'|'notRequested' }
export const initialPreferences:NativePreferences={version:1,shortcut:'alt_right',microphoneId:null,language:'auto',defaultModeId:'auto',deliveryMode:'direct',softVoiceEnabled:false,appWriting:{enabled:true,overrides:[]},keepHistory:true,launchAtLogin:false};
export const initialProvider:ProviderStatus={configured:false,config:{version:1,provider:'aliyun',region:null,workspace:null,asrModel:'qwen3-asr-flash-realtime',rewriteModel:'qwen3.8-flash',adapterVersion:'qwen-realtime-v1'}};



