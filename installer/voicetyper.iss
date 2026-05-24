; VoiceTyper installer (Inno Setup 6).
;
; Per-user install: NO admin / UAC prompt. Wraps the single self-contained release
; exe (target\release\voicetyper.exe) into a Setup.exe with a Start Menu shortcut and
; an uninstall entry. No autostart (deliberate — the user launches it when they want).
;
; Build:
;   1. cargo build --release
;   2. iscc installer\voicetyper.iss
;   -> installer\Output\VoiceTyper-Setup-<version>.exe

#define MyAppName "VoiceTyper"
; CI overrides this with the git tag via /DMyAppVersion=x.y.z; this is the local default.
#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif
#define MyAppPublisher "gluonhiggs"
#define MyAppExeName "voicetyper.exe"
#define MyAppURL "https://github.com/gluonhiggs/Articulate"

[Setup]
; Fixed AppId so version upgrades and uninstall track the SAME app. Never change it.
AppId={{8F4A2C7E-3B1D-4E6A-9C2F-1A5B7D9E0C34}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
; Per-user install location (no admin): C:\Users\<you>\AppData\Local\Programs\VoiceTyper
DefaultDirName={localappdata}\Programs\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
; A running tray app locks the exe — let Setup close it first on upgrade/reinstall.
CloseApplications=yes
RestartApplications=no
OutputDir=Output
OutputBaseFilename=VoiceTyper-Setup-{#MyAppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName} now"; Flags: nowait postinstall skipifsilent

; Note: the uninstaller removes the app exe + shortcuts but NOT
; %APPDATA%\VoiceTyper\config.toml, so your API key + settings survive a reinstall.
