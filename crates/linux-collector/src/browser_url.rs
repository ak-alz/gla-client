//! Reads the active browser's address-bar text via AT-SPI2 (the Linux
//! accessibility bus screen readers use) — no browser extension required.
//! Same one-way discipline as `windows_collector::browser_url` (this
//! crate's Windows counterpart, built and empirically verified in the same
//! round this module was written): the raw text is handed straight to
//! `normalization::classify_url` (host-only, everything else discarded
//! immediately) and never returned, stored, or logged by any caller.
//!
//! # Verification status — READ BEFORE RELYING ON THIS
//!
//! UNVERIFIED against a real running browser. Written against the stable,
//! long-standing AT-SPI2 D-Bus interfaces (`org.a11y.atspi.Accessible`,
//! `org.a11y.atspi.Text`) using only their oldest, most basic
//! methods/properties — deliberately NOT the more elaborate `Collection`
//! interface's `MatchRule`-based queries, despite those being more
//! efficient (a single query instead of a manual tree walk), because this
//! author has materially higher confidence in the exact wire signatures of
//! the basic methods than the nested `MatchRule` struct layout, and a
//! working-but-slower implementation beats a fast one built on a guess.
//! The accessibility bus itself (`org.a11y.Bus`) and its address lookup
//! WERE confirmed real and reachable in the environment this was written
//! in (a WSLg session) — what's unverified is everything past that: the
//! actual application/Accessible tree shape of a real Chrome/Firefox
//! window, and whether the exact role/interface names below match its
//! real accessible tree (no GUI browser was installable in that
//! environment to check against). Same honesty bar as
//! `macos-collector`'s own crate doc comment for the same underlying
//! reason — no way to verify on hand for this platform in this round.
//!
//! # Design (mirrors `windows_collector::browser_url` exactly)
//!
//! 1. Resolve the accessibility bus address via the session bus's
//!    `org.a11y.Bus.GetAddress` method, then connect to it.
//! 2. Walk `org.a11y.atspi.Registry`'s root accessible's children to find
//!    the application whose name matches the active window's browser
//!    process (`chrome`/`firefox`/etc. — same `browser_process_names`
//!    gating as `collector.rs` already uses for the rest of this crate).
//! 3. Within that application's accessible tree, find a descendant whose
//!    role is "entry" (a text-entry control — AT-SPI's rough equivalent
//!    of Windows' `ControlType=Edit`) via a plain recursive
//!    `GetChildAtIndex`/`ChildCount` walk, bounded in depth to avoid a
//!    pathological full-tree scan on a heavily nested web page.
//! 4. Read its text via `org.a11y.atspi.Text.GetText(0, -1)`.

use normalization::UrlRules;
use std::collections::HashSet;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

// Address bars sit near the top of the accessible tree (toolbar, a few
// levels below the top-level window) — bounding the walk avoids wasting
// time descending into a deeply nested web page's own DOM-derived
// accessible tree, which a browser also exposes over AT-SPI.
const MAX_WALK_DEPTH: u32 = 6;
const ROLE_ENTRY: &str = "entry";

/// Owns the one D-Bus connection to the accessibility bus for the
/// collector's lifetime — created lazily on first use (mirrors
/// `windows_collector::browser_url::AddressBarReader`'s reasoning: no
/// point paying connection setup cost on every poll when nothing about it
/// changes).
pub struct AddressBarReader {
    connection: Option<Connection>,
    attempted_init: bool,
}

impl Default for AddressBarReader {
    fn default() -> Self {
        AddressBarReader {
            connection: None,
            attempted_init: false,
        }
    }
}

impl AddressBarReader {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_initialized(&mut self) {
        if self.attempted_init {
            return;
        }
        self.attempted_init = true;
        self.connection = Self::try_connect().ok();
    }

    fn try_connect() -> zbus::Result<Connection> {
        let session = Connection::session()?;
        let address: String = session
            .call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &())?
            .body()
            .deserialize()?;
        zbus::blocking::connection::Builder::address(address.as_str())?.build()
    }

    /// Reads the given browser process's address-bar text, or `None` on
    /// any failure — accessibility bus unavailable, application not found
    /// in the AT-SPI registry (e.g. the browser was launched before the
    /// bus came up, a real documented AT-SPI quirk), no entry-role
    /// descendant found within `MAX_WALK_DEPTH`, etc. Every failure
    /// collapses to "no signal this tick," matching every other collector
    /// signal in this codebase.
    pub fn read(&mut self, process_name: &str) -> Option<String> {
        self.ensure_initialized();
        let connection = self.connection.as_ref()?;
        let app_path = find_application_by_process_name(connection, process_name)?;
        let entry_path = find_entry_descendant(connection, &app_path, MAX_WALK_DEPTH)?;
        read_text(connection, &entry_path)
    }
}

fn find_application_by_process_name(connection: &Connection, process_name: &str) -> Option<OwnedObjectPath> {
    let root = OwnedObjectPath::try_from("/org/a11y/atspi/accessible/root").ok()?;
    let child_count: i32 = get_property(connection, &root, "org.a11y.atspi.Accessible", "ChildCount")?;
    let target_stem = process_name.trim_end_matches(".exe").to_lowercase();
    for i in 0..child_count {
        let child: (String, OwnedObjectPath) = connection
            .call_method(
                Some("org.a11y.atspi.Registry"),
                root.as_str(),
                Some("org.a11y.atspi.Accessible"),
                "GetChildAtIndex",
                &(i,),
            )
            .ok()?
            .body()
            .deserialize()
            .ok()?;
        let (_, child_path) = child;
        let name: String = get_property(connection, &child_path, "org.a11y.atspi.Accessible", "Name").unwrap_or_default();
        // Real browser AT-SPI application names are human-readable
        // ("Firefox", "Google Chrome"), not the raw process name — a
        // substring match against the process name's stem is the same
        // pragmatic, non-exact-match approach the rest of this feature
        // already uses (see `normalization::classify_url`'s own
        // substring-not-exact matching choice).
        if name.to_lowercase().contains(&target_stem) || target_stem.contains(&name.to_lowercase()) {
            return Some(child_path);
        }
    }
    None
}

fn find_entry_descendant(connection: &Connection, path: &OwnedObjectPath, remaining_depth: u32) -> Option<OwnedObjectPath> {
    if remaining_depth == 0 {
        return None;
    }
    let role: String = connection
        .call_method(
            Some("org.a11y.atspi.Registry"),
            path.as_str(),
            Some("org.a11y.atspi.Accessible"),
            "GetRoleName",
            &(),
        )
        .ok()?
        .body()
        .deserialize()
        .ok()?;
    if role.eq_ignore_ascii_case(ROLE_ENTRY) {
        return Some(path.clone());
    }
    let child_count: i32 = get_property(connection, path, "org.a11y.atspi.Accessible", "ChildCount").unwrap_or(0);
    for i in 0..child_count {
        let child: (String, OwnedObjectPath) = connection
            .call_method(
                Some("org.a11y.atspi.Registry"),
                path.as_str(),
                Some("org.a11y.atspi.Accessible"),
                "GetChildAtIndex",
                &(i,),
            )
            .ok()?
            .body()
            .deserialize()
            .ok()?;
        let (_, child_path) = child;
        if let Some(found) = find_entry_descendant(connection, &child_path, remaining_depth - 1) {
            return Some(found);
        }
    }
    None
}

fn read_text(connection: &Connection, path: &OwnedObjectPath) -> Option<String> {
    let text: String = connection
        .call_method(
            Some("org.a11y.atspi.Registry"),
            path.as_str(),
            Some("org.a11y.atspi.Text"),
            "GetText",
            &(0i32, -1i32),
        )
        .ok()?
        .body()
        .deserialize()
        .ok()?;
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn get_property<T: TryFrom<zbus::zvariant::OwnedValue>>(
    connection: &Connection,
    path: &OwnedObjectPath,
    interface: &str,
    property: &str,
) -> Option<T> {
    let value: zbus::zvariant::OwnedValue = connection
        .call_method(
            Some("org.a11y.atspi.Registry"),
            path.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(interface, property),
        )
        .ok()?
        .body()
        .deserialize()
        .ok()?;
    T::try_from(value).ok()
}

/// Same gating as `windows_collector::browser_url::should_classify_via_url`
/// — skip the (real D-Bus round trips, not free) read entirely when there
/// are no URL rules configured, or the active process isn't a known
/// browser.
pub fn should_classify_via_url(process_name: &str, browser_process_names: &HashSet<String>, rules: &UrlRules) -> bool {
    if rules.is_empty() {
        return false;
    }
    browser_process_names.contains(&process_name.to_lowercase())
}

/// Mirrors `windows_collector::browser_url::classify_browser_url`.
pub fn classify_browser_url(reader: &mut AddressBarReader, process_name: &str, rules: &UrlRules) -> Option<String> {
    let text = reader.read(process_name)?; // local variable, dropped at end of scope
    normalization::classify_url(Some(&text), rules)
}
