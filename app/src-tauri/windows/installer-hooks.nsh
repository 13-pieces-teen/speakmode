; Hooks execute before Tauri's running-process check and file/registry deletion.
; Default: preserve profile and credential. Updates/passive/silent runs preserve
; both without showing prompts. Only the app's early native maintenance branch
; knows the owned data directory and exact configured credential scope.
!include LogicLib.nsh

LangString SpeakModeCleanupFailed 1033 "SpeakMode could not confirm a clean exit. Quit it from the tray and retry. No installed files have been removed."
LangString SpeakModeCleanupFailed 2052 "SpeakMode 未能确认安全退出。请从托盘退出应用后重试。安装文件尚未删除。"
LangString SpeakModeDeleteData 1033 "Also delete SpeakMode history, Modes and settings? Choose No to keep them for reinstall."
LangString SpeakModeDeleteData 2052 "同时删除 SpeakMode 的历史记录、个人 Mode 和设置吗？选择“否”可在重装后继续使用。"
LangString SpeakModeDeleteKey 1033 "Also remove the API Key for the currently configured region/workspace from Windows Credential Manager? Choose No to keep it."
LangString SpeakModeDeleteKey 2052 "同时从 Windows 凭据管理器删除当前配置地域/工作区的 API Key 吗？选择“否”可保留。"

!macro NSIS_HOOK_PREINSTALL
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    ClearErrors
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --prepare-uninstall' $R8
    ${If} ${Errors}
    ${OrIf} $R8 != 0
      IfSilent +2
      MessageBox MB_OK|MB_ICONEXCLAMATION "$(SpeakModeCleanupFailed)"
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  StrCpy $R9 "--prepare-uninstall"
  ${IfNot} ${Silent}
  ${AndIf} $PassiveMode != 1
  ${AndIf} $UpdateMode != 1
    ; Use the explicit checkbox on Tauri's existing uninstall confirmation.
    ; The stock recursive deletion below must never independently act on it.
    ${If} $DeleteAppDataCheckboxState == 1
      StrCpy $R9 "$R9 --delete-user-data"
    ${EndIf}
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "$(SpeakModeDeleteKey)" IDNO speakmode_keep_key
      StrCpy $R9 "$R9 --delete-cloud-key"
    speakmode_keep_key:
  ${EndIf}
  StrCpy $DeleteAppDataCheckboxState 0
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    ClearErrors
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" $R9' $R8
    ${If} ${Errors}
    ${OrIf} $R8 != 0
      IfSilent +2
      MessageBox MB_OK|MB_ICONEXCLAMATION "$(SpeakModeCleanupFailed)"
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${Else}
    ; With no executable the verified cleanup path is unavailable. NSIS may
    ; remove installed remnants, but never guesses a user-data/credential path.
    ${If} $R9 != "--prepare-uninstall"
      IfSilent +2
      MessageBox MB_OK|MB_ICONEXCLAMATION "$(SpeakModeCleanupFailed)"
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode != 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "SpeakMode"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "SpeakMode"
  ${EndIf}
!macroend
