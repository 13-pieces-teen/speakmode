import type {RestorePatch} from '../services/contracts';

export interface RestoreText {
 id:string;
 text:string;
 revisionId:string;
}
export interface ScalarSelection {start:number;end:number}
export interface RestoreSelection extends RestoreText {range:ScalarSelection}
export interface RestorePreview {
 kind:'insert'|'replace';
 sourceText:string;
 replacedText:string;
 text:string;
 patch:RestorePatch;
}

function scalarBoundary(text:string,offset:number):boolean {
 if(offset<=0||offset>=text.length)return true;
 const before=text.charCodeAt(offset-1),after=text.charCodeAt(offset);
 return !(before>=0xD800&&before<=0xDBFF&&after>=0xDC00&&after<=0xDFFF);
}

/** DOM offsets are UTF-16. Native restore ranges are Unicode scalar offsets.
 * Reject split surrogate boundaries rather than guessing what was selected. */
export function captureRestoreSelection(text:RestoreText,start:number,end:number):RestoreSelection|null {
 if(!Number.isInteger(start)||!Number.isInteger(end)||start<0||end<start||end>text.text.length
  ||!scalarBoundary(text.text,start)||!scalarBoundary(text.text,end))return null;
 return {...text,range:{start:Array.from(text.text.slice(0,start)).length,end:Array.from(text.text.slice(0,end)).length}};
}

function validSelection(current:RestoreText|undefined,selection:RestoreSelection|null,allowCaret:boolean):selection is RestoreSelection {
 if(!current||!selection||selection.id!==current.id||selection.revisionId!==current.revisionId
  ||selection.text!==current.text)return false;
 const {start,end}=selection.range,length=Array.from(current.text).length;
 return Number.isInteger(start)&&Number.isInteger(end)&&start>=0&&end>=start&&end<=length
  &&(allowCaret||end>start);
}

/** Only an explicitly captured, revision-bound target may yield a patch.
 * Character differences or provenance refs never infer a destination range. */
export function buildRestorePreview(source:RestoreText|undefined,target:RestoreText|undefined,
 sourceSelection:RestoreSelection|null,targetSelection:RestoreSelection|null):RestorePreview|null {
 if(!validSelection(source,sourceSelection,false)||!validSelection(target,targetSelection,true))return null;
 const sourceScalars=Array.from(sourceSelection.text),targetScalars=Array.from(targetSelection.text);
 const replacement=sourceScalars.slice(sourceSelection.range.start,sourceSelection.range.end).join('');
 const {start,end}=targetSelection.range;
 return {kind:start===end?'insert':'replace',sourceText:replacement,
  replacedText:targetScalars.slice(start,end).join(''),
  text:targetScalars.slice(0,start).join('')+replacement+targetScalars.slice(end).join(''),
  patch:{blockId:targetSelection.id,start,end,sourceSelections:[{
   itemId:sourceSelection.id,start:sourceSelection.range.start,end:sourceSelection.range.end,
  }]}};
}
