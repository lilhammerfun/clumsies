; Build with assemble-windows.ps1. Keep AppId stable across upgrades.
[Setup]
AppId=ai.clumsies.desktop
AppName=Clumsies
AppVersion={#AppVersion}
AppPublisher=Clumsies
AppPublisherURL=https://clumsies.ai
DefaultDirName={localappdata}\Programs\Clumsies
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
DisableProgramGroupPage=yes
OutputBaseFilename=Clumsies-{#AppVersion}-windows-x86_64-Setup
SetupIconFile={#PackageDir}\icons\clumsies.ico
UninstallDisplayIcon={app}\icons\clumsies.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; The client must save its documents itself before it exits.
CloseApplications=no
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#PackageDir}\clumsies-desktop.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\clumsiesd.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\icons\*"; DestDir: "{app}\icons"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Clumsies"; Filename: "{app}\clumsies-desktop.exe"; IconFilename: "{app}\icons\clumsies.ico"
Name: "{autodesktop}\Clumsies"; Filename: "{app}\clumsies-desktop.exe"; IconFilename: "{app}\icons\clumsies.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\clumsies-desktop.exe"; Description: "Launch Clumsies"; Flags: nowait postinstall skipifsilent

[Code]
function PrepareInstalledApp(StopEngine: Boolean): String;
var
  Locator, Services, Processes, Process: Variant;
  I: Integer;
begin
  Result := '';
  try
    Locator := CreateOleObject('WbemScripting.SWbemLocator');
    Services := Locator.ConnectServer('', 'root\CIMV2');
    Processes := Services.ExecQuery(
      'SELECT * FROM Win32_Process WHERE Name = ''clumsies-desktop.exe'' OR Name = ''clumsiesd.exe''');
    for I := 0 to Processes.Count - 1 do begin
      Process := Processes.ItemIndex(I);
      if VarIsNull(Process.ExecutablePath) then
        Continue;
      if CompareText(Process.ExecutablePath, ExpandConstant('{app}\clumsies-desktop.exe')) = 0 then begin
        Result := 'Please save your work and close Clumsies, then try again.';
        Exit;
      end;
    end;
    if not StopEngine then
      Exit;
    // The resident engine survives closing the UI. Stop only this installation's
    // engine; never another user's, a portable copy's, or a development daemon.
    for I := 0 to Processes.Count - 1 do begin
      Process := Processes.ItemIndex(I);
      if VarIsNull(Process.ExecutablePath) then
        Continue;
      if CompareText(Process.ExecutablePath, ExpandConstant('{app}\clumsiesd.exe')) = 0 then
        if Process.Terminate(0) <> 0 then
          Result := 'Could not stop the Clumsies engine. Close its Agent sessions and try again.';
    end;
  except
    Result := 'Could not check running Clumsies processes: ' + GetExceptionMessage;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := PrepareInstalledApp(True);
end;

function InitializeUninstall: Boolean;
var
  Error: String;
begin
  Error := PrepareInstalledApp(False);
  Result := Error = '';
  if not Result then
    SuppressibleMsgBox(Error, mbError, MB_OK, IDOK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Error: String;
begin
  if CurUninstallStep = usUninstall then begin
    Error := PrepareInstalledApp(True);
    if Error <> '' then
      RaiseException(Error);
  end;
end;
