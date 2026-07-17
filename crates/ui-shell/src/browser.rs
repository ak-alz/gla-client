//! Opens a URL in the user's default system browser — deliberately NOT an
//! embedded webview (Tauri or otherwise). ADR 0013 permits a lazily-created
//! Tauri window for this, but this crate makes a more conservative choice:
//! shelling out to the OS's own "open URL" mechanism means this agent
//! process never spawns a WebView2/Chromium process tree at all, for any
//! reason — the ~353 MB regression ADR 0013 documents for a Tauri window
//! (even hidden) simply cannot happen here, by construction, not by
//! discipline. The already-existing React/MUI dashboard
//! (`frontend/`) still gets reused as-is, satisfying ADR 0013's actual
//! goal ("avoid a second native-toolkit reimplementation of that UI") —
//! only the mechanism for showing it differs from what the ADR discussed.
//! Documented here, not silently substituted, precisely because it's a
//! deviation from what that ADR's text explicitly named.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum OpenUrlError {
    #[error("failed to open {url:?} in the default browser: {source}")]
    Failed { url: String, source: std::io::Error },
}

pub fn open_url(url: &str) -> Result<(), OpenUrlError> {
    open::that(url).map_err(|source| OpenUrlError::Failed {
        url: url.to_string(),
        source,
    })
}
