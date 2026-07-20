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

/// Checks whether Accessibility permission is currently granted, WITHOUT
/// prompting the user — `AXIsProcessTrusted()` per Apple's documented
/// behavior. Safe to poll repeatedly (e.g. before deciding whether to
/// even attempt `input_counter::start()`).
///
/// UNVERIFIED: never compiled against the real `ApplicationServices`/
/// `HIServices` framework this function would need to link against (the
/// `objc2-*` crate family this crate depends on focuses on Objective-C
/// class bindings; `AXIsProcessTrusted` is a plain C function from an
/// older framework and would need its own `extern "C"` declaration —
/// not yet written here, deliberately, rather than guessing at a linker
/// incantation with zero way to verify it).
pub fn accessibility_trusted() -> bool {
    todo!(
        "call the C function AXIsProcessTrusted() from ApplicationServices/HIServices; \
         requires an extern \"C\" declaration and linking against the framework — \
         never attempted without a real macOS toolchain to verify against"
    )
}
