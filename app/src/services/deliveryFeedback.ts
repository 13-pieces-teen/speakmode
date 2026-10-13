import type {LiveSession} from './contracts';

export interface DeliveryFeedback {
 title:string;
 message:string;
 warning:boolean;
 canCopyOutput:boolean;
 canViewOutput:boolean;
}

const failureMessages:Record<string,string>={
 targetUnavailableAtStop:'未找到可输入的位置，成稿已保留。',
 stopAnchorUnavailable:'未能确认当前输入窗口，成稿已保留。',
 stopFocusChanged:'结束口述时输入位置已改变，成稿已保留。',
 targetNotEditable:'当前位置不是可编辑的文本输入框，成稿已保留。',
 targetCapabilityUnsupported:'这个输入框无法确认光标位置，成稿已保留，可直接复制后手动粘贴。',
 targetInspectionTimeout:'输入框响应较慢，未能及时确认输入位置。成稿已保留。',
 targetHigherIntegrity:'目标应用以管理员权限运行，本次未自动输入。成稿已保留，可直接复制后手动粘贴。',
 targetChangedBeforeDelivery:'输入位置已改变，本次未自动输入。成稿已保留。',
 modifiersNotReleased:'快捷键尚未松开，本次未自动输入。成稿已保留。',
 clipboardUnavailable:'剪贴板暂不可用，本次未自动输入。成稿已保留。',
 nativeDispatchFailed:'自动输入未完成，请先核对目标中的文本。成稿已保留。',
 deliveryLedgerUnavailable:'未能准备本次自动输入，成稿已保留。',
};

/** Ready means a saved output, not proof that another application received it. */
export function deliveryFeedback(session:LiveSession|null):DeliveryFeedback|null {
 if(!session||session.state.toLowerCase()!=='ready')return null;
 const fallback=session.rewriteFallback?'已使用识别稿。':'';
 switch(session.deliveryState){
  case 'prepared':case 'dispatched':
   return {title:'正在发送成稿',message:fallback+'正在发送到输入位置…',warning:false,canCopyOutput:false,canViewOutput:false};
  case 'unconfirmed':
   if(session.deliveryFailureReason==='partialInputUnconfirmed')return {title:'自动输入未完整',message:fallback+'请先核对输入框中的内容，需要时可复制成稿。',warning:true,canCopyOutput:true,canViewOutput:true};
   return {title:'输入已派发',message:fallback+'请核对目标中的文本，应用不会自动重复输入。',warning:false,canCopyOutput:false,canViewOutput:true};
  case 'confirmed':return null;
  case 'skipped':case 'failed':
   return {title:session.deliveryState==='skipped'?'成稿未自动输入':'自动输入未完成',
    message:fallback+(failureMessages[session.deliveryFailureReason??'']??(session.deliveryState==='failed'
     ?'自动输入未完成，请先核对目标中的文本。成稿已保留。'
     :'未自动输入，成稿已保留。')),warning:true,canCopyOutput:true,canViewOutput:true};
  default:return null;
 }
}
