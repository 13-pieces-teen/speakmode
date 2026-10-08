import {createContext,useContext,type ReactNode} from 'react';
import {useDesktop} from '../services/DesktopProvider';
import {fromModeId,toModeId,type Mode} from '../model';
import type {ModeRecord} from '../services/contracts';
import {isPersonalMode,type PersonalMode} from '../modes';
export type ModeDraft=Omit<PersonalMode,'version'>&{expectedRevision:number|null};
interface Catalog {modes:PersonalMode[];defaultMode:Mode;setDefaultMode:(mode:Mode)=>Promise<void>;saveMode:(mode:ModeDraft)=>Promise<PersonalMode>;removeMode:(id:PersonalMode['id'])=>Promise<void>;findMode:(id:Mode)=>PersonalMode|undefined;label:(id:Mode)=>string;sessionAvailable:boolean}
const ModeContext=createContext<Catalog|null>(null);
function personal(record:ModeRecord):PersonalMode{return {id:record.id,name:record.name,base:record.baseMode==='structure'?'结构':'轻整理',instructions:record.rules,version:record.revision};}
export function ModeProvider({children}:{children:ReactNode}) {
 const desktop=useDesktop();const modes=desktop.modes.filter(m=>!m.builtin).map(personal);const findMode=(id:Mode)=>modes.find(m=>m.id===id);
 return <ModeContext.Provider value={{modes,defaultMode:fromModeId(desktop.preferences.defaultModeId),sessionAvailable:desktop.available,findMode,label:id=>findMode(id)?.name??(isPersonalMode(id)?'个人 Mode':id),
 setDefaultMode:async mode=>{await desktop.command('mode.setDefault',{modeId:toModeId(mode),expectedVersion:desktop.preferences.version});await desktop.refresh();},
 saveMode:async draft=>{const saved=await desktop.command<ModeRecord>('mode.save',{draft:{id:draft.expectedRevision===null?null:draft.id,name:draft.name.trim(),baseMode:draft.base==='结构'?'structure':'light',rules:draft.instructions.trim(),expectedRevision:draft.expectedRevision}});await desktop.refresh();return personal(saved);},
 removeMode:async id=>{await desktop.command('mode.delete',{modeId:id,expectedRevision:findMode(id)?.version});await desktop.refresh();}
 }}>{children}</ModeContext.Provider>;
}
export function useModes(){const value=useContext(ModeContext);if(!value)throw new Error('ModeProvider is required');return value;}

