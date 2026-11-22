# Clashfork NSIS hooks. These hooks only address the fork-owned service identity.
# The main installer is elevated (perMachine), so the service can be published here and
# removed by the uninstaller without a second elevation prompt.

!define CLASHFORK_CORE_SHA256 "512982f924c6e3ea95d5a66042ba1116074074db81097bd3e7e4e013d9df612a"

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Installing Clashfork Service and approved Mihomo core..."
  ${If} ${FileExists} "$INSTDIR\resources\clashfork-service-install.exe"
    nsExec::ExecToLog '"$INSTDIR\resources\clashfork-service-install.exe" --install-service --install-core "$INSTDIR\resources\clashfork-core.exe" --sha256 "${CLASHFORK_CORE_SHA256}"'
    Pop $0
    ${If} $0 != 0
      MessageBox MB_ICONEXCLAMATION "Clashfork was installed, but its TUN service could not be installed. Re-run the installer as administrator or reinstall Clashfork to enable the VPN."
    ${Else}
      DetailPrint "Clashfork Service is ready."
    ${EndIf}
  ${Else}
    MessageBox MB_ICONEXCLAMATION "Clashfork's TUN service files are missing from this installer. The app will run, but the VPN cannot be enabled."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Removing Clashfork Service..."
  ${If} ${FileExists} "$INSTDIR\resources\clashfork-service-uninstall.exe"
    nsExec::ExecToLog '"$INSTDIR\resources\clashfork-service-uninstall.exe"'
    Pop $0
    ${If} $0 != 0
      DetailPrint "Clashfork Service removal returned $0; a later reinstall can repair it."
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
    SetShellVarContext current
    RmDir /r "$LOCALAPPDATA\Clashfork"
    RmDir /r "$APPDATA\Clashfork"
  ${EndIf}
!macroend
