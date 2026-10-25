// Included inside the fixed native inspection worker. Metadata only; never
// reads UI text, changes focus, probes with input, or repeats a paste.
pub fn capture_activity() -> Result<InputActivity> {
    let before=handy_keys::input_activity_snapshot()
        .ok_or_else(||target_failure(TargetCaptureFailure::InputActivityUnavailable))?;
    // A held mouse button can be dragging a selection despite an unchanged
    // opaque provider. Modifier releases after stop are allowed; dispatch
    // still waits for modifiers before clipboard/input and rechecks afterwards.
    if [1,2,4,5,6].iter().any(|key| unsafe{GetAsyncKeyState(*key)} as u16 & 0x8000 != 0) {
        return Err(target_failure(TargetCaptureFailure::InputActivityChanged));
    }
    let after=handy_keys::input_activity_snapshot()
        .ok_or_else(||target_failure(TargetCaptureFailure::InputActivityUnavailable))?;
    if before!=after {return Err(target_failure(TargetCaptureFailure::InputActivityChanged));}
    Ok(InputActivity{generation:before.generation,interactions:before.interactions})
}

fn capture_compatibility_window(anchor:ForegroundAnchor,activity:InputActivity)->Result<TargetSnapshot> {
    capture_keyboard_target(anchor,activity,true)
}

fn capture_keyboard_target(anchor:ForegroundAnchor,activity:InputActivity,opaque_only:bool)->Result<TargetSnapshot> {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    let unsupported=||target_failure(TargetCaptureFailure::TargetCapabilityUnsupported);
    if capture_activity()?!=activity {return Err(target_failure(TargetCaptureFailure::InputActivityChanged));}
    if capture_anchor()?!=anchor {return Err(target_failure(TargetCaptureFailure::StopFocusChanged));}
    if anchor.hwnd==0 || anchor.focused_hwnd==0 || anchor.selection.is_some(){return Err(unsupported());}
    require_external_target_process(anchor.process_id,unsafe{GetCurrentProcessId()})?;
    unsafe {
        let hwnd=HWND(anchor.hwnd as _);
        if !IsWindowVisible(hwnd).as_bool() || !IsWindowEnabled(hwnd).as_bool(){return Err(unsupported());}
        let mut target=TargetSnapshot{hwnd:anchor.hwnd,process_id:anchor.process_id,focused_hwnd:anchor.focused_hwnd,
            process_created_at:anchor.process_created_at,runtime_id:vec![],selection:None,compatibility:Some(activity)};
        CoInitializeEx(None,COINIT_MULTITHREADED).ok().map_err(fail)?;let _com=ComGuard;
        let automation:IUIAutomation=CoCreateInstance(&CUIAutomation8,None,CLSCTX_INPROC_SERVER).map_err(fail)?;
        let options=automation.cast::<IUIAutomation2>().map_err(fail)?;
        options.SetConnectionTimeout(200).map_err(fail)?;options.SetTransactionTimeout(200).map_err(fail)?;
        let focused=automation.GetFocusedElement().map_err(fail)?;
        // A real writable control is eligible across applications even when
        // its provider has no selection/prefix API. Never read Value or GetText.
        if focused.CurrentProcessId().map_err(fail)? as u32!=anchor.process_id
            || focused.CurrentIsPassword().map_err(fail)?.as_bool()
            || !focused.CurrentIsEnabled().map_err(fail)?.as_bool()
            || !focused.CurrentIsKeyboardFocusable().map_err(fail)?.as_bool()
            || !focused.CurrentHasKeyboardFocus().map_err(fail)?.as_bool() {
            return Err(target_failure(TargetCaptureFailure::TargetNotEditable));
        }
        let control=focused.CurrentControlType().map_err(fail)?;
        if !opaque_only && [UIA_EditControlTypeId,UIA_DocumentControlTypeId,UIA_GroupControlTypeId].contains(&control) {
            if let Ok(value)=focused.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                if value.CurrentIsReadOnly().map_err(fail)?.as_bool() {return Err(target_failure(TargetCaptureFailure::TargetNotEditable));}
            }else {
                let text=focused.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId).map_err(fail)?;
                let readonly=text.DocumentRange().map_err(fail)?.GetAttributeValue(UIA_IsReadOnlyAttributeId).map_err(fail)?;
                if readonly==windows::core::VARIANT::from(true) {return Err(target_failure(TargetCaptureFailure::TargetNotEditable));}
                if !explicit_writable_text_attribute(&readonly) {return Err(unsupported());}
            }
            target.runtime_id=runtime_id_of(&focused)?;
            if target.runtime_id.is_empty() || target.runtime_id.len()>256 {return Err(unsupported());}
        }else {
            // Window-only routing is restricted to the observed WeChat root.
            // Other opaque windows, buttons and unknown/readonly fields stay blocked.
            if control!=UIA_WindowControlTypeId || anchor.focused_hwnd!=anchor.hwnd
                || focused.CurrentNativeWindowHandle().map_err(fail)?!=hwnd {return Err(unsupported());}
            let mut class=[0u16;128];let length=GetClassNameW(hwnd,&mut class);
            if length<=0 {return Err(unsupported());}
            let name=crate::platform::application::name_for_target(&target).ok_or_else(unsupported)?;
            if !wechat::supported_window(&name,&String::from_utf16_lossy(&class[..length as usize])) {return Err(unsupported());}
            if focused.GetCurrentPropertyValue(UIA_IsTextPatternAvailablePropertyId).map_err(fail)?!=windows::core::VARIANT::from(false)
                || focused.GetCurrentPropertyValue(UIA_IsValuePatternAvailablePropertyId).map_err(fail)?!=windows::core::VARIANT::from(false) {return Err(unsupported());}
        }
        let final_focused=automation.GetFocusedElement().map_err(fail)?;
        if !automation.CompareElements(&focused,&final_focused).map_err(fail)?.as_bool() {return Err(target_failure(TargetCaptureFailure::StopFocusChanged));}
        verify_input_route(&target)?;
        if capture_anchor()?!=anchor {return Err(target_failure(TargetCaptureFailure::StopFocusChanged));}
        if capture_activity()?!=activity {return Err(target_failure(TargetCaptureFailure::InputActivityChanged));}
        Ok(target)
    }
}
