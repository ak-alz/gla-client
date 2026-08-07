//! Installs an already-verified `AvailableUpdate` — real user request:
//! "Приходится руками что-то ставить" (одно-клик из трея). Deliberately
//! does NOT use `updater::staging`/`health` (the raw rename-the-running-
//! binary + supervised-restart path) — that crate's own module doc
//! comment is explicit that wiring it up means designing a whole
//! old-process/new-process health-signal protocol from scratch, real
//! extra engineering this round doesn't need. Instead: hand off to the
//! platform's OWN already-tested installer/install.sh — the exact same
//! mechanism every manual update already goes through, just automated:
//!
//! - **Windows**: download the signed installer artifact, verify its
//!   checksum, launch it with Inno Setup's silent flags. The installer
//!   ALREADY closes the running agent (`KillRunningAgent` in
//!   `agent.iss`'s `[Code]`, forceful `taskkill /F`, verified safe by
//!   `durable-queue`'s own crash-tolerance work) and re-registers
//!   autostart. Its `[Run]` "launch now" step has `Flags: ...
//!   skipifsilent`, so a silent install does NOT relaunch the agent on
//!   its own — this module's `cmd`-based babysitter does that part.
//! - **Linux, tarball installs only**: download the tarball, extract,
//!   run the bundled `install.sh` (idempotent — designed to be re-run
//!   for exactly this purpose already, see its own comments), whose
//!   last step (`systemctl --user restart`) is what replaces the
//!   running process. `.deb`/`.rpm`/`.PKGBUILD` installs (root-owned
//!   `/usr/bin`) return `Unsupported` — this tray app has no business
//!   prompting for `sudo`, those users keep using their package
//!   manager (or reinstall the new deb/rpm by hand — no different from
//!   today).
//! - **macOS**: `Unsupported` — no packaged artifact ships for this
//!   platform yet (see `manifest_url`'s own doc comment upstream).
//!
//! The self-kill problem: on Windows, `taskkill /F /IM growth-layer-
//! agent.exe` matches THIS process too (it launched the installer),
//! so whatever waits for the installer to finish and then relaunches
//! the app must NOT be this process — it would be killed mid-wait. A
//! detached `cmd.exe /C "installer.exe /VERYSILENT ... && start ...
//! app.exe"` survives that (separate process tree, different image
//! name), so THIS process spawns it and exits immediately rather than
//! waiting around to be killed. Linux doesn't have this problem the
//! same way — a spawned child is reparented to init/systemd on parent
//! exit, not killed, so no babysitter trick is needed there; `bash
//! install.sh` is simply spawned detached and this process exits.

use crate::update_check::AvailableUpdate;
use std::path::PathBuf;
use updater::{disk_space, download_with_checksum, DownloadConfig, UreqDownloadTransport};

/// Comfortably above the real artifact sizes (~3 MB installer, ~2 MB
/// tarball) — a fixed round number is simpler than plumbing the
/// manifest's exact byte count through, and the margin is what
/// actually matters for "will this download even fit," not precision.
const MIN_FREE_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug)]
pub enum ApplyUpdateError {
    InsufficientDiskSpace,
    Download(String),
    Spawn(String),
    /// This install method (system package manager, or a platform with
    /// no unattended path yet) isn't handled by this module — the
    /// caller should fall back to opening release notes instead, the
    /// same way it already does when no update is known yet.
    Unsupported,
}

impl std::fmt::Display for ApplyUpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientDiskSpace => write!(f, "not enough free disk space to download the update"),
            Self::Download(msg) => write!(f, "download failed: {msg}"),
            Self::Spawn(msg) => write!(f, "could not launch the installer: {msg}"),
            Self::Unsupported => write!(f, "one-click install isn't available for this install method"),
        }
    }
}

fn download_artifact(update: &AvailableUpdate, dest: &PathBuf) -> Result<(), ApplyUpdateError> {
    if !disk_space::has_enough_free_space(&std::env::temp_dir(), MIN_FREE_BYTES).unwrap_or(false) {
        return Err(ApplyUpdateError::InsufficientDiskSpace);
    }
    let transport = UreqDownloadTransport::default();
    download_with_checksum(&transport, &update.artifact_url, &update.artifact_sha256, dest, &DownloadConfig::default())
        .map_err(|err| ApplyUpdateError::Download(err.to_string()))
}

#[cfg(target_os = "windows")]
pub fn apply(update: &AvailableUpdate) -> Result<(), ApplyUpdateError> {
    use std::os::windows::process::CommandExt;

    let file_name = update.artifact_url.rsplit('/').next().unwrap_or("DevPaceSetup.exe");
    let installer_path = std::env::temp_dir().join(file_name);
    download_artifact(update, &installer_path)?;

    let app_exe = std::env::current_exe().map_err(|e| ApplyUpdateError::Spawn(e.to_string()))?;

    // A real TEMP .bat FILE, not a `cmd /C "<inline line with && and
    // nested quotes>"` one-liner — found by actually running this live
    // against a real installed agent: `cmd /C` with `.args()` double-
    // escapes the nested quotes (cmd then reports "is not recognized as
    // an internal or external command"), and `.raw_arg()` avoids THAT
    // but a `&&`-chained, multiply-quoted one-liner passed as a single
    // `/C` argument still hit cmd's OWN "Синтаксическая ошибка в имени
    // файла..." parser quirk — a real, reproduced failure, not a
    // hypothetical one. Writing the exact same two lines to a .bat file
    // and running `cmd /C <path to .bat>` (a single plain path, no
    // embedded quoting at all) verified working end to end: real
    // download, real silent install, real relaunch.
    //
    // See this module's doc comment for why this MUST be a separate,
    // detached process rather than something this process waits on.
    let batch_path = std::env::temp_dir().join("gla-update-apply.bat");
    let batch_contents = format!(
        "\"{}\" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART\r\nstart \"\" \"{}\"\r\n",
        installer_path.display(),
        app_exe.display(),
    );
    std::fs::write(&batch_path, batch_contents).map_err(|e| ApplyUpdateError::Spawn(e.to_string()))?;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    std::process::Command::new("cmd")
        .args(["/C", &batch_path.to_string_lossy()])
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map_err(|e| ApplyUpdateError::Spawn(e.to_string()))?;

    std::process::exit(0);
}

#[cfg(target_os = "linux")]
pub fn apply(update: &AvailableUpdate) -> Result<(), ApplyUpdateError> {
    // Only a tarball install (entirely under $HOME, no root involved)
    // is safe to self-update here — a .deb/.rpm/PKGBUILD install owns
    // /usr/bin and updating it needs a privilege prompt this
    // background tray app has no business making.
    let exe = std::env::current_exe().map_err(|e| ApplyUpdateError::Spawn(e.to_string()))?;
    let home = std::env::var("HOME").map_err(|_| ApplyUpdateError::Unsupported)?;
    if !exe.starts_with(&home) {
        return Err(ApplyUpdateError::Unsupported);
    }

    let file_name = update.artifact_url.rsplit('/').next().unwrap_or("growth-layer-agent-linux-x86_64.tar.gz");
    let tarball_path = std::env::temp_dir().join(file_name);
    download_artifact(update, &tarball_path)?;

    let extract_dir = std::env::temp_dir().join(format!("gla-update-{}", update.version));
    std::fs::create_dir_all(&extract_dir).map_err(|e| ApplyUpdateError::Spawn(e.to_string()))?;
    let status = std::process::Command::new("tar")
        .arg("xzf")
        .arg(&tarball_path)
        .arg("--strip-components=1")
        .arg("-C")
        .arg(&extract_dir)
        .status()
        .map_err(|e| ApplyUpdateError::Spawn(e.to_string()))?;
    if !status.success() {
        return Err(ApplyUpdateError::Spawn("tar extraction failed".to_string()));
    }

    // install.sh's own last step (`systemctl --user restart`) is what
    // replaces this running process — spawned detached (not waited on)
    // so it isn't torn down along with whatever it eventually restarts;
    // on Linux a spawned child simply reparents to init on this
    // process's exit, no babysitter trick needed (see module doc).
    std::process::Command::new("bash")
        .arg(extract_dir.join("install.sh"))
        .current_dir(&extract_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| ApplyUpdateError::Spawn(e.to_string()))?;

    std::process::exit(0);
}

#[cfg(target_os = "macos")]
pub fn apply(_update: &AvailableUpdate) -> Result<(), ApplyUpdateError> {
    Err(ApplyUpdateError::Unsupported)
}
