import type {LiveSession} from './contracts';
const id=(session:LiveSession|null)=>session?.sessionId??session?.id;
const live=(session:LiveSession|null)=>!!session&&['starting','recording'].includes(session.state.toLowerCase());

/** Renderer monotonic receipt time is only for meter freshness, never cloud latency. */
export function receiveLevel(session:LiveSession|null,payload:{sessionId?:string;generation?:number;level:number},at:number):LiveSession|null{
 if(!live(session)||!session||id(session)!==payload.sessionId||session.generation!==payload.generation||!Number.isFinite(payload.level)||!Number.isFinite(at))return session;
 return {...session,level:Math.max(0,Math.min(1,payload.level)),levelReceivedAt:at};
}
/** A caption/Mode snapshot must not erase a fresh pulse, or create a fake heartbeat. */
export function mergeLiveLevel(previous:LiveSession|null,next:LiveSession|null):LiveSession|null{
 if(!next)return null;
 if(!live(next))return {...next,level:0,levelReceivedAt:undefined};
 const same=previous&&id(previous)===id(next)&&previous.generation===next.generation;
 return {...next,...(same&&previous.levelReceivedAt!==undefined?{level:previous.level,levelReceivedAt:previous.levelReceivedAt}:{levelReceivedAt:undefined})};
}
