; NSIS installer for Context Password for Bitwarden.
;
; Build with: makensis packaging\installer.nsi
; Version is injected from outside (CI passes /DPRODUCT_VERSION=x.y.z);
; the !ifndef fallback lets a local build work without that flag.
;
; Assumes `cargo build --release` has already produced
; target\release\context-password.exe — this script only packages it.

!include "MUI2.nsh"

!ifndef PRODUCT_VERSION
  !define PRODUCT_VERSION "0.0.0"
!endif
; VIProductVersion below requires a strict X.X.X.X numeric string — a
; prerelease version like "1.0.0-rc.1" isn't valid there even though it's
; fine everywhere else in this script (filename, display strings, registry).
; Pass /DPRODUCT_VERSION_NUMERIC=x.y.z alongside /DPRODUCT_VERSION for a
; prerelease build; ordinary releases need nothing extra.
!ifndef PRODUCT_VERSION_NUMERIC
  !define PRODUCT_VERSION_NUMERIC "${PRODUCT_VERSION}"
!endif

!define PRODUCT_NAME "Context Password for Bitwarden"
!define PRODUCT_PUBLISHER "Dominik Dzienia"
!define PRODUCT_EXE "context-password.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}"

Name "${PRODUCT_NAME}"
OutFile "..\target\release\ContextPassword-${PRODUCT_VERSION}-x64-setup.exe"
; Install folder uses the crate name, not the display name — matches the
; MSI installer's own folder naming (`wix\main.wxs`'s APPLICATIONFOLDER).
InstallDir "$PROGRAMFILES64\context-password"
InstallDirRegKey HKLM "${UNINSTALL_KEY}" "InstallLocation"
RequestExecutionLevel admin
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${PRODUCT_VERSION_NUMERIC}.0"
VIAddVersionKey "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey "CompanyName" "${PRODUCT_PUBLISHER}"
VIAddVersionKey "LegalCopyright" "Copyright (c) 2026 ${PRODUCT_PUBLISHER}"
VIAddVersionKey "FileVersion" "${PRODUCT_VERSION}"
VIAddVersionKey "ProductVersion" "${PRODUCT_VERSION}"

;--------------------------------
; Modern UI 2 configuration — must come before the page macros below.

!define MUI_ICON "..\resources\app.ico"
!define MUI_UNICON "..\resources\app.ico"
!define MUI_ABORTWARNING

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE.txt"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES

; Finish page: two checkboxes, both checked by default — "Run" is native to
; MUI2; the second checkbox is the standard trick of repurposing the
; "show readme" slot to call a function instead of opening a file.
!define MUI_FINISHPAGE_RUN "$INSTDIR\${PRODUCT_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Run ${PRODUCT_NAME}"
!define MUI_FINISHPAGE_RUN_CHECKED
!define MUI_FINISHPAGE_SHOWREADME ""
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Create desktop shortcut"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION CreateDesktopShortcut
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

;--------------------------------

Function CreateDesktopShortcut
    CreateShortCut "$DESKTOP\${PRODUCT_NAME}.lnk" "$INSTDIR\${PRODUCT_EXE}"
FunctionEnd

Section "Install"
    SetOutPath "$INSTDIR"
    File "..\target\release\${PRODUCT_EXE}"
    File "..\LICENSE.txt"

    CreateDirectory "$SMPROGRAMS\${PRODUCT_NAME}"
    CreateShortCut "$SMPROGRAMS\${PRODUCT_NAME}\${PRODUCT_NAME}.lnk" "$INSTDIR\${PRODUCT_EXE}"
    CreateShortCut "$SMPROGRAMS\${PRODUCT_NAME}\Uninstall.lnk" "$INSTDIR\uninstall.exe"

    WriteUninstaller "$INSTDIR\uninstall.exe"

    WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT_NAME}"
    WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" "$INSTDIR\uninstall.exe"
    WriteRegStr HKLM "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${PRODUCT_EXE}"
    WriteRegStr HKLM "${UNINSTALL_KEY}" "Publisher" "${PRODUCT_PUBLISHER}"
    WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${PRODUCT_VERSION}"
    WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoModify" 1
    WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
    Delete "$INSTDIR\${PRODUCT_EXE}"
    Delete "$INSTDIR\LICENSE.txt"
    Delete "$INSTDIR\uninstall.exe"
    RMDir "$INSTDIR"

    Delete "$SMPROGRAMS\${PRODUCT_NAME}\${PRODUCT_NAME}.lnk"
    Delete "$SMPROGRAMS\${PRODUCT_NAME}\Uninstall.lnk"
    RMDir "$SMPROGRAMS\${PRODUCT_NAME}"
    Delete "$DESKTOP\${PRODUCT_NAME}.lnk"

    DeleteRegKey HKLM "${UNINSTALL_KEY}"
SectionEnd
