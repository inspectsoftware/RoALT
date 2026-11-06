; AltDock for Windows: a per-user install of the app, no admin rights.
; Build it with scripts\build-installer.ps1, which passes the version from
; src-tauri\tauri.conf.json and where cargo put the release build.

#if Ver < EncodeVer(6,6,0)
  #error Inno Setup 6.6 or newer is needed (WizardStyle dynamic windows11)
#endif
#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z, or build with scripts\build-installer.ps1
#endif
; VersionInfoVersion must be numeric; a prerelease such as 0.2.0-beta.1 passes 0.2.0 here.
#ifndef AppNumVersion
  #define AppNumVersion AppVersion
#endif
#ifndef AppExe
  #define AppExe "..\src-tauri\target\release\altdock.exe"
#endif

[Setup]
; Its own AppId, not RoALT's: neither installer touches the other app.
AppId={{65C8A8C4-9C0A-4218-946C-850017EBB62E}
AppName=AltDock
AppVersion={#AppVersion}
AppPublisher=AltDock contributors
VersionInfoVersion={#AppNumVersion}
; Per-user: {autopf} is %LOCALAPPDATA%\Programs.
DefaultDirName={autopf}\AltDock
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 1809, the oldest Windows Tauri 2 supports.
MinVersion=10.0.17763
WizardStyle=modern dynamic windows11
SetupIconFile=..\src-tauri\icons\icon.ico
UninstallDisplayIcon={app}\AltDock.exe
UninstallDisplayName=AltDock
; Restart Manager closes a running AltDock before its exe is replaced.
CloseApplications=force
OutputDir=..\dist-installer
OutputBaseFilename=AltDock-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#AppExe}"; DestDir: "{app}"; DestName: "AltDock.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\AltDock"; Filename: "{app}\AltDock.exe"
Name: "{autodesktop}\AltDock"; Filename: "{app}\AltDock.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\AltDock.exe"; Description: "{cm:LaunchProgram,AltDock}"; Flags: nowait postinstall skipifsilent; Check: WebView2Installed
; AltDock's self-updater (disabled for now) runs Setup with /VERYSILENT ... /RELAUNCH and quits: start it again.
Filename: "{app}\AltDock.exe"; Flags: nowait; Check: Relaunch

[Code]
const
  WebView2Key = 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  { Microsoft's Evergreen Bootstrapper: Setup only opens the link, it downloads nothing itself. }
  WebView2Download = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703';
  { tauri-plugin-single-instance holds <identifier>-sim while AltDock runs. }
  RunningMutex = 'dev.altdock.app-sim';

function HasWebView2(Root: Integer): Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(Root, WebView2Key, 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0');
end;

{ The machine-wide runtime (HKLM32 is WOW6432Node) or a per-user one. }
function WebView2Installed: Boolean;
begin
  Result := HasWebView2(HKLM32) or HasWebView2(HKCU);
end;

function InitializeSetup: Boolean;
var
  Code: Integer;
begin
  if not WebView2Installed and (SuppressibleMsgBox(
      'AltDock draws its window with the Microsoft Edge WebView2 Runtime, which is not installed.' #13#10#13#10 +
      'Setup will continue, but AltDock will not open until the runtime is installed.' #13#10#13#10 +
      'Download Microsoft''s WebView2 installer in your browser now?',
      mbConfirmation, MB_YESNO, IDNO) = IDYES) then
    ShellExecAsOriginalUser('open', WebView2Download, '', '', SW_SHOWNORMAL, ewNoWait, Code);
  Result := True;
end;

{ Setup was started by AltDock's self-updater (/RELAUNCH), silently. }
function Relaunch: Boolean;
var
  I: Integer;
begin
  Result := False;
  if WizardSilent and WebView2Installed then
    for I := 1 to ParamCount do
      if CompareText(ParamStr(I), '/RELAUNCH') = 0 then
        Result := True;
end;

{ A running AltDock holds its exe and database open: uninstalling then would leave both behind. }
function InitializeUninstall: Boolean;
begin
  Result := True;
  while Result and CheckForMutexes(RunningMutex) do
    Result := SuppressibleMsgBox('AltDock is running. Close it, then click OK.', mbError, MB_OKCANCEL, IDCANCEL) = IDOK;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data: String;
begin
  Data := ExpandConstant('{localappdata}\AltDock');
  if (CurUninstallStep <> usPostUninstall) or UninstallSilent or not DirExists(Data) then
    exit;
  { Keep is the first button, so the default: TaskDialogMsgBox has no default-button flag. }
  if TaskDialogMsgBox('Also delete your AltDock accounts and settings?',
      'They are in ' + Data + '. Your Roblox accounts themselves are not affected.',
      mbConfirmation, MB_YESNO, ['&Keep them' #13#10 'They are there again if you reinstall AltDock.',
      '&Delete them' #13#10 'Saved accounts, cookies, groups and settings. This cannot be undone.'], 0) = IDNO then begin
    DelTree(Data, True, True, True);
    { The window position tauri-plugin-window-state keeps. }
    DelTree(ExpandConstant('{userappdata}\dev.altdock.app'), True, True, True);
  end;
end;
