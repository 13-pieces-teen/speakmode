import {useEffect,useRef,useState} from 'react';
import {Button,IconButton,Spinner,TextField} from '@radix-ui/themes';
import {Cross2Icon,MagnifyingGlassIcon,ReaderIcon} from '@radix-ui/react-icons';
import {PageHeading} from '../components/ui';
import {HistoryListItem} from '../components/HistoryListItem';
import {HistoryDetail} from '../components/HistoryDetail';
import {useDesktop} from '../services/DesktopProvider';
import {errorMessage} from '../services/bridge';
import {useNavigationGuard} from '../components/NavigationGuard';
import {fromDetail,type Dictation} from '../model';
import './history.css';
export function HistoryPage({selectedId,onSelect}:{selectedId:string|null;onSelect:(id:string|null)=>void}){
 const desktop=useDesktop(),navigation=useNavigationGuard();const [query,setQuery]=useState(desktop.query),[error,setError]=useState(''),[loading,setLoading]=useState(false);
 const [fetched,setFetched]=useState<Dictation|null>(null);
 const searchRef=useRef<HTMLInputElement>(null),searchRun=useRef(0);const loadRef=useRef(desktop.loadRecord);loadRef.current=desktop.loadRecord;
 useEffect(()=>()=>{searchRun.current++;},[]);
 const listed=desktop.records.find(r=>r.id===selectedId);
 const selected=listed?.detail?listed:desktop.temporaryResult?.session.id===selectedId?fromDetail(desktop.temporaryResult,true):fetched?.id===selectedId?fetched:listed;
 useEffect(()=>{if(!selectedId||selected?.detail){setLoading(false);return;}let active=true;setLoading(true);setError('');void loadRef.current(selectedId).then(record=>{if(active)setFetched(record);}).catch(e=>{if(active)setError(errorMessage(e));}).finally(()=>{if(active)setLoading(false);});return()=>{active=false;};},[selectedId,selected?.detail]);
 const grouped=new Map<string,typeof desktop.records>();for(const record of desktop.records){const group=grouped.get(record.group)??[];group.push(record);grouped.set(record.group,group);}
 function search(value:string){navigation.request(()=>{const run=++searchRun.current;onSelect(null);setQuery(value);setError('');void desktop.searchHistory(value).catch(e=>{if(run===searchRun.current)setError(errorMessage(e));});});}
 return <div className="history-page page-stack" data-view={selectedId?'detail':'list'}><PageHeading title="历史记录" description="找回说过的话，需要时再核对和调整。"/>{error&&<p role="alert" className="inline-error">{error}</p>}<div className="history-layout">
  <section className="history-index surface" aria-label="口述记录"><div className="history-index-heading"><h2 className="section-title">口述记录</h2><span className="history-count" role="status">{desktop.historyLoading?'正在读取…':`${desktop.historyTotal} 条`}</span></div><TextField.Root ref={searchRef} size="2" type="search" placeholder="搜索口述记录" aria-label="搜索口述记录" value={query} onChange={e=>search(e.target.value)} className="history-search"><TextField.Slot><MagnifyingGlassIcon/></TextField.Slot>{desktop.historyLoading&&<TextField.Slot side="right"><Spinner aria-label="正在更新口述记录"/></TextField.Slot>}{query&&<TextField.Slot side="right"><IconButton size="1" variant="ghost" color="gray" aria-label="清空搜索" onClick={()=>{search('');searchRef.current?.focus();}}><Cross2Icon/></IconButton></TextField.Slot>}</TextField.Root>
   <div className="history-list-scroll" aria-busy={desktop.historyLoading}><div className="history-record-groups">{grouped.size===0?<div className="history-list-empty" role="status"><ReaderIcon/><h3>{desktop.historyLoading?'正在读取记录':query?'没有找到这段口述':'还没有口述记录'}</h3><p>{desktop.historyLoading?'正在更新口述列表，请稍候。':query?'试试其他关键词。':!desktop.available?'在桌面应用中完成口述后，记录会出现在这里。':'开启保留历史后，完成口述可在这里找回内容。'}</p></div>:[...grouped.entries()].map(([group,entries])=><section className="history-date-group" key={group} aria-label={group}><h3 className="history-date-label">{group}</h3><ul className="history-record-list">{entries.map(record=><li key={record.id}><HistoryListItem record={record} selected={record.id===selectedId} onSelect={id=>navigation.request(()=>onSelect(id))}/></li>)}</ul></section>)}</div>{desktop.nextCursor!==null&&<Button className="history-load-more" variant="soft" color="gray" loading={desktop.historyLoading} onClick={()=>{const run=searchRun.current;void desktop.loadMore().catch(e=>{if(run===searchRun.current)setError(errorMessage(e));});}}>载入更多</Button>}</div>
  </section><div className="history-detail-shell">{selected?.detail?<HistoryDetail key={selected.id} record={selected} onBack={()=>onSelect(null)}/>:<section className="history-unselected surface"><ReaderIcon/><h2>{loading?'正在读取完整内容':selectedId&&error?'无法读取此记录':'选择一条口述'}</h2><p>{loading?'请稍候。':'查看整理结果，或与识别稿对照。'}</p>{selectedId&&<Button variant="soft" onClick={()=>onSelect(null)}>返回列表</Button>}</section>}</div>
 </div></div>;
}


