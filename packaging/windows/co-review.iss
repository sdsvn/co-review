; Co-Review's Windows installer (Inno Setup 6), built by packaging/package.sh:
;   iscc /DVersion=<version> /DSource=<staged Co-Review folder> /DOutput=<dir> /DIcon=<ico> co-review.iss
; Installs per user (no admin rights) into %LOCALAPPDATA%\Programs\Co-Review, where the `co-review` command looks for
; the app, and puts the command's folder (resources\app\bin) on the user's PATH. A running Co-Review is closed first.

#ifndef Version
  #define Version "0.0.0"
#endif

[Setup]
AppId={{6F1D8C3A-4E2B-4B7A-9C0D-2A5E8F7B1C34}
AppName=Co-Review
AppVersion={#Version}
AppVerName=Co-Review {#Version}
AppPublisher=Co-Review contributors
AppPublisherURL=https://sdsvn.github.io/co-review/
AppSupportURL=https://github.com/sdsvn/co-review/issues
DefaultDirName={localappdata}\Programs\Co-Review
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#Output}
OutputBaseFilename=Co-Review-win-x64
SetupIconFile={#Icon}
UninstallDisplayIcon={app}\Co-Review.exe
UninstallDisplayName=Co-Review
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
CloseApplications=force
RestartApplications=no

[InstallDelete]
; An update replaces the bundled files instead of leaving old ones behind.
Type: filesandordirs; Name: "{app}\resources"

[Files]
Source: "{#Source}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{autoprograms}\Co-Review"; Filename: "{app}\Co-Review.exe"

[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}\resources\app\bin"; Check: NeedsAddPath(ExpandConstant('{app}\resources\app\bin'))

[Run]
Filename: "{app}\Co-Review.exe"; Description: "Open Co-Review"; Flags: nowait postinstall skipifsilent

[Code]
function NeedsAddPath(Dir: string): Boolean;
var
  Path: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
    Result := True
  else
    Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Path, Dir: string;
  Index: Integer;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
    exit;
  Dir := ExpandConstant('{app}\resources\app\bin');
  Index := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';');
  if Index = 0 then
    exit;
  Delete(Path, Index, Length(Dir) + 1);
  if (Length(Path) > 0) and (Path[Length(Path)] = ';') then
    Delete(Path, Length(Path), 1);
  RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path);
end;
