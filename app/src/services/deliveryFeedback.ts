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
 targetHigherIntegrity:'目标应用以管理员权限运行，本次未自动输入。成稿已保留，请查看成稿后手动粘贴。',
 targetChangedBeforeDelivery:'输入位置已改变，本次未自动输入。成稿已保留。',
 modifiersNotReleased:'快捷键尚未松开，本次未自动输入。成稿已保留。',
 clipboardUnavailable:'剪贴板暂不可用，本次未自动输入。成稿已保留。',
 nativeDispatchFailed:'自动输入未完成，请先核对目标中的文本。成稿已保留。',
 deliveryLedgerUnavailable:'未能准备本次自动输入，成稿已保留。',
};

/** Ready means a saved output, not proof that another application received it. */
export function deliveryFeedback(session:LiveSession|null):DeliveryFeedback|null {
 if(!session||session.state.toLowerCase()!=='ready')return null;
 switch(session.deliveryState){
  case 'prepared':case 'dispatched':
   return {title:'正在发送成稿',message:'正在发送到输入位置…',warning:false,canCopyOutput:false,canViewOutput:false};
  case 'unconfirmed':
   return {title:'输入已派发',message:'请核对目标中的文本，应用不会自动重复输入。',warning:false,canCopyOutput:false,canViewOutput:true};
  case 'confirmed':return null;
  case 'skipped':case 'failed':
   return {title:session.deliveryState==='skipped'?'成稿未自动输入':'自动输入未完成',
    message:failureMessages[session.deliveryFailureReason??'']??(session.deliveryState==='failed'
     ?'自动输入未完成，请先核对目标中的文本。成稿已保留。'
     :'未自动输入，成稿已保留。'),warning:true,canCopyOutput:true,canViewOutput:true};
  default:return null;
 }
}
