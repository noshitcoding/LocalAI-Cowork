; The marker is part of the signed PE version resource, outside compressed data.
; The update observer accepts only this exact original-process completion contract.
VIAddVersionKey /LANG=1033 "CoworkInstallerContract" "nsis.currentUser.originalProcess.v1"
CRCCheck force

; Tauri's default macro can kill processes by executable name. Replace it with
; an exact installation-file availability check, without terminating any process.
!ifmacrodef CheckIfAppIsRunning
  !macroundef CheckIfAppIsRunning
!else
  !error "Review the changed Tauri installer process-check contract before release"
!endif

!macro CheckIfAppIsRunning executableName productName
  ${If} ${FileExists} "$INSTDIR\${executableName}"
    System::Call 'kernel32::CreateFileW(w "$INSTDIR\${executableName}", i 0xC0000000, i 0, p 0, i 3, i 0x00200000, p 0) p .r9'
    ${If} $9 == -1
      nsis_tauri_utils::StrReplace "$(appRunning)" "{{product_name}}" "${productName}"
      Pop $R1
      SetErrorLevel 2
      Abort $R1
    ${Else}
      System::Call 'kernel32::CloseHandle(p r9)'
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !if "${INSTALLMODE}" != "currentUser"
    !error "Cowork update observation requires the currentUser original-process installer"
  !endif
!macroend

; Both registration and removal run as the installing current user. The host
; installs an immutable private copy so AppImage/installer paths are not runtime
; dependencies of the browser extension.
!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog '"$INSTDIR\cowork-browser-host.exe" --register'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog '"$INSTDIR\cowork-browser-host.exe" --unregister'
  Pop $0
!macroend
