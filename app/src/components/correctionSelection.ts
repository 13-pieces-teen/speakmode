import type {CorrectionPatch} from '../services/contracts';
import type {RestoreSelection,RestoreText} from './restoreSelection';
export interface CorrectionPreview {text:string;patch:CorrectionPatch}
export interface CorrectionKeyboardSelection {start:number;end:number;direction:'forward'|'backward'}
/** Read-only WebView textareas do not consistently move their caret with arrows.
 * Handle only explicit horizontal selection. Other navigation/OS shortcuts keep
 * their browser behavior, including visual-line Home/End and Ctrl+A/C. */
export function moveCorrectionSelection(text:string,start:number,end:number,direction:string,key:string,shift:boolean,modified=false):CorrectionKeyboardSelection|null {
 if(modified||!['ArrowLeft','ArrowRight'].includes(key)||!Number.isInteger(start)||!Number.isInteger(end)||start<0||end<start||end>text.length)return null;
 const boundaries=[0];for(const char of Array.from(text))boundaries.push(boundaries[boundaries.length-1]+char.length);
 if(!boundaries.includes(start)||!boundaries.includes(end))return null;
 const right=key==='ArrowRight';
 if(!shift&&start!==end){const position=right?end:start;return {start:position,end:position,direction:'forward'};}
 const anchor=direction==='backward'?end:start,focus=direction==='backward'?start:end,index=boundaries.indexOf(focus);
 const next=boundaries[Math.max(0,Math.min(boundaries.length-1,index+(right?1:-1)))];
 return shift?{start:Math.min(anchor,next),end:Math.max(anchor,next),direction:next<anchor?'backward':'forward'}:{start:next,end:next,direction:'forward'};
}
export function correctionTermError(replacement:string):string {
 if(/[\p{Cc}\p{Cf}\p{Cs}\u2028\u2029]/u.test(replacement))return '词条不能包含换行、控制字符或隐藏字符。';
 if(!replacement.trim())return '请输入正确写法。';
 if(Array.from(replacement.trim()).length>80)return '每个词条最多 80 个字符。';
 return '';
}
/** Exact user-selected occurrence, bound to the displayed revision and full block.
 * No find/replace-all and no normalization of the surrounding text. */
export function buildCorrectionPreview(target:RestoreText|undefined,selection:RestoreSelection|null,replacement:string):CorrectionPreview|null {
 if(!target||!selection||target.id!==selection.id||target.revisionId!==selection.revisionId||target.text!==selection.text)return null;
 const scalars=Array.from(target.text),{start,end}=selection.range;
 if(!Number.isInteger(start)||!Number.isInteger(end)||start<0||end<=start||end>scalars.length||end-start>80)return null;
 const term=replacement.trim();
 if(correctionTermError(replacement))return null;
 const expectedText=scalars.slice(start,end).join('');
 if(expectedText===term)return null;
 return {text:scalars.slice(0,start).join('')+term+scalars.slice(end).join(''),patch:{blockId:target.id,start,end,expectedText,replacement:term}};
}
