Unicode true
!include "MUI2.nsh"
!define APP "Vaultpack"
!define EXE "vaultpack.exe"
Name "${APP}"
OutFile "..\dist\Vaultpack-Setup.exe"
InstallDir "$PROGRAMFILES64\${APP}"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!define MUI_ICON "..\crates\vaultpack-app\assets\vaultpack.ico"
!define MUI_UNICON "..\crates\vaultpack-app\assets\vaultpack.ico"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetOutPath "$INSTDIR"
  File "..\target\release\${EXE}"
  File "..\crates\vaultpack-app\assets\vpk.ico"
  CreateShortcut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\${EXE}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCR ".vpk" "" "Vaultpack.Archive"
  WriteRegStr HKCR "Vaultpack.Archive" "" "Vaultpack Archive"
  WriteRegStr HKCR "Vaultpack.Archive\DefaultIcon" "" "$INSTDIR\vpk.ico"
  WriteRegStr HKCR "Vaultpack.Archive\shell\open\command" "" '"$INSTDIR\${EXE}" "%1"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}" "DisplayName" "${APP}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}" "DisplayIcon" "$INSTDIR\${EXE}"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\${APP}.lnk"
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\vpk.ico"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCR ".vpk"
  DeleteRegKey HKCR "Vaultpack.Archive"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"
SectionEnd
