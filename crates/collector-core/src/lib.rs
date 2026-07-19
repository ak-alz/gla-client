//! Hoisted out of `windows-collector` (AG-WIN-001) now that a second
//! platform (`linux-collector`, AG-LNX-002) needs the identical contract
//! — `windows-collector`'s own doc comment named this exact trigger
//! ("if/when AG-LNX-001 or AG-MAC-001 need the same trait, hoisting it
//! into a shared crate at that point is a rename plus a `pub use`, not a
//! redesign") and this crate is exactly that, done now that the second
//! implementation genuinely exists rather than speculatively.
//!
//! Mirrors `core/interfaces.py::SignalCollector`/`RawSignalSnapshot` —
//! the Python source's own architectural boundary ("core/ не знает
//! НИЧЕГО про Windows/macOS/Linux API. Он работает только с этим
//! интерфейсом") is restored here across the Rust rewrite exactly as it
//! existed in Python: one shared contract, N platform implementations,
//! not N independent copies of the same shape.

/// What one `poll()` returns — mirrors `core/interfaces.py::RawSignalSnapshot`
/// field-for-field. Never carries a window title or any input value, only
/// counts/flags/the process name — the same architectural privacy
/// boundary as the Python source, on every platform.
#[derive(Debug, Clone)]
pub struct RawSignalSnapshot {
    pub active_process_name: Option<String>,
    pub keyboard_events: i64,
    pub mouse_move_events: i64,
    pub mouse_click_events: i64,
    pub is_idle: bool,
    pub idle_seconds: f64,
    pub category_override: Option<String>,
}

/// The platform-agnostic contract every native collector implements.
/// `Error` is an associated type (not a single shared enum) deliberately
/// — each platform's failure modes are genuinely different (a Win32 hook
/// installation failure has nothing in common with an X11 connection
/// failure), and forcing them into one shared enum would either grow an
/// ever-widening cross-platform enum or erase the specific error
/// information each platform's caller actually wants.
pub trait SignalCollector {
    type Error: std::error::Error;

    fn start(&mut self) -> Result<(), Self::Error>;
    fn stop(&mut self);
    fn poll(&mut self) -> RawSignalSnapshot;
}
