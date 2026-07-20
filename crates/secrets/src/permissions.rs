//! "Local DB permissions" — restricting the secret-bearing files/dirs
//! to the current user only. Unix: a direct `chmod`-equivalent syscall
//! (`std::fs::set_permissions`), no shelling out needed. Windows: no
//! Rust std API exists for ACL manipulation, and hand-rolled Win32
//! `SECURITY_DESCRIPTOR`/ACL FFI is real, unsafe, security-sensitive
//! code that's easy to get subtly wrong — `icacls.exe` is Microsoft's
//! own, already-hardened tool for exactly this operation, and is
//! present on every supported Windows version by default; shelling out
//! to it is the more trustworthy choice here than reimplementing ACL
//! manipulation from scratch.

use std::io;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum PermissionError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[cfg(windows)]
    #[error("icacls failed with exit status {0:?}: {1}")]
    IcaclsFailed(Option<i32>, String),
    #[cfg(windows)]
    #[error("USERNAME environment variable is not set")]
    NoUsername,
}

/// Restricts `path` (a file OR a directory) so only the current OS
/// user can read/write it.
#[cfg(unix)]
pub fn restrict_to_current_user_only(path: &Path) -> Result<(), PermissionError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if path.is_dir() { 0o700 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Restricts `path` (a file OR a directory) so only the current OS
/// user can read/write it, via `icacls /inheritance:r /grant:r
/// <user>:F` — removes inherited permissions entirely, then grants
/// full control to (and only to) the current user.
#[cfg(windows)]
pub fn restrict_to_current_user_only(path: &Path) -> Result<(), PermissionError> {
    let username = std::env::var("USERNAME").map_err(|_| PermissionError::NoUsername)?;
    let grant = format!("{username}:(OI)(CI)F");

    let output = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(&grant)
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        Err(PermissionError::IcaclsFailed(
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "secrets-permissions-test-{name}-{}",
            std::process::id()
        ))
    }

    #[cfg(unix)]
    #[test]
    fn a_restricted_file_has_exactly_owner_read_write_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("file");
        std::fs::write(&path, b"secret").unwrap();

        restrict_to_current_user_only(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "expected exactly rw-------, got {:o}",
            mode & 0o777
        );

        std::fs::remove_file(&path).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_restricted_directory_has_exactly_owner_read_write_execute_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("dir");
        std::fs::create_dir_all(&path).unwrap();

        restrict_to_current_user_only(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "expected exactly rwx------, got {:o}",
            mode & 0o777
        );

        std::fs::remove_dir_all(&path).ok();
    }

    #[cfg(windows)]
    #[test]
    fn a_restricted_file_denies_the_everyone_group_per_icacls() {
        let path = temp_path("file.txt");
        std::fs::write(&path, b"secret").unwrap();

        restrict_to_current_user_only(&path).unwrap();

        // Real verification via icacls itself, not just "the command
        // exited 0" — reads back the ACL and confirms BUILTIN\Users /
        // Everyone are no longer listed as having any access, and the
        // current user is.
        let output = std::process::Command::new("icacls")
            .arg(&path)
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&output.stdout);
        assert!(
            !listing.contains("BUILTIN\\Users") && !listing.contains("Everyone"),
            "expected no generic-group ACE after restriction, got: {listing}"
        );
        let username = std::env::var("USERNAME").unwrap();
        assert!(
            listing.contains(&username),
            "expected the current user to still have access, got: {listing}"
        );

        std::fs::remove_file(&path).ok();
    }
}
