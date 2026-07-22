//! UNVERIFIED — see crate-level doc comment. Mirrors `linux-collector`'s
//! `environment::UnsupportedReason` honesty pattern: a capability that
//! isn't available right now returns a specific, typed reason, never a
//! silent `None`/guessed value.

/// Why a capability that COULD exist on this OS isn't available right
/// now — always a specific reason, per the same "honest gap, not a
/// silent guess" principle already applied on Linux
/// (`linux_collector::environment::UnsupportedReason`) and Windows
/// (explicit `CollectorError` variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingPermission {
    /// `AXIsProcessTrusted()` returned `false` — the input-count
    /// capability (`input_counter.rs`, which needs `CGEventTapCreate`)
    /// is unavailable until the user grants Accessibility permission in
    /// System Settings. Per this task's own scoping ("input aggregate
    /// only if permitted and necessary"), the caller should treat this
    /// as an expected, common state — not retry/nag, and not attempt any
    /// privilege-escalation workaround (there is none that would be
    /// appropriate here, matching this project's "no admin/root
    /// required unless justified" principle applied to macOS's TCC
    /// model).
    AccessibilityNotGranted,
}

// `AXIsProcessTrusted` is a plain C function from the older Carbon-era
// Accessibility API (`ApplicationServices` -> `HIServices` ->
// `Accessibility.h`), not wrapped by any objc2-* binding crate this
// project depends on (confirmed by grepping the actual cached
// objc2-app-kit/objc2-core-graphics/objc2-core-foundation 0.3.2 sources
// for the symbol — it genuinely isn't there, this isn't a case of not
// looking hard enough) — hence the raw `extern "C"` declaration below,
// exactly as this file originally anticipated. `#[link(name =
// "ApplicationServices", kind = "framework")]` is the standard, well-
// established way to link a macOS system framework directly from an
// extern block without a build.rs. No `#[cfg(target_os = "macos")]`
// needed here — this whole module is only ever compiled on macOS in the
// first place (see `lib.rs`'s `#[cfg(target_os = "macos")] mod permissions;`).
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    /// Returns a C `Boolean` (`unsigned char`, 0 or 1) — never a real
    /// Rust `bool` at the FFI boundary, per Apple's own C signature.
    fn AXIsProcessTrusted() -> u8;
}

/// Checks whether Accessibility permission is currently granted, WITHOUT
/// prompting the user — `AXIsProcessTrusted()` per Apple's documented
/// behavior. Safe to poll repeatedly (e.g. before deciding whether to
/// even attempt `input_counter::start()`).
///
/// UNVERIFIED: written against the real, cached `objc2-*` 0.3.2 sources
/// this crate depends on (confirmed those crates do NOT expose this
/// particular function, hence the raw `extern "C"` block above) — but
/// never compiled or linked on a real Mac. The `#[link(...)]` framework
/// name and the C function's exact ABI are believed correct (this is
/// Apple's own long-documented, stable API), not guessed at random.
pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}
