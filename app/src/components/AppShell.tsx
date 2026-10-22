import type {ReactNode} from 'react';
import {GearIcon,HomeIcon,CounterClockwiseClockIcon,MixerHorizontalIcon,PlayIcon,ReaderIcon} from '@radix-ui/react-icons';
import {Button} from '@radix-ui/themes';
import {useDesktop} from '../services/DesktopProvider';
import {WindowChrome} from './WindowChrome';
export type View='home'|'history'|'settings'|'modes'|'dictionary';
export function AppShell({view,onNavigate,onRecorder,children}:{view:View;onNavigate:(view:View)=>void;onRecorder:()=>void;children:ReactNode}){
 const desktop=useDesktop();const active=desktop.session&&(desktop.session.workerReleased===false||!['idle','ready','cancelled','failed','incomplete'].includes(desktop.session.state.toLowerCase())||['prepared','dispatched'].includes(desktop.session.deliveryState??''));
 return <div className="desktop-frame"><WindowChrome available={desktop.available}/><div className="app-shell"><aside className="app-sidebar"><nav aria-label="主导航" className="main-nav"><button className="nav-item" aria-current={view==='home'?'page':undefined} onClick={()=>onNavigate('home')}><HomeIcon/>首页</button><button className="nav-item" aria-current={view==='history'?'page':undefined} onClick={()=>onNavigate('history')}><CounterClockwiseClockIcon/>历史记录</button><button className="nav-item" aria-current={view==='modes'?'page':undefined} onClick={()=>onNavigate('modes')}><MixerHorizontalIcon/>场合</button><button className="nav-item" aria-current={view==='dictionary'?'page':undefined} onClick={()=>onNavigate('dictionary')}><ReaderIcon/>词典</button><button className="nav-item settings-nav" aria-current={view==='settings'?'page':undefined} onClick={()=>onNavigate('settings')}><GearIcon/>设置</button></nav><div className="sidebar-bottom"><Button className="sidebar-record" variant="solid" size="2" disabled={!desktop.available||!desktop.provider.configured||Boolean(active)} onClick={onRecorder}><PlayIcon/>{active?'口述进行中':'开始口述'}</Button></div></aside><main className="app-main" key={view} data-view={view}>{children}</main></div></div>;
}


