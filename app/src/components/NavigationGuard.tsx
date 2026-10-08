import {createContext,useCallback,useContext,useRef,useState,type ReactNode} from 'react';
import {AlertDialog,Button} from '@radix-ui/themes';
import {errorMessage} from '../services/bridge';
export interface EditGuard {dirty:boolean;pending?:boolean;save:()=>Promise<void>;discard:()=>void}
const Context=createContext<{register:(guard:EditGuard|null)=>void;request:(action:()=>void)=>void}|null>(null);
export function NavigationGuardProvider({children}:{children:ReactNode}) {
 const guard=useRef<EditGuard|null>(null),pending=useRef<(()=>void)|null>(null);
 const [open,setOpen]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState(''),[guardPending,setGuardPending]=useState(false);
 const register=useCallback((next:EditGuard|null)=>{guard.current=next;setGuardPending(Boolean(next?.pending));},[]);
 const request=useCallback((action:()=>void)=>{if(!guard.current?.dirty){action();return;}pending.current=action;setError('');setOpen(true);},[]);
 const leave=(discard:boolean)=>{if(discard)guard.current?.discard();const action=pending.current;pending.current=null;guard.current=null;setOpen(false);action?.();};
 async function save(){setBusy(true);setError('');try{await guard.current?.save();leave(false);}catch(e){setError(errorMessage(e));}finally{setBusy(false);}}
 return <Context.Provider value={{register,request}}>{children}<AlertDialog.Root open={open} onOpenChange={next=>{if(!busy)setOpen(next);}}><AlertDialog.Content maxWidth="460px"><AlertDialog.Title>保存这次修改？</AlertDialog.Title><AlertDialog.Description>{guardPending?'正在处理这次操作，请等完成后再离开。':'离开前可以保存当前文本，或放弃尚未保存的编辑。'}</AlertDialog.Description>{error&&<p role="alert" className="inline-error">{error}</p>}<div className="guard-actions"><Button disabled={busy} variant="soft" color="gray" onClick={()=>setOpen(false)}>继续编辑</Button><Button disabled={busy||guardPending} variant="soft" color="gray" onClick={()=>leave(true)}>放弃修改</Button><Button loading={busy} disabled={guardPending} onClick={()=>void save()}>保存并离开</Button></div></AlertDialog.Content></AlertDialog.Root></Context.Provider>;
}
export function useNavigationGuard(){const value=useContext(Context);if(!value)throw new Error('NavigationGuardProvider is required');return value;}

