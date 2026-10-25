#ifndef AppVersion
  #error AppVersion must be supplied by the release build
#endif
#ifndef SourceDir
  #error SourceDir must be supplied by the release build
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

#define AppName "OrviScript"
#define AppPublisher "OrviScript"
#define AppURL "https://orviscript.com"
#define ReleaseURL "https://github.com/xMatis/orviscript/releases"

[Setup]
AppId={{A97F3974-48EB-43EF-9575-7B5BC797EAB4}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
AppUpdatesURL={#ReleaseURL}
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\OrviScript
DefaultGroupName=OrviScript
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=orviscript-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
ChangesEnvironment=yes
ChangesAssociations=yes
UsePreviousAppDir=yes
UsePreviousTasks=yes
UninstallDisplayIcon={app}\assets\orviscript-file-icon.ico
#ifdef SignToolName
SignTool={#SignToolName}
SignedUninstaller=yes
#endif

[Tasks]
Name: "vscode"; Description: "Install or update the OrviScript extension for Visual Studio Code"; GroupDescription: "Editor integration:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\orviscript.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\orvi-lsp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\orviscript-vscode.vsix"; DestDir: "{app}\editors"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\orviscript-file-icon.ico"; DestDir: "{app}\assets"; Flags: ignoreversion

[Registry]
Root: HKCU; Subkey: "Software\Classes\.orvi"; ValueType: string; ValueName: ""; ValueData: "OrviScript.Source"; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\OrviScript.Source"; ValueType: string; ValueName: ""; ValueData: "OrviScript source file"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\OrviScript.Source\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\assets\orviscript-file-icon.ico,0"
Root: HKCU; Subkey: "Software\Classes\OrviScript.Source\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{sys}\notepad.exe"" ""%1"""

[Run]
Filename: "{code:GetVSCodeCommand}"; Parameters: "--install-extension ""{app}\editors\orviscript-vscode.vsix"" --force"; Description: "Install the OrviScript VS Code extension"; Flags: postinstall skipifsilent runhidden; Tasks: vscode; Check: VSCodeAvailable

[Code]
function NormalizePathPart(Value: String): String;
begin
  Value := Lowercase(Trim(Value));
  while (Length(Value) > 0) and (Value[Length(Value)] = '\') do
    Delete(Value, Length(Value), 1);
  Result := Value;
end;

function PathContains(CurrentPath, Candidate: String): Boolean;
var
  Item: String;
  Separator: Integer;
begin
  Result := False;
  Candidate := NormalizePathPart(Candidate);
  repeat
    Separator := Pos(';', CurrentPath);
    if Separator = 0 then begin
      Item := CurrentPath;
      CurrentPath := '';
    end else begin
      Item := Copy(CurrentPath, 1, Separator - 1);
      Delete(CurrentPath, 1, Separator);
    end;
    if NormalizePathPart(Item) = Candidate then begin
      Result := True;
      Exit;
    end;
  until CurrentPath = '';
end;

procedure AddInstallDirToPath;
var
  CurrentPath: String;
  InstallDir: String;
begin
  InstallDir := ExpandConstant('{app}');
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', CurrentPath) then
    CurrentPath := '';
  if not PathContains(CurrentPath, InstallDir) then begin
    if (CurrentPath <> '') and (CurrentPath[Length(CurrentPath)] <> ';') then
      CurrentPath := CurrentPath + ';';
    RegWriteExpandStringValue(HKCU, 'Environment', 'Path', CurrentPath + InstallDir);
  end;
end;

procedure RemoveInstallDirFromPath;
var
  CurrentPath: String;
  InstallDir: String;
  Item: String;
  NewPath: String;
  Separator: Integer;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', CurrentPath) then
    Exit;
  InstallDir := NormalizePathPart(ExpandConstant('{app}'));
  NewPath := '';
  repeat
    Separator := Pos(';', CurrentPath);
    if Separator = 0 then begin
      Item := CurrentPath;
      CurrentPath := '';
    end else begin
      Item := Copy(CurrentPath, 1, Separator - 1);
      Delete(CurrentPath, 1, Separator);
    end;
    if (Trim(Item) <> '') and (NormalizePathPart(Item) <> InstallDir) then begin
      if NewPath <> '' then NewPath := NewPath + ';';
      NewPath := NewPath + Trim(Item);
    end;
  until CurrentPath = '';
  RegWriteExpandStringValue(HKCU, 'Environment', 'Path', NewPath);
end;

function GetVSCodeCommand(Param: String): String;
var
  Candidate: String;
begin
  Candidate := ExpandConstant('{localappdata}\Programs\Microsoft VS Code\bin\code.cmd');
  if FileExists(Candidate) then begin
    Result := Candidate;
    Exit;
  end;
  Candidate := ExpandConstant('{commonpf64}\Microsoft VS Code\bin\code.cmd');
  if FileExists(Candidate) then begin
    Result := Candidate;
    Exit;
  end;
  Result := '';
end;

function VSCodeAvailable: Boolean;
begin
  Result := GetVSCodeCommand('') <> '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    AddInstallDirToPath;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveInstallDirFromPath;
end;
