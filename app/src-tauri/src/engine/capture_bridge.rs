//! Capture shutdown and product cutoff are independent of transient UI events.
use super::*;
use crossbeam_channel::Receiver;

pub(super) trait CaptureSource {
    fn frames(&self)->&Receiver<AudioFrame>;
    fn events(&self)->&Receiver<AudioEvent>;
    fn duration_ms(&self)->u64;
    fn request_stop(&self);
    fn cancel(&self);
    fn wait_drained(&mut self,deadline:Duration)->platform::Result<CaptureSummary>;
}
impl CaptureSource for CaptureHandle {
    fn frames(&self)->&Receiver<AudioFrame>{&self.frames}
    fn events(&self)->&Receiver<AudioEvent>{&self.events}
    fn duration_ms(&self)->u64{CaptureHandle::duration_ms(self)}
    fn request_stop(&self){CaptureHandle::request_stop(self)}
    fn cancel(&self){CaptureHandle::cancel(self)}
    fn wait_drained(&mut self,deadline:Duration)->platform::Result<CaptureSummary>{CaptureHandle::wait_drained(self,deadline)}
}

/// Validate native order before intentionally trimming the product cutoff.
/// Callback scheduling can drain beyond the last allowed source sample.
pub(super) struct FrameLimit {limit:usize,real:usize,observed:u64,pub next:u64,padded:bool}
impl FrameLimit {
    pub fn new(limit_ms:u64)->Self{Self{limit:limit_ms.saturating_mul(16) as usize,real:0,observed:0,next:0,padded:false}}
    pub fn limit_reached(&self)->bool{self.limit>0&&self.real>=self.limit}
    pub fn take(&mut self,mut frame:AudioFrame)->Result<Option<AudioFrame>>{
        let length=frame.pcm16.len();
        if self.padded||frame.sequence!=self.observed||length==0||length>1600||frame.padding_samples>=length
            ||(frame.padding_samples>0&&(length!=1600||frame.pcm16[length-frame.padding_samples..].iter().any(|s|*s!=0))){
            return Err(ApiError::new("audio_incomplete","audio","采音帧顺序或尾帧无效，已有内容仍可恢复"));
        }
        self.observed+=1;self.padded=frame.padding_samples>0;
        let real=length-frame.padding_samples;
        let take=real.min(self.limit.saturating_sub(self.real));
        if take==0{return Ok(None);}
        if take<real {frame.pcm16[take..].fill(0);frame.padding_samples=length-take;}
        frame.sequence=self.next;self.next+=1;self.real+=take;Ok(Some(frame))
    }
    pub fn finish(&self,emitted:u64)->Result<()>{
        if self.observed!=emitted{return Err(ApiError::new("audio_incomplete","audio","采音尾帧尚未收齐，已有内容仍可恢复"));}Ok(())
    }
}
