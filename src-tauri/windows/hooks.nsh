; QuotaDock installer hooks (bundle.windows.nsis.installerHooks), inserted by installer.nsi.
;
; Rules: never delete user data (%LOCALAPPDATA%\QuotaDock, %USERPROFILE%\.quotadock), never create
; or enable an autostart entry the user did not have, never edit Claude settings. Side effects of a
; real uninstall are skipped with /UPDATE (an upgrade runs the old uninstaller that way).
;
; This file is UTF-8 with a BOM: makensis reads it in the ANSI code page otherwise.

; The data folder, as decided by crates/quotadock-core/src/platform/data_location.rs (no override).
!define QD_DATADIR "$LOCALAPPDATA\QuotaDock"
!define QD_BRIDGE "$PROFILE\.quotadock\bin\quotadock-claude-bridge.exe"
!define QD_RUNKEY "Software\Microsoft\Windows\CurrentVersion\Run"

; Lets a running QuotaDock quit through its own exit path (polling stopped, app_stop logged) before
; Restart Manager would force it. A copy started from another folder (an unpackaged build) is not
; seen by Restart Manager at all, so an interactive run asks the user to quit it.
!macro QD_QUIT_RUNNING
  nsis_tauri_utils::FindProcessCurrentUser "${MAINBINARYNAME}.exe"
  Pop $R0
  ${If} $R0 = 0
  ${AndIf} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    DetailPrint "Asking QuotaDock to quit"
    ; A second launch forwards --quit to the running instance and exits at once.
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --quit'
    StrCpy $R1 0
    ${Do}
      Sleep 500
      IntOp $R1 $R1 + 1
      nsis_tauri_utils::FindProcessCurrentUser "${MAINBINARYNAME}.exe"
      Pop $R0
      ${If} $R0 <> 0
      ${OrIf} $R1 >= 30
        ${ExitDo}
      ${EndIf}
    ${Loop}
  ${EndIf}

  ${If} $LANGUAGE = 2052
    StrCpy $R9 "QuotaDock 正在运行。$\n$\n请在托盘图标的菜单中选择“退出”，然后点“重试”。"
  ${Else}
    StrCpy $R9 "QuotaDock is running.$\n$\nQuit it from the tray icon's menu, then click Retry."
  ${EndIf}
  ${Do}
    nsis_tauri_utils::FindProcessCurrentUser "${MAINBINARYNAME}.exe"
    Pop $R0
    ${If} $R0 <> 0
    ${OrIf} ${Silent}
    ${OrIf} $PassiveMode = 1
      ${ExitDo}
    ${EndIf}
    ; Cancel is the user's choice: exit code 1 and no aborted page, so a same-version page takes it
    ; as a cancel, not a failure. The jump counts instructions: +3 skips SetErrorLevel and Quit.
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$R9" IDRETRY +3
    SetErrorLevel 1
    Quit
  ${Loop}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro QD_QUIT_RUNNING
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Autostart: keep the user's choice when QuotaDock moves into this folder (for example from an
  ; unpackaged build). Only an existing value is re-pointed; none is created, and Task Manager's
  ; enabled/disabled state (StartupApproved) is left as it is.
  ReadRegStr $R0 HKCU "${QD_RUNKEY}" "${PRODUCTNAME}"
  ${If} $R0 != ""
    WriteRegStr HKCU "${QD_RUNKEY}" "${PRODUCTNAME}" '"$INSTDIR\${MAINBINARYNAME}.exe" --autostart'
    DetailPrint "Autostart now starts $INSTDIR\${MAINBINARYNAME}.exe"
  ${EndIf}

  ; Database backup before this version first opens it (it may migrate the schema or re-analyse
  ; history). QuotaDock is not running here (closed above), so a file copy is consistent; the WAL
  ; file, if any, holds committed data and is copied with it. A backup of this version that already
  ; exists is the one from before the upgrade: a later reinstall of the same version keeps it.
  ${If} ${FileExists} "${QD_DATADIR}\quotadock.sqlite3"
  ${AndIfNot} ${FileExists} "${QD_DATADIR}\backups\before-${VERSION}\quotadock.sqlite3"
    StrCpy $R1 "${QD_DATADIR}\backups\before-${VERSION}"
    CreateDirectory "$R1"
    Delete "$R1\quotadock.sqlite3-wal"
    CopyFiles /SILENT "${QD_DATADIR}\quotadock.sqlite3" "$R1"
    ${If} ${FileExists} "${QD_DATADIR}\quotadock.sqlite3-wal"
      CopyFiles /SILENT "${QD_DATADIR}\quotadock.sqlite3-wal" "$R1"
    ${EndIf}
    DetailPrint "Database backed up to $R1"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ; Claude Code's status line may still run QuotaDock's bridge. The uninstaller never deletes the
    ; bridge or edits Claude settings (it cannot tell whether the user changed them since), so it
    ; points to the in-app restore, which only restores a statusLine QuotaDock itself wrote.
    ${If} ${FileExists} "${QD_BRIDGE}"
    ${AndIfNot} ${Silent}
    ${AndIf} $PassiveMode <> 1
      ${If} $LANGUAGE = 2052
        StrCpy $R9 "Claude Code 的状态栏可能仍在使用 QuotaDock 的桥接程序（%USERPROFILE%\.quotadock\bin）。$\n$\n建议点“否”取消卸载：在 QuotaDock 中打开“设置 → AI 服务 → 状态栏桥接”，点“停用并恢复”，然后再卸载。$\n$\n点“是”继续卸载：桥接程序和 Claude 设置保持不变，状态栏仍可使用；以后重新安装 QuotaDock 即可恢复。"
      ${Else}
        StrCpy $R9 "Claude Code's status line may still use QuotaDock's bridge (%USERPROFILE%\.quotadock\bin).$\n$\nRecommended: click No to cancel, open QuotaDock > Settings > AI services > Status-line bridge, click $\"Disable and restore$\", then uninstall again.$\n$\nClick Yes to uninstall anyway: the bridge and your Claude settings stay as they are and the status line keeps working; reinstall QuotaDock later to restore it."
      ${EndIf}
      ; No is the choice this prompt recommends: exit code 1 and no aborted page, as in QD_QUIT_RUNNING.
      ; The jump counts instructions: +3 skips SetErrorLevel and Quit.
      MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 "$R9" IDYES +3
      SetErrorLevel 1
      Quit
    ${EndIf}

    !insertmacro QD_QUIT_RUNNING
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ; The stock uninstaller removes the Run value; also remove Task Manager's state for it and
    ; Windows' per-app notification settings, so nothing of the notification identity remains.
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "${PRODUCTNAME}"
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\${BUNDLEID}"
  ${EndIf}
!macroend
