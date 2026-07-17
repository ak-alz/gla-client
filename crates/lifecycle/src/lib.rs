//! Service lifecycle and autostart (AG-008): single-instance enforcement,
//! start-on-login, crash-vs-clean-quit detection, a pure sleep/wake +
//! session lock/unlock + logoff state machine, crash-restart registration,
//! and a rotating event log for all of the above. Windows-first — see
//! each module's doc comment for what is and isn't implemented on other
//! platforms, and TEST_REPORT.md for what was verified by simulation
//! rather than by actually suspending/locking/logging out of the machine
//! this session runs on.

mod autostart;
mod crash_detection;
mod crash_restart;
mod power_events;
mod rotating_log;
mod single_instance;

pub use autostart::{Autostart, AutostartError};
pub use crash_detection::CrashMarker;
pub use crash_restart::{register_for_crash_restart, RestartError};
pub use power_events::{LifecycleAction, LifecycleState, PowerEvent};
pub use rotating_log::RotatingLog;
pub use single_instance::{acquire, SingleInstanceError, SingleInstanceGuard};
