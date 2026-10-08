// Display a short history of microphone loudness, not a fabricated spectrum.
// Native RMS is linear full scale; ordinary speech is far below 1.0.
export const WAVEFORM_BARS=13;
export const WAVEFORM_FLOOR_DB=-60;
export const WAVEFORM_TOP_DB=-18;
export const WAVEFORM_HISTORY_MS=60;
export const WAVEFORM_STALE_MS=480;
export const WAVEFORM_BASE_SCALE=.13;
export interface WaveformSample {rms:number;receivedAt:number}
export interface WaveformState {envelope:number;history:number[];lastFrameAt:number;lastHistoryAt:number}
export function loudnessFromRms(rms:number):number {
 if(!Number.isFinite(rms)||rms<=0)return 0;
 const db=20*Math.log10(Math.min(1,rms));
 return Math.max(0,Math.min(1,(db-WAVEFORM_FLOOR_DB)/(WAVEFORM_TOP_DB-WAVEFORM_FLOOR_DB)));
}
export function initialWaveform(now:number):WaveformState {
 return {envelope:0,history:Array.from({length:WAVEFORM_BARS},()=>0),lastFrameAt:now,lastHistoryAt:now};
}
export function stepWaveform(state:WaveformState,sample:WaveformSample|null,now:number):WaveformState {
 const dt=Math.max(0,Math.min(100,now-state.lastFrameAt));
 const fresh=!!sample&&now>=sample.receivedAt&&now-sample.receivedAt<=WAVEFORM_STALE_MS;
 const target=fresh?loudnessFromRms(sample.rms):0;
 // Quick attack acknowledges speech; the slower release prevents harsh flicker.
 const tau=target>state.envelope?45:180;
 let envelope=state.envelope+(target-state.envelope)*(1-Math.exp(-dt/tau));
 if(envelope<.003)envelope=0;
 let history=state.history,lastHistoryAt=state.lastHistoryAt;
 if(now-state.lastHistoryAt>=WAVEFORM_HISTORY_MS){
  const count=Math.min(WAVEFORM_BARS,Math.floor((now-state.lastHistoryAt)/WAVEFORM_HISTORY_MS));
  history=[...history.slice(count),...Array.from({length:count},()=>envelope)];
  lastHistoryAt=now;
 }
 return {envelope,history,lastFrameAt:now,lastHistoryAt};
}
export function waveformScale(loudness:number):number {
 return WAVEFORM_BASE_SCALE+(1-WAVEFORM_BASE_SCALE)*Math.max(0,Math.min(1,loudness));
}
export function waveformQuiet(state:WaveformState,sample:WaveformSample|null,now:number):boolean {
 return (!sample||now-sample.receivedAt>WAVEFORM_STALE_MS)&&state.envelope===0&&state.history.every(value=>value===0);
}
