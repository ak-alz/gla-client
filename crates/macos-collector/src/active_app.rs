//! UNVERIFIED — see crate-level doc comment.

/// Returns the frontmost application's process name — never a window
/// title, matching the same privacy boundary already enforced on
/// Windows/Linux (only the narrow, consent-gated browser-title exception
/// reads any text beyond a process name, and this function is not that
/// exception).
///
/// Documented approach: `NSWorkspace.shared.frontmostApplication` →
/// `NSRunningApplication.localizedName` (or `.bundleIdentifier`, TBD
/// which reads more like `windows-collector`'s existing process-name
/// convention once this can actually be compared against real output).
/// Requires no permission — see
/// `docs/02_ARCHITECTURE/AGENT_MACOS_CAPABILITY_MATRIX.md`'s capability
/// table, first row, for why this is believed to need nothing beyond
/// the app simply running.
///
/// UNVERIFIED: never compiled against `objc2-app-kit`'s actual
/// `NSWorkspace`/`NSRunningApplication` bindings API surface.
pub fn frontmost_process_name() -> Option<String> {
    todo!(
        "NSWorkspace::sharedWorkspace().frontmostApplication() -> Option<Retained<NSRunningApplication>>, \
         then .localizedName() -> Option<Retained<NSString>> converted to a Rust String — \
         exact objc2-app-kit method names/signatures never checked against a compiler"
    )
}
