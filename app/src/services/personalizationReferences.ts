import type {PersonalizationConfig,PersonalizationReference} from './personalization';

/** Content-free authority from a successful native configuration receipt. */
export interface PersonalizationReferencePolicy {
 version:number;
 profileIds:readonly string[];
 deletedProfileIds:readonly string[];
}
export const emptyPersonalizationReferencePolicy:PersonalizationReferencePolicy={version:0,profileIds:[],deletedProfileIds:[]};

export function advancePersonalizationReferencePolicy(previous:PersonalizationReferencePolicy,config:PersonalizationConfig):PersonalizationReferencePolicy {
 if(config.version<=previous.version)return previous;
 const profileIds=config.profiles.map(profile=>profile.id);
 const deleted=new Set(previous.deletedProfileIds);
 for(const id of previous.profileIds)if(!profileIds.includes(id))deleted.add(id);
 return {version:config.version,profileIds,deletedProfileIds:[...deleted]};
}

function deleted(reference:PersonalizationReference,policy:PersonalizationReferencePolicy):boolean {
 const id=reference.profileId;
 return Boolean(id&&(policy.deletedProfileIds.includes(id)||(policy.version>reference.version&&!policy.profileIds.includes(id))));
}

// These are the reference-bearing links in native replies, snapshots and local
// record/comparison views. Text blocks, source, IDs and revision markers stay intact.
const referenceLinks=['writingReceipt','personalization','output','detail','native','result','temporaryResult','session','recovery','reference'] as const;
/** Recheck at acceptance AND render time: a response may predate the deletion. */
export function clearDeletedPersonalizationReferences<T>(value:T,policy:PersonalizationReferencePolicy=emptyPersonalizationReferencePolicy):T {
 if(!value||typeof value!=='object'||policy.version===0)return value;
 if(Array.isArray(value)){
  const next=value.map(item=>clearDeletedPersonalizationReferences(item,policy));
  return (next.some((item,index)=>item!==value[index])?next:value) as T;
 }
 const object=value as Record<string,unknown>;
 if(typeof object.version==='number'&&Array.isArray(object.rules)&&Array.isArray(object.omitted)&&typeof object.scene==='string'&&deleted(value as unknown as PersonalizationReference,policy)){
  if(object.referenceCleared===true&&object.profileName==null&&object.background==null&&object.rules.length===0&&object.omitted.length===0)return value;
  return {...object,profileName:null,background:null,rules:[],omitted:[],referenceCleared:true} as T;
 }
 let next:Record<string,unknown>|undefined;
 for(const key of referenceLinks){
  const before=object[key];
  if(!before||typeof before!=='object')continue;
  const after=clearDeletedPersonalizationReferences(before,policy);
  if(after!==before){next??={...object};next[key]=after;}
 }
 return (next??value) as T;
}
