!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "OpenCoworkLocalDaemon"
  FindFirst $0 $1 "$LOCALAPPDATA\OpenCowork\daemon\bin\cowork-local-daemon-*.exe"
  open_cowork_daemon_uninstall_loop:
    StrCmp $1 "" open_cowork_daemon_uninstall_done
    nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /T /IM "$1"'
    FindNext $0 $1
    Goto open_cowork_daemon_uninstall_loop
  open_cowork_daemon_uninstall_done:
  FindClose $0
  Sleep 500
  RMDir /r "$LOCALAPPDATA\OpenCowork\daemon\bin"
!macroend
