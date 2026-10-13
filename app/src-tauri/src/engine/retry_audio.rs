//! Cached audio uses the same wall-clock cadence as a live 16 kHz capture.
//! Waiting starts after configuration ACK; backpressure never earns catch-up credit.
//! Cached PCM already includes the session's audio conditioning. Never process it again.
use super::*;

pub(super) async fn replay(cache:Vec<i16>,tx:mpsc::Sender<AsrInput>,cancel:CancellationToken,
    configured:Arc<AtomicBool>,notify:Arc<Notify>,diagnostics:&DiagnosticsRecorder,trace:Option<&TraceHandle>) {
    loop {
        let ready=notify.notified();
        if configured.load(Ordering::Acquire){break;}
        tokio::select!{biased;_=cancel.cancelled()=>return,_=ready=>{}}
    }
    let mut count=0;let mut bridge=BridgeTelemetry::default();
    for (sequence,frame) in cache.chunks(1600).enumerate(){
        // A fresh relative sleep after every successful queue send prevents
        // timer stalls and channel backpressure from causing a later burst.
        let duration=Duration::from_secs_f64(frame.len() as f64/16_000.0);
        tokio::select!{biased;_=cancel.cancelled()=>return,_=tokio::time::sleep(duration)=>{}}
        let mut pcm16=frame.to_vec();let padding_samples=1600-pcm16.len();pcm16.resize(1600,0);
        let input=AsrInput::Frame{sequence:sequence as u64,pcm16,padding_samples};
        let counts=input.frame_counts();if let Some((real,padding))=counts{bridge.observed.add_frame(real,padding);}
        let sent=tokio::select!{biased;_=cancel.cancelled()=>return,r=tx.send(input)=>r};if sent.is_err(){return;}
        count+=1;bridge.accepted(counts);diagnostics.bridge(trace,&bridge);
    }
    let sent=tokio::select!{biased;_=cancel.cancelled()=>return,r=tx.send(AsrInput::Finish{next_sequence:count})=>r};
    if sent.is_ok(){bridge.accepted(None);diagnostics.bridge(trace,&bridge);}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spawn(cache:Vec<i16>,capacity:usize)->(mpsc::Receiver<AsrInput>,CancellationToken,Arc<AtomicBool>,Arc<Notify>,tokio::task::JoinHandle<()>) {
        let(tx,rx)=mpsc::channel(capacity);let cancel=CancellationToken::new();let ready=Arc::new(AtomicBool::new(false));let notify=Arc::new(Notify::new());
        let(token,flag,signal)=(cancel.clone(),ready.clone(),notify.clone());
        let task=tokio::spawn(async move{replay(cache,tx,token,flag,signal,&DiagnosticsRecorder::default(),None).await});
        (rx,cancel,ready,notify,task)
    }
    #[tokio::test]
    async fn ack_gates_audio_and_every_real_sample_and_padding_precede_finish(){
        let pcm:Vec<i16>=(0..3601).map(|i|i as i16).collect();let(rx,cancel,ready,notify,task)=spawn(pcm.clone(),5);let mut rx=rx;
        tokio::time::sleep(Duration::from_millis(120)).await;assert!(rx.try_recv().is_err());
        let start=Instant::now();ready.store(true,Ordering::Release);notify.notify_waiters();let mut real=Vec::new();let mut padding=0;let mut count=0;
        loop{match tokio::time::timeout(Duration::from_secs(2),rx.recv()).await.unwrap().unwrap(){
            AsrInput::Frame{sequence,pcm16,padding_samples}=>{assert_eq!(sequence,count);count+=1;real.extend_from_slice(&pcm16[..pcm16.len()-padding_samples]);assert!(pcm16[pcm16.len()-padding_samples..].iter().all(|s|*s==0));padding+=padding_samples;},
            AsrInput::Finish{next_sequence}=>{assert_eq!(next_sequence,3);break;}}}
        assert_eq!(real,pcm);assert_eq!(padding,1199);assert!(start.elapsed()>=Duration::from_secs_f64(3601.0/16000.0));task.await.unwrap();assert!(!cancel.is_cancelled());
    }
    #[tokio::test]
    async fn queue_backpressure_does_not_accumulate_catch_up_credit(){
        let(mut rx,_,ready,notify,task)=spawn(vec![1;6400],1);ready.store(true,Ordering::Release);notify.notify_waiters();
        tokio::time::sleep(Duration::from_millis(330)).await;assert!(matches!(rx.recv().await,Some(AsrInput::Frame{sequence:0,..})));
        assert!(matches!(rx.recv().await,Some(AsrInput::Frame{sequence:1,..})));let sent=Instant::now();
        assert!(matches!(rx.recv().await,Some(AsrInput::Frame{sequence:2,..})));assert!(sent.elapsed()>=Duration::from_millis(100));
        assert!(matches!(rx.recv().await,Some(AsrInput::Frame{sequence:3,..})));assert!(matches!(rx.recv().await,Some(AsrInput::Finish{next_sequence:4})));task.await.unwrap();
    }
    #[tokio::test]
    async fn soft_voice_cache_is_replayed_once_without_reapplying_gain(){
        use crate::platform::audio_conditioner::{AudioConditioner,ConditioningMode};
        let mut audio:Vec<f32>=(0..1777).map(|i|(i as f32*0.071).sin()*0.01).collect();
        AudioConditioner::new(ConditioningMode::SoftVoice).process(&mut audio);
        let cache:Vec<i16>=audio.iter().map(|v|(v*32768.0).round().clamp(-32768.0,32767.0) as i16).collect();
        let(mut rx,_,ready,notify,task)=spawn(cache.clone(),4);
        ready.store(true,Ordering::Release);notify.notify_waiters();let mut received=Vec::new();
        while let Some(input)=rx.recv().await {
            match input {
                AsrInput::Frame{pcm16,padding_samples,..}=>received.extend_from_slice(&pcm16[..pcm16.len()-padding_samples]),
                AsrInput::Finish{next_sequence}=>{assert_eq!(next_sequence,2);break;}
            }
        }
        task.await.unwrap();assert_eq!(received,cache);
    }
    #[tokio::test]
    async fn cancellation_releases_ack_timer_and_full_queue_without_finish(){
        for phase in 0..3 {
            let(mut rx,cancel,ready,notify,task)=spawn(vec![1;6400],1);
            if phase>0{ready.store(true,Ordering::Release);notify.notify_waiters();}
            if phase==2{tokio::time::sleep(Duration::from_millis(250)).await;}
            cancel.cancel();tokio::time::timeout(Duration::from_millis(200),task).await.unwrap().unwrap();
            while let Ok(value)=rx.try_recv(){assert!(!matches!(value,AsrInput::Finish{..}));}
        }
    }
}
