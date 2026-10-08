import {useEffect, useId, useRef, useState} from 'react';
import {Button} from '@radix-ui/themes';
import {ChevronDownIcon, ChevronUpIcon} from '@radix-ui/react-icons';
import type {Dictation} from '../model';
import {RecordText} from './ui';

/** Bound only the home preview; copy and history still use the full revision. */
export function LatestDictationPreview({record}: {record: Dictation}) {
 const [expanded, setExpanded] = useState(false);
 const [overflowing, setOverflowing] = useState(false);
 const viewportRef = useRef<HTMLDivElement>(null);
 const contentRef = useRef<HTMLDivElement>(null);
 const toggleRef = useRef<HTMLButtonElement>(null);
 const id = useId();
 const text = record.current.join('\n');
 useEffect(() => {
  const viewport = viewportRef.current, content = contentRef.current;
  if (!viewport || !content) return;
  const measure = () => {
   const limit = Number.parseFloat(getComputedStyle(viewport).getPropertyValue('--latest-preview-height'));
   setOverflowing(content.scrollHeight > limit + 1);
  };
  const observer = new ResizeObserver(measure);
  observer.observe(content); measure();
  return () => observer.disconnect();
 }, [text]);
 function toggle() {
  setExpanded(value => !value);
  if (expanded) requestAnimationFrame(() => toggleRef.current?.scrollIntoView({block: 'nearest', behavior: 'instant'}));
 }
 return <div className="latest-preview">
  <div id={id} ref={viewportRef} className="latest-preview-viewport" data-expanded={expanded} data-overflowing={overflowing}>
   <div ref={contentRef}><RecordText record={record}/></div>
  </div>
  {overflowing && <div className="latest-preview-footer">
   <span>{expanded ? '已显示完整内容' : '内容较长，当前显示部分预览'}</span>
   <Button ref={toggleRef} size="1" variant="ghost" aria-expanded={expanded} aria-controls={id} onClick={toggle}>
    {expanded ? '收起全文' : '展开全文'}{expanded ? <ChevronUpIcon/> : <ChevronDownIcon/>}
   </Button>
  </div>}
 </div>;
}
