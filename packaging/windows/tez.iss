; tez for Windows: a per-user installer (no admin prompt) built with Inno Setup 6.
;
;   iscc /DAppVersion=0.2.7 /DOutputName=tez-v0.2.7-windows-x64-setup packaging\windows\tez.iss
;
; It puts tez.exe and tez-app.exe where tez keeps them anyway (%USERPROFILE%\.tez\bin, or
; TEZ_HOME\bin), runs `tez install` to set up PHP, the web server, services and PATH, and adds the
; app to the Start menu. Uninstalling removes tez's programs and keeps databases and settings.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef OutputName
  #define OutputName "tez-setup"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif

[Setup]
AppId={{29FC142D-1B5B-4A80-8A51-D24EDC404711}
AppName=tez
AppVersion={#AppVersion}
AppVerName=tez {#AppVersion}
AppPublisher=Amritpal Singh
AppPublisherURL=https://github.com/boparaiamrit/tez
AppSupportURL=https://github.com/boparaiamrit/tez/issues
AppUpdatesURL=https://github.com/boparaiamrit/tez/releases
VersionInfoVersion={#AppVersion}
VersionInfoDescription=tez setup
DefaultDirName={code:TezHome}\bin
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename={#OutputName}
SetupIconFile=..\..\app\assets\icon\tez.ico
UninstallDisplayIcon={app}\tez-app.exe
UninstallDisplayName=tez
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=force
RestartApplications=no
ChangesEnvironment=yes

[Messages]
WelcomeLabel2=This installs tez {#AppVersion}: fast, native Laravel development with every PHP and Node.js version, databases, Horizon and local HTTPS.%n%nNo administrator rights are needed.

[Tasks]
Name: "setup"; Description: "Set up PHP, the web server and services now (downloads about 300 MB)"
Name: "desktopicon"; Description: "Add a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#SourceDir}\tez.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\tez-app.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\tez"; Filename: "{app}\tez-app.exe"; Comment: "Fast, native Laravel development"
Name: "{autodesktop}\tez"; Filename: "{app}\tez-app.exe"; Comment: "Fast, native Laravel development"; Tasks: desktopicon

[Run]
; A console window shows the downloads; it stays open if something fails, so the message can be read.
Filename: "{cmd}"; Parameters: "/c """"{app}\tez.exe"" install || pause"""; StatusMsg: "Setting up PHP, the web server and services. This can take a few minutes..."; Tasks: setup; Flags: waituntilterminated
Filename: "{cmd}"; Parameters: "/c """"{app}\tez.exe"" start && ""{app}\tez.exe"" open"""; Description: "Start tez and open the app"; Flags: postinstall nowait runhidden skipifsilent

[UninstallRun]
Filename: "{app}\tez.exe"; Parameters: "stop"; Flags: runhidden waituntilterminated; RunOnceId: "StopTez"

[UninstallDelete]
; The commands `tez install` added beside tez.exe.
Type: files; Name: "{app}\php.exe"
Type: files; Name: "{app}\composer.exe"
Type: files; Name: "{app}\composer.phar"
Type: files; Name: "{app}\exec.exe"
Type: files; Name: "{app}\node.exe"
Type: files; Name: "{app}\npm.exe"
Type: files; Name: "{app}\npx.exe"
Type: files; Name: "{app}\pnpm.exe"
Type: files; Name: "{app}\yarn.exe"
Type: files; Name: "{app}\corepack.exe"
Type: files; Name: "{app}\*.old-*"

[Code]
// tez's own folder: TEZ_HOME when it is set, else %USERPROFILE%\.tez (as tez itself decides).
function TezHome(Param: String): String;
begin
  Result := GetEnv('TEZ_HOME');
  if Result = '' then
    Result := ExpandConstant('{%USERPROFILE}') + '\.tez';
end;

// A running tez holds its own files open: stop it before they are replaced.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  if FileExists(ExpandConstant('{app}\tez.exe')) then
    Exec(ExpandConstant('{app}\tez.exe'), 'stop', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    MsgBox('tez is removed. Your databases, PHP versions and settings are still in ' +
      TezHome('') + '. Delete that folder to remove them too.', mbInformation, MB_OK);
end;
