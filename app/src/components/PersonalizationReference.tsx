import type {PersonalizationReference as Reference} from '../services/personalization';

export function PersonalizationReference({reference,cleared=false}:{reference?:Reference|null;cleared?:boolean}){
 if(cleared||reference?.referenceCleared)return <p className="personalization-caption">参考详情已清除。</p>;
 if(!reference)return null;
 return <details className="personalization-reference"><summary>查看本次参考信息 · v{reference.version}</summary>
  <div><p>{reference.profileName?`身份：${reference.profileName}`:'仅使用通用表达习惯'}。以下是实际发送的参考，不代表模型全部遵守。</p>
  {reference.background&&<dl className="personalization-background">{(['profession','domain','description','audience','tasks'] as const).map(key=>{const value=reference.background![key],text=Array.isArray(value)?value.join('、'):value;return text?<div key={key}><dt>{({profession:'职业',domain:'领域',description:'工作描述',audience:'沟通对象',tasks:'常见任务'} as const)[key]}</dt><dd className="personalization-preserve">{text}</dd></div>:null;})}</dl>}
  {reference.rules.length>0&&<ul>{reference.rules.map((rule,index)=><li key={`${rule.key}-${index}`}>{rule.text}<small>来源：{rule.origin.startsWith('scene:')?'场合覆盖':({global:'通用习惯',scene:'场合覆盖',profile:'画像补充',template:'已接受的模板建议'} as Record<string,string>)[rule.origin]??rule.origin}</small></li>)}</ul>}
  {reference.omitted.length>0&&<p>受参考长度限制，本次未发送：{reference.omitted.map(key=>key.startsWith('background.tasks.')?'部分常见任务':({sentenceStyle:'句子写法',tone:'语气',keepEnglishTerms:'英文术语习惯',fewerCourtesies:'客套偏好',paragraphs:'段落偏好',lists:'分点偏好',instructions:'补充要求','background.profession':'职业','background.domain':'领域','background.audience':'沟通对象','background.description':'工作描述'} as Record<string,string>)[key]??key).join('、')}。</p>}
  </div></details>;
}
