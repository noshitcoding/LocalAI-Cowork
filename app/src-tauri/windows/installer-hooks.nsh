!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "OpenCoworkLocalDaemon"
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /IM "cowork-local-daemon-*.exe"'
  RMDir /r "$LOCALAPPDATA\OpenCowork\daemon\bin"
!macroend
