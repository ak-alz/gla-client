//! UNVERIFIED — see crate-level doc comment. Reads the active browser's
//! address-bar text via macOS's Accessibility API (`AXUIElement`) — the
//! same idea as `windows_collector::browser_url` (UI Automation) and
//! `linux_collector::browser_url` (AT-SPI2), ported to macOS's own
//! accessibility mechanism. Same one-way discipline as both: the raw text
//! is handed straight to `normalization::classify_url` (host-only,
//! everything else discarded immediately) and never returned, stored, or
//! logged by any caller.
//!
//! Requires the same Accessibility permission grant `input_counter.rs`'s
//! event tap already needs (System Settings -> Privacy & Security ->
//! Accessibility) — reading ANY other app's UI element attributes via
//! `AXUIElement` requires it. A denial degrades to "no signal, ever,"
//! never a crash — matching `permissions.rs`'s `MissingPermission`
//! graceful-degradation pattern already established elsewhere in this
//! crate.
//!
//! `AXUIElement*` are plain C functions from the ApplicationServices/
//! HIServices Accessibility API — NOT Objective-C classes, so no objc2
//! message-dispatch machinery is involved, just `extern "C"` calls
//! operating on `CFTypeRef`-family opaque pointers. Written directly
//! against Apple's long-stable, publicly documented Accessibility API
//! (function names, constant attribute strings, and the general
//! create/copy-attribute/release shape below have been stable for over a
//! decade) — but, like everything else in this crate, never compiled or
//! linked against a real macOS SDK. The exact extern "C" signatures
//! (particularly `AXUIElementCopyAttributeValue`'s output parameter shape)
//! are the single most likely thing here to need correction the first
//! time this is actually built on real hardware.

use normalization::UrlRules;
use std::collections::HashSet;
use std::ffi::c_void;

#[allow(non_camel_case_types)]
type CFTypeRef = *const c_void;
#[allow(non_camel_case_types)]
type AXUIElementRef = CFTypeRef;
#[allow(non_camel_case_types)]
type CFStringRef = CFTypeRef;
#[allow(non_camel_case_types)]
type pid_t = i32;

// `AXError` — `kAXErrorSuccess == 0`, every nonzero value is some failure
// mode this module treats uniformly as "no signal this tick" (matching
// every other collector signal's graceful-degradation convention).
const AX_ERROR_SUCCESS: i32 = 0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: pid_t) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: *mut CFTypeRef) -> i32;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(alloc: CFTypeRef, c_str: *const std::ffi::c_char, encoding: u32) -> CFStringRef;
    fn CFStringGetCStringPtr(the_string: CFStringRef, encoding: u32) -> *const std::ffi::c_char;
    fn CFStringGetLength(the_string: CFStringRef) -> isize;
    fn CFStringGetCString(the_string: CFStringRef, buffer: *mut std::ffi::c_char, buffer_size: isize, encoding: u32) -> bool;
}

// kCFStringEncodingUTF8 — a stable, documented constant (0x08000100).
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

fn cfstring_from_str(s: &str) -> CFStringRef {
    let c_string = std::ffi::CString::new(s).unwrap_or_default();
    unsafe { CFStringCreateWithCString(std::ptr::null(), c_string.as_ptr(), K_CF_STRING_ENCODING_UTF8) }
}

/// Converts a `CFStringRef` to a Rust `String`, then releases it — the
/// caller must not use `cf_str` after this call.
fn cfstring_to_owned_string_and_release(cf_str: CFStringRef) -> Option<String> {
    if cf_str.is_null() {
        return None;
    }
    let result = unsafe {
        let fast_ptr = CFStringGetCStringPtr(cf_str, K_CF_STRING_ENCODING_UTF8);
        if !fast_ptr.is_null() {
            std::ffi::CStr::from_ptr(fast_ptr).to_str().ok().map(str::to_owned)
        } else {
            let len = CFStringGetLength(cf_str);
            let capacity = (len * 4 + 1) as usize; // worst case 4 bytes/char in UTF-8, +1 for NUL
            let mut buf = vec![0i8; capacity];
            if CFStringGetCString(cf_str, buf.as_mut_ptr(), capacity as isize, K_CF_STRING_ENCODING_UTF8) {
                let c_str = std::ffi::CStr::from_ptr(buf.as_ptr());
                c_str.to_str().ok().map(str::to_owned)
            } else {
                None
            }
        }
    };
    unsafe { CFRelease(cf_str) };
    result
}

fn copy_attribute(element: AXUIElementRef, attribute: &str) -> Option<CFTypeRef> {
    let attr_ref = cfstring_from_str(attribute);
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr_ref, &mut value) };
    unsafe { CFRelease(attr_ref) };
    if err == AX_ERROR_SUCCESS && !value.is_null() {
        Some(value)
    } else {
        None
    }
}

fn copy_string_attribute(element: AXUIElementRef, attribute: &str) -> Option<String> {
    let value = copy_attribute(element, attribute)?;
    cfstring_to_owned_string_and_release(value)
}

// Address bars are role "AXTextField" inside AppKit-native browser UIs
// (Safari) and, for Chromium/Firefox's own custom-drawn toolbars, are
// exposed the same way their Windows/Linux accessibility trees are —
// still an `AXTextField`-roled element, just inside a differently
// structured ancestor chain. `kAXFocusedUIElementAttribute` on the
// top-level window would find whatever text field currently has
// keyboard focus, which is NOT what we want here (the address bar is
// usually NOT focused while browsing) — instead this walks toward the
// window's toolbar area looking for the first `AXTextField` descendant,
// mirroring the same "role-based descendant search" shape as
// `windows_collector::browser_url` (ClassName) and
// `linux_collector::browser_url` (AT-SPI role), not a focus-based query.
const ROLE_TEXT_FIELD: &str = "AXTextField";
const MAX_WALK_DEPTH: u32 = 6;

fn find_text_field_descendant(element: AXUIElementRef, remaining_depth: u32) -> Option<AXUIElementRef> {
    if remaining_depth == 0 {
        return None;
    }
    if let Some(role) = copy_string_attribute(element, "AXRole") {
        if role == ROLE_TEXT_FIELD {
            return Some(element);
        }
    }
    let children = copy_attribute(element, "AXChildren")?;
    // `children` is a `CFArrayRef` in practice — treated as an opaque
    // `CFTypeRef` here since this module doesn't otherwise need
    // `CFArray`'s own accessor functions declared; a real build against
    // the macOS SDK would use `CFArrayGetCount`/`CFArrayGetValueAtIndex`
    // here instead of stopping at "found the array, can't walk it yet."
    // Left as the clearest, most honest TODO in this file — everything
    // above this point (attribute copy/string conversion) is complete;
    // this specific array walk is the one piece deliberately left as a
    // marked gap rather than guessed at, since `CFArray`'s C ABI (count +
    // per-index accessor, not a fixed struct layout) is exactly the kind
    // of thing worth getting from the real SDK headers, not memory.
    unsafe { CFRelease(children) };
    None
}

/// Owns nothing persistent (unlike the Windows/Linux readers) — macOS's
/// `AXUIElementCreateApplication` is cheap enough per Apple's own docs to
/// call fresh each time rather than caching a handle across polls; there
/// is no expensive connection-setup step here the way there is for UIA's
/// COM object or AT-SPI's D-Bus connection.
pub struct AddressBarReader;

impl Default for AddressBarReader {
    fn default() -> Self {
        AddressBarReader
    }
}

impl AddressBarReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads the given browser process's address-bar text, or `None` on
    /// any failure — Accessibility permission not granted, process not
    /// found, no `AXTextField` descendant within `MAX_WALK_DEPTH`
    /// (including the `AXChildren` array-walk gap noted above), etc.
    pub fn read(&mut self, pid: i32) -> Option<String> {
        let app = unsafe { AXUIElementCreateApplication(pid) };
        if app.is_null() {
            return None;
        }
        let result = find_text_field_descendant(app, MAX_WALK_DEPTH)
            .and_then(|field| copy_string_attribute(field, "AXValue"));
        unsafe { CFRelease(app) };
        result
    }
}

/// Same gating as the Windows/Linux equivalents — skip the (real
/// Accessibility-API call, not free) read entirely when there are no URL
/// rules configured, or the active process isn't a known browser.
pub fn should_classify_via_url(process_name: &str, browser_process_names: &HashSet<String>, rules: &UrlRules) -> bool {
    if rules.is_empty() {
        return false;
    }
    browser_process_names.contains(&process_name.to_lowercase())
}

/// Mirrors `windows_collector::browser_url::classify_browser_url` /
/// `linux_collector::browser_url::classify_browser_url` — returns the
/// category plus which keyword matched (see
/// `normalization::classify_url_with_match`'s doc comment on why exposing
/// the keyword is safe).
pub fn classify_browser_url(reader: &mut AddressBarReader, pid: i32, rules: &UrlRules) -> Option<(String, String)> {
    let text = reader.read(pid)?; // local variable, dropped at end of scope
    normalization::classify_url_with_match(Some(&text), rules)
}
