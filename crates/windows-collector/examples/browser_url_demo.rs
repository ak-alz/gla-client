//! Real, live verification for the URL-rules feature — no mocking. Run with:
//!
//!   cargo run -p windows-collector --example browser_url_demo
//!
//! Requires a real Chrome/Edge/Firefox window with a YouTube tab open and
//! IN THE FOREGROUND (this reads whatever's actually focused, exactly like
//! production) — proves the full real pipeline end to end: real UIA
//! address-bar read (`browser_url.rs`) -> real host extraction and keyword
//! match (`normalization::classify_url`) -> `RawSignalSnapshot.category_override`,
//! through the actual `WindowsSignalCollector`, not a standalone PoC.

use std::collections::HashSet;
use windows_collector::{RawSignalSnapshot, SignalCollector, WindowsSignalCollector};

fn print_snapshot(label: &str, snap: &RawSignalSnapshot) {
    println!(
        "[{label}] process={:?} category_override={:?}",
        snap.active_process_name, snap.category_override
    );
}

fn main() {
    println!("=== windows-collector URL-rules real live verification ===");
    println!("(bring a Chrome/Edge/Firefox window with a youtube.com tab to the foreground now)");
    std::thread::sleep(std::time::Duration::from_secs(5));

    let browsers: HashSet<String> = ["chrome.exe", "msedge.exe", "firefox.exe"]
        .into_iter()
        .map(String::from)
        .collect();
    let mut collector = WindowsSignalCollector::new(120.0, browsers, Vec::new());
    collector.set_browser_url_rules(vec![("rest".to_string(), vec!["youtube".to_string()])]);
    collector.start().expect("real hook installation must succeed");

    // Two polls: UIA/Firefox's a11y engine can be cold on the very first
    // query (see browser_url.rs's doc comment) — a second poll a moment
    // later should succeed regardless of engine.
    print_snapshot("poll 1", &collector.poll());
    std::thread::sleep(std::time::Duration::from_millis(800));
    let second = collector.poll();
    print_snapshot("poll 2", &second);

    if second.category_override.as_deref() == Some("rest") {
        println!("\n✅ SUCCESS: real address-bar read -> real host extraction -> real rule match, end to end.");
    } else {
        println!("\n❌ Did not classify as expected — check that a youtube.com tab is really foreground.");
    }

    collector.stop();
}
