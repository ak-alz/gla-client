//! UNVERIFIED — see crate-level doc comment.

use objc2_app_kit::NSWorkspace;

/// Returns the frontmost application's process name — never a window
/// title, matching the same privacy boundary already enforced on
/// Windows/Linux (only the narrow, consent-gated browser-title exception
/// reads any text beyond a process name, and this function is not that
/// exception).
///
/// `NSRunningApplication.localizedName` (not `.bundleIdentifier`) —
/// chosen to match `windows-collector`'s existing process-name
/// convention (a human-readable app name like "Safari", not a reverse-DNS
/// identifier like "com.apple.Safari").
///
/// Requires no permission — per Apple's documentation, `NSWorkspace`'s
/// frontmost-application query is plain, unrestricted app introspection,
/// unlike `input_counter.rs`'s event tap.
///
/// UNVERIFIED: written against the real, cached `objc2-app-kit` 0.3.2
/// source (`NSWorkspace::sharedWorkspace()`, `.frontmostApplication()`,
/// `NSRunningApplication::localizedName()`, all confirmed to exist with
/// these exact signatures) — but never compiled or linked on a real Mac.
pub fn frontmost_process_name() -> Option<String> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let name = app.localizedName()?;
    Some(name.to_string())
}
