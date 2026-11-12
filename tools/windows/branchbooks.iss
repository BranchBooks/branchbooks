; The Windows installer (Inno Setup 7): per user, no admin prompt, into %LOCALAPPDATA%\Programs.
; tools\windows\build.ps1 -Installer builds the app and then this:
;   ISCC.exe /DAppVersion=0.2.0 [/DChannel=dev] [/DSourceExe=...] [/DOutputDir=...] branchbooks.iss
; Channel "dev" is a second app beside the release: its own name, app id, exe, folders and books.
; Uninstalling asks whether to delete the books too (No by default); a silent uninstall never does.

#ifndef AppVersion
  #error Pass the version: /DAppVersion=x.y.z
#endif
#ifndef Channel
  #define Channel ""
#endif
#ifndef SourceExe
  #define SourceExe "..\..\dist\windows\BranchBooks.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist\windows"
#endif

#if Channel == "dev"
  #define AppName "BranchBooks Dev"
  #define AppGuid "9A165151-F5C6-48BD-9F1A-861214B6BB7E"
  ; WebView2's data and the crash log go in %LOCALAPPDATA%\<exe name>, so this keeps them apart too.
  #define ExeStem "BranchBooksDev"
  ; The same folder a debug build uses (src/store.rs, data_dir).
  #define DataFolder "BranchBooks-dev"
  #define SetupName "BranchBooks-Dev-setup"
#elif Channel == ""
  #define AppName "BranchBooks"
  #define AppGuid "C836F050-E256-44FB-A54D-54E71728CD23"
  #define ExeStem "BranchBooks"
  #define DataFolder "BranchBooks Data"
  #define SetupName "BranchBooks-setup"
#else
  #error Channel is "" (the release) or "dev"
#endif

[Setup]
AppId={{{#AppGuid}}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=BranchBooks
AppPublisherURL=https://branchbooks.dev
AppSupportURL=https://gitlab.com/branchbooks/branchbooks/-/issues
; Per user: {autopf} is %LOCALAPPDATA%\Programs. An update finds the folder by AppId.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DisableDirPage=auto
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename={#SetupName}
SetupIconFile=..\..\crates\branchbooks-ui\icons\icon.ico
UninstallDisplayIcon={app}\{#ExeStem}.exe
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; An update closes the running app (Restart Manager); the [Run] entry starts the new one.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "{#ExeStem}.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#ExeStem}.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#ExeStem}.exe"; Tasks: desktopicon

[Run]
; Also after a silent update (the app runs the installer with /VERYSILENT), to reopen it. A
; postinstall entry runs as the person, not elevated.
Filename: "{app}\{#ExeStem}.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall

[UninstallDelete]
; WebView2's cache and the crash log: the app's, not the person's.
Type: filesandordirs; Name: "{localappdata}\{#ExeStem}"

[Code]
const
  WebView2 = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';

{ Microsoft Edge WebView2 Runtime, which the app is drawn with: on every Windows 11, and on Windows 10
  through Edge's updates. }
function HasWebView2: Boolean;
var
  Version: String;
begin
  Result := False;
  if RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\' + WebView2, 'pv', Version) then
    Result := (Version <> '') and (Version <> '0.0.0.0');
  if not Result and RegQueryStringValue(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\' + WebView2, 'pv', Version) then
    Result := (Version <> '') and (Version <> '0.0.0.0');
end;

{ Elevated, Setup could be running as another account (over-the-shoulder elevation) and install into
  that account's profile, and the uninstaller would then ask for admin: refuse, as VS Code's user
  installer warns. shortcut: with UAC turned off every admin runs elevated and is refused too; offer a
  machine-wide installer if that comes up. }
function InitializeSetup: Boolean;
begin
  Result := False;
  if IsAdmin then begin
    SuppressibleMsgBox('{#AppName} installs for you alone and doesn''t need administrator rights.'#13#10#13#10 +
      'Run the setup again without "Run as administrator".', mbError, MB_OK, IDOK);
    Exit;
  end;
  if not HasWebView2 then begin
    SuppressibleMsgBox('{#AppName} needs the Microsoft Edge WebView2 Runtime, which isn''t on this computer.'#13#10#13#10 +
      'Install it from https://go.microsoft.com/fwlink/p/?LinkId=2124703, then run this setup again.', mbError, MB_OK, IDOK);
    Exit;
  end;
  Result := True;
end;

{ The person's books and settings, kept unless they say otherwise. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data, Old, Folders: String;
begin
  if (CurUninstallStep <> usPostUninstall) or UninstallSilent then
    Exit;
  Data := ExpandConstant('{localappdata}\{#DataFolder}');
  Folders := '';
  if DirExists(Data) then
    Folders := Data;
#if Channel == ""
  { Before the move out of the roaming profile; still here only if it could not be moved. }
  Old := ExpandConstant('{userappdata}\BranchBooks');
  if DirExists(Old) then
    Folders := Folders + #13#10 + Old;
#endif
  if Folders = '' then
    Exit;
  if MsgBox('Also delete your books and settings?'#13#10#13#10 +
      'Books you sync to a remote are still there, but books only on this computer will be gone for good.'#13#10#13#10 +
      Trim(Folders), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then begin
    DelTree(Data, True, True, True);
#if Channel == ""
    DelTree(Old, True, True, True);
#endif
  end;
end;
