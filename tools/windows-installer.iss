#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef Architecture
  #error Architecture is required
#endif
#ifndef Payload
  #error Payload is required
#endif

[Setup]
AppId={{027DF18D-B408-45D3-A18C-5B6AD2EE546A}
AppName=Listenbox
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}.0
AppPublisher=Listenbox
AppPublisherURL=https://listenbox.app/
AppSupportURL=https://github.com/listenbox/client/issues
AppUpdatesURL=https://github.com/listenbox/client/releases
DefaultDirName={localappdata}\Programs\Listenbox
DefaultGroupName=Listenbox
PrivilegesRequired=lowest
DisableDirPage=yes
DisableProgramGroupPage=yes
#if Architecture == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
MinVersion=10.0.22000
AppMutex=Local\ListenboxDesktop
CloseApplications=no
RestartApplications=no
AllowCancelDuringInstall=yes
UninstallDisplayIcon={app}\listenbox-desktop.exe
OutputDir=..\crates\desktop\dist\installers
OutputBaseFilename=Listenbox-{#AppVersion}-windows-{#Architecture}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Files]
Source: "{#Payload}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Listenbox"; Filename: "{app}\listenbox-desktop.exe"

[Run]
Filename: "{app}\listenbox-desktop.exe"; Description: "Open Listenbox"; Flags: nowait postinstall skipifsilent
Filename: "{app}\listenbox-desktop.exe"; Flags: nowait; Check: IsUpgrade

[Code]
function IsUpgrade: Boolean;
begin
  Result := WizardSilent;
end;

function InitializeSetup: Boolean;
var
  Deadline: Cardinal;
begin
  Deadline := GetTickCount + 60000;
  while CheckForMutexes('Local\ListenboxDesktop') and (GetTickCount < Deadline) do
    Sleep(100);
  Result := not CheckForMutexes('Local\ListenboxDesktop');
  if not Result then
    MsgBox('Listenbox is still finishing current work. Please let it quit safely, then run this installer again.', mbError, MB_OK);
end;
