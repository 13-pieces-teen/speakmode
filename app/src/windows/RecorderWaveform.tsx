import {useEffect,useLayoutEffect,useRef} from 'react';
import {initialWaveform,stepWaveform,waveformQuiet,waveformScale,WAVEFORM_BARS,type WaveformSample} from './waveform';

export function RecorderWaveform({level,receivedAt}:{level:number;receivedAt?:number}){
 const bars=useRef<(HTMLSpanElement|null)[]>([]),frame=useRef(0),sample=useRef<WaveformSample|null>(null),legacySeeded=useRef(false);
 const display=useRef(initialWaveform(performance.now())),live=useRef(true),reduce=useRef(false);
 const animate=useRef<(now:number)=>void>(()=>{});
 animate.current=now=>{
  if(!live.current)return;
  const previous=display.current,next=stepWaveform(previous,sample.current,now);display.current=next;
  // Imperative leaf updates keep waveform frames out of the Mode/caption tree.
  const values=reduce.current?next.history:[...next.history.slice(0,-1),next.envelope];
  values.forEach((value,index)=>{const bar=bars.current[index];if(!bar)return;const transform=`scaleY(${waveformScale(value).toFixed(4)})`;if(bar.style.transform!==transform)bar.style.transform=transform;});
  frame.current=waveformQuiet(next,sample.current,now)?0:requestAnimationFrame(animate.current);
 };
 useEffect(()=>{
  live.current=true;const motion=matchMedia('(prefers-reduced-motion: reduce)');reduce.current=motion.matches;
  const changed=()=>{reduce.current=motion.matches;};motion.addEventListener('change',changed);
  return()=>{live.current=false;cancelAnimationFrame(frame.current);frame.current=0;motion.removeEventListener('change',changed);};
 },[]);
 useLayoutEffect(()=>{
  // Only event receipts renew freshness. A source snapshot is not an audio heartbeat.
  // Legacy snapshots can seed a visible level once, then decay if no events follow.
  if(!Number.isFinite(receivedAt)&&legacySeeded.current)return;
  legacySeeded.current=true;
  sample.current={rms:level,receivedAt:Number.isFinite(receivedAt)?receivedAt!:performance.now()};
  if(!frame.current)frame.current=requestAnimationFrame(animate.current);
 },[level,receivedAt]);
 return <div className="recorder-level-bars" aria-hidden="true" data-signal="microphone-rms">{Array.from({length:WAVEFORM_BARS},(_,index)=><span key={index} ref={element=>{bars.current[index]=element;}} style={{transform:`scaleY(${waveformScale(0)})`}}/>)}</div>;
}
