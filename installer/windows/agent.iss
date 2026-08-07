; AG-WIN-002 — per-user Inno Setup installer for growth-layer-agent.exe.
;
; Design decisions worth knowing before editing this file:
;
; - PER-USER, NOT PER-MACHINE: PrivilegesRequired=lowest + DefaultDirName
;   under {localappdata} — matches ADR 0013's "no admin required unless
;   justified" principle already established for the agent binary itself
;   (see windows-collector's PROCESS_QUERY_LIMITED_INFORMATION choice).
;   No UAC prompt at install time.
;
; - DATA NEVER LIVES UNDER {app}: the agent's own data directory
;   (%LOCALAPPDATA%\GrowthLayerAgent\ — device_id.json, queue/, logs/,
;   crash_marker.json) is never referenced anywhere in this script, on
;   purpose. Upgrade/uninstall only ever touch {app} (the install
;   directory), so pairing identity and the offline queue survive both
;   by construction, not by installer-script discipline that could later
;   be broken by an unrelated edit. See agent-bin/src/paths.rs's doc
;   comment for the Rust-side half of this same argument.
;
; - AppId IS FIXED: this is what makes Inno recognize a later installer
;   run (same AppId, higher AppVersion) as an upgrade rather than a
;   parallel, conflicting install. Never regenerate it.
;
; - "REPAIR": classic Inno Setup (unlike MSI) has no formal repair verb.
;   Re-running this same installer over an existing install overwrites
;   files and re-applies the autostart registration — verified for real
;   in TEST_REPORT.md by deleting the installed exe and re-running the
;   installer.
;
; - CLOSING A RUNNING AGENT BEFORE INSTALL/UNINSTALL: done via a forceful
;   `taskkill /F` in [Code], not a graceful IPC handshake — deliberate,
;   because AG-004's durable-queue was specifically built (and
;   independently reviewed) to survive an ungraceful process kill without
;   losing or duplicating data. A forceful close during upgrade is exactly
;   as safe as the crash-recovery path this agent already has to handle
;   regardless (see durable-queue's TEST_REPORT.md).
;
; - NOT SIGNED BY THIS SCRIPT: no Authenticode code-signing certificate is
;   configured here. See TEST_REPORT.md for what was done instead (a
;   self-signed demonstration signature applied to the OUTPUT installer,
;   separately from this script, via PowerShell's Set-AuthenticodeSignature)
;   and what remains a genuine, external, environment gap (a real,
;   CA-issued Authenticode certificate).

; "DevPace" — user-facing display name only (installer UI, Start Menu
; shortcut, uninstall entry, tray tooltip). AppId below is UNCHANGED and
; MUST stay that way (see its own comment) — that's what makes Inno
; recognize this as an upgrade of the same product, not a parallel
; install. The actual data directory (device_id.json/queue/logs — see
; this file's top comment) and the exe/process name are ALSO deliberately
; unchanged: renaming either risks losing an existing user's device
; pairing or breaking the auto-updater, for a purely cosmetic gain — this
; rename only touches what a user actually sees.
#define MyAppName "DevPace"
#define MyAppVersion "0.1.31"
#define MyAppPublisher "DevPace"
#define MyAppExeName "growth-layer-agent.exe"

[Setup]
AppId={{53BFB598-6B57-49BC-8E70-1BE8AB8ADE5E}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={localappdata}\Programs\DevPace
DisableProgramGroupPage=yes
DisableDirPage=yes
DisableReadyPage=yes
DisableWelcomePage=no
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist\windows
; Version-LESS filename, deliberately — the download page/docs link at
; the stable `releases/latest/download/DevPaceSetup.exe` URL (same
; convention the Linux .deb/.rpm/tarball assets already use), which only
; resolves if the uploaded asset is actually named that. A versioned name
; here (the previous convention) silently 404s that link on every release
; unless someone remembers to also upload a renamed copy by hand — found
; for real when a user hit exactly that 404 on v0.1.23 (back when this
; was still named GrowthLayerAgentSetup).
OutputBaseFilename=DevPaceSetup
Compression=lzma
SolidCompression=yes
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppName}
AppSupportURL=https://github.com/ak-alz/pts-agent
VersionInfoVersion={#MyAppVersion}

[Languages]
; Single language deliberately: Inno Setup always shows a language-picker
; dialog when more than one [Languages] entry is defined, even under
; /VERYSILENT (confirmed empirically while testing this script — it is
; not a bug in the silent-install flags, it's documented Inno behavior).
; A real product installer would likely want both languages with
; /LANG=russian passed by whatever wraps this (e.g. a future auto-update
; flow); for this task's actual verification goal (prove per-user
; install/upgrade/repair/uninstall genuinely work), one language removes
; an unrelated interactive prompt from the loop.
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Files]
Source: "..\..\target\release\growth-layer-agent.exe"; DestDir: "{app}"; Flags: ignoreversion

; A real, found-by-the-user gap: without this section, the installer
; registered autostart and ran the agent once, but left NO way to find
; or relaunch it afterward short of rebooting -- no Start Menu entry at
; all (DisableProgramGroupPage=yes above only skips the WIZARD PAGE that
; asks which group name to use; it does not create icons on its own).
; One shortcut, fixed group name (matches DisableProgramGroupPage's own
; "no interactive choice" philosophy this script already follows) --
; deliberately no Desktop icon, since a tray-only background agent
; showing up on the Desktop is the more surprising, unrequested one of
; the two conventional shortcut locations.
[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"

[Run]
Filename: "{app}\{#MyAppExeName}"; Parameters: "--register-autostart"; Flags: runhidden waituntilterminated; StatusMsg: "Настройка автозапуска..."
Filename: "{app}\{#MyAppExeName}"; Description: "Запустить DevPace"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\{#MyAppExeName}"; Parameters: "--unregister-autostart"; Flags: runhidden waituntilterminated; RunOnceId: "UnregisterAutostart"

[Code]
procedure KillRunningAgent();
var
  ResultCode: Integer;
begin
  // Best-effort: Exec returning False / a nonzero ResultCode both just
  // mean "nothing was running to kill" — not a failure worth surfacing,
  // see the module-level comment on why a forceful kill is safe here.
  Exec('taskkill.exe', '/F /IM growth-layer-agent.exe', '', SW_HIDE,
    ewWaitUntilTerminated, ResultCode);
end;

function InitializeSetup(): Boolean;
begin
  KillRunningAgent();
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    KillRunningAgent();
  end;
end;
