use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum QueueError {
    #[error("queue is full: {current_bytes} bytes pending, limit is {max_bytes} bytes")]
    Full { current_bytes: u64, max_bytes: u64 },

    #[error("queue directory {dir:?} is at an unsupported format version: found {found}, expected {expected}")]
    UnsupportedFormatVersion {
        dir: std::path::PathBuf,
        found: String,
        expected: String,
    },

    /// Another live `DurableQueue` handle (in this process or another) already
    /// holds `dir`'s lock file. Returned instead of silently proceeding —
    /// see `queue.rs`'s `open()` doc comment for why a second concurrent
    /// `open()` on the same directory is unsafe, not just discouraged.
    #[error("queue directory {dir:?} is already open by another DurableQueue handle")]
    AlreadyOpen { dir: std::path::PathBuf },

    #[error(transparent)]
    Io(#[from] io::Error),
}
