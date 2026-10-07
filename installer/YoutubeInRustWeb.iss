; YoutubeInRustWeb installer (Inno Setup 6).
; Build:   "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" installer\YoutubeInRustWeb.iss
; Output:  installer\Output\YoutubeInRustWeb-Setup.exe

#define AppName "YoutubeInRustWeb"
#define AppVersion "0.5.1"
#define AppExe "YoutubeInRustWeb.exe"

[Setup]
AppId={{6F4C2B1E-8D3A-4E7B-9C5F-2A1D0E9B7C43}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=BetoCW
AppPublisherURL=https://github.com/BetoCW/MusicLowCost
AppComments=Lightweight native YouTube Music player (Rust)
; Per-user install: no administrator rights needed.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir=Output
OutputBaseFilename={#AppName}-Setup
SetupIconFile=..\icons\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; If the app is running it is closed so it can be updated.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "README.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
; Actualizacion desde el boton UpDate (instalador en silencio): se vuelve a abrir sola.
Filename: "{app}\{#AppExe}"; Flags: nowait; Check: WizardSilent

[UninstallDelete]
; Components downloaded by the app (yt-dlp / deno). Settings and sessions are kept.
Type: filesandordirs; Name: "{userappdata}\{#AppName}\bin"
Type: filesandordirs; Name: "{userappdata}\{#AppName}\login-webview"
