; A per-user installer delegates binary-pair replacement to the verified package script.
#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef PackageDir
  #error PackageDir is required
#endif
[Setup]
AppId={{C034A574-D363-4550-8575-A3D21A9E9F88}
AppName=Clumsies CLI
AppVersion={#AppVersion}
AppPublisher=Clumsies
DefaultDirName={localappdata}\Programs\ClumsiesCLI
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=clumsies-cli-{#AppVersion}-windows-x86_64-Setup
Compression=lzma2
SolidCompression=yes
CloseApplications=no
RestartApplications=no
DisableProgramGroupPage=yes
WizardStyle=modern
UninstallDisplayName=Clumsies CLI

[Files]
Source: "{#PackageDir}\*"; DestDir: "{tmp}\clumsies-cli-package"; Flags: ignoreversion recursesubdirs createallsubdirs deleteafterinstall
Source: "{#PackageDir}\install.ps1"; DestDir: "{app}"; Flags: ignoreversion

[Code]
procedure RunPackageScript(Source: String; Uninstall: Boolean);
var
  Params: String;
  ResultCode: Integer;
begin
  Params := '-NoProfile -ExecutionPolicy Bypass -File "' + Source + '\install.ps1" -InstallRoot "' + ExpandConstant('{app}') + '"';
  if Uninstall then
    Params := Params + ' -Uninstall'
  else
    Params := Params + ' -Source "' + Source + '" -AddToPath';
  if not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'), Params, '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    RaiseException('Unable to run the Clumsies CLI installer script.');
  if ResultCode <> 0 then
    RaiseException('Clumsies CLI installation failed. Close active Clumsies MCP connections and retry. Existing programs and local drafts were retained.');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    RunPackageScript(ExpandConstant('{tmp}\clumsies-cli-package'), False);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RunPackageScript(ExpandConstant('{app}'), True);
end;
