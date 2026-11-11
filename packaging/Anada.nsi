Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "${NOTICES_INCLUDE}"
!ifndef BIN_DIR
!error "BIN_DIR is required"
!endif
!ifndef OUTPUT_FILE
!define OUTPUT_FILE "Anada-Setup.exe"
!endif
Name "Anada"
VIProductVersion "1.1.0.0"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Dependency copyright statements are included in third-party notices"
VIAddVersionKey /LANG=1033 "ProductName" "Anada"
VIAddVersionKey /LANG=1033 "ProductVersion" "1.1.0"
VIAddVersionKey /LANG=1033 "FileVersion" "1.1.0"
VIAddVersionKey /LANG=1033 "FileDescription" "Anada setup"
OutFile "${OUTPUT_FILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\Anada"
InstallDirRegKey HKCU "Software\Anada" "InstallLocation"
SetCompressor /SOLID lzma
!define MUI_ICON "..\assets\anada.ico"
!define MUI_UNICON "..\assets\anada.ico"
!define MUI_WELCOMEPAGE_TITLE "Anada"
!define MUI_WELCOMEPAGE_TEXT "Anada uses Virtual Audio Cable (VAC) for its four channels. Install VAC with its own installer, then use Set up Virtual Audio Cable in the Anada Start menu folder. This setup does not install or remove an audio driver.$\r$\n$\r$\nInstallation is for the current Windows user. It does not enable sign-in startup or change Windows audio defaults.$\r$\n$\r$\nBefore updating or removing Anada, exit its audio host from the tray. Settings are preserved."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\anada.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Anada now"
!insertmacro MUI_PAGE_FINISH
!define MUI_UNCONFIRMPAGE_TEXT_TOP "Remove Anada for the current user? Your settings and audio drivers will be kept. Exit its host from the tray before continuing."
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!macro SafetyCheck MODE
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File /oname=Check-InstallSafety.ps1 "Check-InstallSafety.ps1"
  System::Call 'kernel32::SetEnvironmentVariableW(w "ANADA_INSTALL_CHECK_DIRECTORY", w "$INSTDIR") i.r0'
  System::Call 'kernel32::SetEnvironmentVariableW(w "ANADA_INSTALL_CHECK_SCRIPT", w "$PLUGINSDIR\Check-InstallSafety.ps1") i.r0'
  System::Call 'kernel32::SetEnvironmentVariableW(w "ANADA_INSTALL_CHECK_MODE", w "${MODE}") i.r0'
  safety_check_retry:
  nsExec::ExecToStack `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -Command "& ([scriptblock]::Create([IO.File]::ReadAllText([Environment]::GetEnvironmentVariable('ANADA_INSTALL_CHECK_SCRIPT')))) -InstallDirectory ([Environment]::GetEnvironmentVariable('ANADA_INSTALL_CHECK_DIRECTORY')) -Removing:([Environment]::GetEnvironmentVariable('ANADA_INSTALL_CHECK_MODE') -eq 'remove')"`
  Pop $0
  Pop $1
  ${If} $0 != 0
    IfSilent safety_check_cancel
    ${If} $1 == ""
      StrCpy $1 "Anada could not complete the installation safety check (result: $0)."
    ${EndIf}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$1$\r$\n$\r$\nResolve the issue above, then click Retry to check again. Cancel stops this operation." IDRETRY safety_check_retry
    safety_check_cancel:
    SetErrorLevel 2
    Quit
  ${EndIf}
  SetErrorLevel 0
!macroend
Section "Anada"
  !insertmacro SafetyCheck install
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  ClearErrors
  File "${BIN_DIR}\anada.exe"
  File "${BIN_DIR}\anada-host.exe"
  File "Check-InstallSafety.ps1"
  File "Anada-install.txt"
  File "Configure-VAC.cmd"
  File "Configure-VAC.ps1"
  File "Restore-VAC.cmd"
  !insertmacro InstallNotices
  IfErrors 0 +3
    SetErrorLevel 3
    Abort "Could not write Anada files. Close Anada and try again."
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Anada"
  CreateShortcut "$SMPROGRAMS\Anada\Anada.lnk" "$INSTDIR\anada.exe"
  CreateShortcut "$SMPROGRAMS\Anada\Uninstall Anada.lnk" "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\Anada\Set up Virtual Audio Cable.lnk" "$INSTDIR\Configure-VAC.cmd"
  CreateShortcut "$SMPROGRAMS\Anada\Restore Virtual Audio Cable settings.lnk" "$INSTDIR\Restore-VAC.cmd"
  WriteRegStr HKCU "Software\Anada" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Anada" "DisplayName" "Anada"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Anada" "DisplayVersion" "1.1.0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Anada" "Publisher" "Anada"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Anada" "DisplayIcon" "$INSTDIR\anada.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Anada" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Anada" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Anada" "QuietUninstallString" '$\"$INSTDIR\Uninstall.exe$\" /S'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Anada" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Anada" "NoRepair" 1
SectionEnd
Section "Uninstall"
  !insertmacro SafetyCheck remove
  SetShellVarContext current
  ClearErrors
  Delete "$INSTDIR\anada.exe"
  Delete "$INSTDIR\anada-host.exe"
  Delete "$INSTDIR\Check-InstallSafety.ps1"
  Delete "$INSTDIR\Configure-VAC.cmd"
  Delete "$INSTDIR\Configure-VAC.ps1"
  Delete "$INSTDIR\Restore-VAC.cmd"
  !insertmacro DeleteNotices
  IfErrors 0 +3
    SetErrorLevel 4
    Abort "Could not remove Anada files. Close Anada and retry removal."
  !insertmacro RemoveNoticeDirectories
  Delete "$INSTDIR\Anada-install.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Anada\Anada.lnk"
  Delete "$SMPROGRAMS\Anada\Uninstall Anada.lnk"
  Delete "$SMPROGRAMS\Anada\Set up Virtual Audio Cable.lnk"
  Delete "$SMPROGRAMS\Anada\Restore Virtual Audio Cable settings.lnk"
  RMDir "$SMPROGRAMS\Anada"
  ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Anada"
  ${If} $0 == '"$INSTDIR\anada-host.exe"'
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Anada"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "Anada"
  ${EndIf}
  ReadRegStr $0 HKCU "Software\Anada" "InstallLocation"
  ${If} $0 == $INSTDIR
    DeleteRegValue HKCU "Software\Anada" "InstallLocation"
    DeleteRegKey /ifempty HKCU "Software\Anada"
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Anada"
  ${EndIf}
SectionEnd









