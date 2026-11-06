use super::*;
use std::cell::{Cell, RefCell};

fn opaque_target()->TargetSnapshot {
    TargetSnapshot{hwnd:10,process_id:20,focused_hwnd:10,runtime_id:vec![],selection:None,
        process_created_at:50,compatibility:Some(InputActivity{generation:1,interactions:7})}
}

#[test]
fn opaque_window_never_claims_a_caret_and_old_snapshots_remain_strict() {
    let target=opaque_target();assert!(compare_targets(&target,&target).is_ok());
    let mut strict=target.clone();strict.compatibility=None;
    assert!(compare_targets(&strict,&strict).is_err());
    let legacy=r#"{"hwnd":10,"processId":20,"focusedHwnd":30,"runtimeId":[1,2],"selection":[4,4],"processCreatedAt":50}"#;
    let legacy:TargetSnapshot=serde_json::from_str(legacy).unwrap();
    assert!(legacy.compatibility.is_none());assert!(compare_targets(&legacy,&legacy).is_ok());
    assert!(!serde_json::to_string(&legacy).unwrap().contains("compatibility"));
    assert!(capture_input_context(&target).is_err()); // Rejected before any OS/UIA inspection.
}

#[test]
fn writable_control_without_a_caret_uses_runtime_identity_and_the_same_once_only_gate() {
    let mut target=opaque_target();target.focused_hwnd=30;target.runtime_id=vec![42,30,91];
    assert!(compare_targets(&target,&target).is_ok());
    assert!(capture_input_context(&target).is_err());
    let ops=Operations::new(target.clone(),false);
    let permit=DeliveryPermit::from_committed_attempt("synthetic-value-provider".into(),1);
    let receipt=dispatch_with(&ops,&permit,&target,"synthetic message",||true);
    assert_eq!(receipt.input_events,4);assert!(matches!(receipt.outcome,DispatchOutcome::Unconfirmed));
    let mut changed=target.clone();changed.runtime_id[2]+=1;
    assert!(compare_targets(&target,&changed).is_err());
    let changed=Operations::new(changed,false);
    let permit=DeliveryPermit::from_committed_attempt("synthetic-changed-control".into(),1);
    let receipt=dispatch_with(&changed,&permit,&target,"synthetic message",||true);
    assert_eq!(receipt.input_events,0);assert_eq!(changed.copies.get(),0);
}

#[test]
fn any_observed_activity_or_window_identity_change_revokes_window_level_delivery() {
    let before=opaque_target();
    let mut alternatives=Vec::new();
    let mut changed=before.clone();changed.hwnd+=1;alternatives.push(changed);
    let mut changed=before.clone();changed.focused_hwnd+=1;alternatives.push(changed);
    let mut changed=before.clone();changed.process_id+=1;alternatives.push(changed);
    let mut changed=before.clone();changed.process_created_at+=1;alternatives.push(changed);
    let mut changed=before.clone();changed.compatibility.as_mut().unwrap().interactions+=1;alternatives.push(changed);
    let mut changed=before.clone();changed.compatibility.as_mut().unwrap().generation+=1;alternatives.push(changed);
    let mut changed=before.clone();changed.compatibility.as_mut().unwrap().generation=0;alternatives.push(changed);
    let mut changed=before.clone();changed.selection=Some((0,0));alternatives.push(changed);
    let mut changed=before.clone();changed.runtime_id=vec![42,10];alternatives.push(changed);
    let mut changed=before.clone();changed.compatibility=None;alternatives.push(changed);
    for changed in alternatives {assert!(compare_targets(&before,&changed).is_err());}
}

struct Operations {
    current:RefCell<TargetSnapshot>,copies:Cell<u32>,chords:Cell<u32>,change_during_copy:bool,
}
impl Operations {
    fn new(target:TargetSnapshot,change_during_copy:bool)->Self {
        Self{current:RefCell::new(target),copies:Cell::new(0),chords:Cell::new(0),change_during_copy}
    }
}
impl NativeOperations for Operations {
    fn validate(&self,target:&TargetSnapshot)->Result<()> {compare_targets(target,&self.current.borrow())}
    fn modifiers_released(&self)->bool{true}
    fn copy(&self,_:&str)->std::result::Result<bool,ClipboardError>{
        self.copies.set(self.copies.get()+1);
        if self.change_during_copy {self.current.borrow_mut().compatibility.as_mut().unwrap().interactions+=1;}
        Ok(true)
    }
    fn chord(&self)->u32{self.chords.set(self.chords.get()+1);4}
}

#[test]
fn unchanged_opaque_window_dispatches_one_chord_with_an_honest_unconfirmed_receipt() {
    let target=opaque_target();let ops=Operations::new(target.clone(),false);
    let permit=DeliveryPermit::from_committed_attempt("synthetic".into(),1);
    let receipt=dispatch_with(&ops,&permit,&target,"synthetic message",||true);
    assert!(matches!(receipt.outcome,DispatchOutcome::Unconfirmed));assert_eq!(receipt.input_events,4);
    assert_eq!(receipt.reason_code.as_deref(),Some("keyboardFocusInputDispatchedUnconfirmed"));
    let repeated=dispatch_with(&ops,&permit,&target,"synthetic message",||true);
    assert!(matches!(repeated.outcome,DispatchOutcome::Skipped));assert_eq!(ops.copies.get(),1);assert_eq!(ops.chords.get(),1);
}

#[test]
fn activity_before_or_during_copy_never_injects_into_the_opaque_window() {
    for during_copy in [false,true] {
        let target=opaque_target();let ops=Operations::new(target.clone(),during_copy);
        if !during_copy {ops.current.borrow_mut().compatibility.as_mut().unwrap().interactions+=1;}
        let permit=DeliveryPermit::from_committed_attempt("synthetic".into(),1);
        let receipt=dispatch_with(&ops,&permit,&target,"synthetic message",||true);
        assert!(matches!(receipt.outcome,DispatchOutcome::Skipped));assert_eq!(receipt.input_events,0);
        assert_eq!(receipt.clipboard_changed,during_copy);assert_eq!(ops.chords.get(),0);
        assert_eq!(receipt.reason_code.as_deref(),Some("inputActivityChanged"));
        let _=dispatch_with(&ops,&permit,&target,"synthetic message",||true);assert_eq!(ops.chords.get(),0);
    }
}

#[test]
fn cancellation_still_precedes_clipboard_and_input_for_an_opaque_window() {
    let target=opaque_target();let ops=Operations::new(target.clone(),false);
    let permit=DeliveryPermit::from_committed_attempt("synthetic".into(),1);assert!(permit.cancel_before_injection());
    let receipt=dispatch_with(&ops,&permit,&target,"synthetic message",||true);
    assert!(matches!(receipt.outcome,DispatchOutcome::Skipped));assert_eq!(ops.copies.get(),0);assert_eq!(ops.chords.get(),0);
}
