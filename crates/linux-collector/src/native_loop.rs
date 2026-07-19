//! Real D-Bus registration for session/power notifications — the
//! Linux equivalent of `windows_collector::native_loop`, translating
//! into the SAME `lifecycle::PowerEvent` type (not a parallel Linux-only
//! enum) so `lifecycle::power_events::LifecycleState` is reused as-is
//! across platforms, exactly like the Windows backend already does.
//!
//! Uses `org.freedesktop.login1` (systemd-logind) — confirmed
//! DE-agnostic during AG-LNX-001's research: `Manager.PrepareForSleep`
//! (suspend/resume, one signal with a boolean argument, unlike Windows'
//! two separate `PBT_*` codes) and `Session.Lock`/`Session.Unlock`.
//! `Manager.PrepareForShutdown(true)` maps to `EndSession` (logind has
//! no separate vetoable "query" phase the way Windows'
//! `WM_QUERYENDSESSION` does — and `LifecycleState::handle` treats
//! `QueryEndSession`/`EndSession` identically regardless, so this loses
//! no real behavior); `PrepareForShutdown(false)` (a previously
//! announced shutdown being cancelled) has no corresponding
//! `PowerEvent` and is intentionally a no-op.
//!
//! # What can and cannot be verified for real in this session
//!
//! Real, live-verified in this crate's own tests (see below): the
//! `Session.Lock`/`Session.Unlock` signals, triggered for real via
//! `loginctl lock-session`/`unlock-session` against the WSL2
//! environment's real, running systemd-logind. NOT triggered for real:
//! `PrepareForSleep`/`PrepareForShutdown` — this session runs on the
//! user's live, interactive machine (via WSL) and actually suspending or
//! shutting it down to test this would be exactly the kind of
//! disruptive action already ruled out for the identical reason on
//! Windows (AG-008/AG-WIN-001) — verified instead via direct code
//! review of the (structurally identical) translation `match`, which
//! the Lock/Unlock live test already exercises end-to-end for the exact
//! same connection/AddMatch/MessageIterator/translation pipeline.

use lifecycle::PowerEvent;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;
use thiserror::Error;
use zbus::blocking::Connection;

#[derive(Debug, Error)]
pub enum NativeLoopError {
    #[error("failed to connect to the D-Bus system bus: {0}")]
    Connect(#[from] zbus::Error),
    #[error("the loop thread ended before reporting readiness")]
    ThreadDiedBeforeReady,
}

const WAKE_INTERFACE: &str = "com.growthlayer.agent.LinuxCollectorInternal";

/// The wake signal's member name must be unique PER `NativeLoop`
/// INSTANCE, not a shared constant — a real bug this crate's own tests
/// caught: D-Bus signals are broadcast to every connection with a
/// matching `AddMatch` rule, so with a shared member name, one
/// `NativeLoop`'s `stop()` would also wake (and prematurely terminate)
/// every OTHER live `NativeLoop`'s reader thread on the same bus,
/// including ones in a completely different test or, in production, a
/// hypothetical second instance in the same process. Confirmed via
/// revert-and-confirm: reverting to a shared constant reproduces
/// `native_loop::tests::real_session_lock_and_unlock_round_trip` failing
/// intermittently with "Disconnected" whenever it runs concurrently with
/// `stop_returns_promptly_without_a_real_logind_event`/`stop_is_idempotent`
/// in the same `cargo test` process (the same class of test-isolation
/// bug already hit in `lifecycle::autostart`'s Windows tests, AG-008).
fn wake_member() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "StopRequested{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

pub struct NativeLoop {
    thread: Option<JoinHandle<()>>,
    wake_conn: Arc<Connection>,
    wake_member: String,
}

impl NativeLoop {
    pub fn start() -> Result<(Self, Receiver<PowerEvent>), NativeLoopError> {
        let conn = Connection::system()?;
        let wake_conn = Arc::new(conn.clone());
        let wake_member = wake_member();

        add_match(
            &conn,
            "type='signal',interface='org.freedesktop.login1.Manager',member='PrepareForSleep'",
        )?;
        add_match(
            &conn,
            "type='signal',interface='org.freedesktop.login1.Manager',member='PrepareForShutdown'",
        )?;
        add_match(
            &conn,
            "type='signal',interface='org.freedesktop.login1.Session',member='Lock'",
        )?;
        add_match(
            &conn,
            "type='signal',interface='org.freedesktop.login1.Session',member='Unlock'",
        )?;
        add_match(
            &conn,
            &format!("type='signal',interface='{WAKE_INTERFACE}',member='{wake_member}'"),
        )?;

        let (notify_tx, notify_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread_wake_member = wake_member.clone();
        let thread = std::thread::spawn(move || run(conn, notify_tx, ready_tx, thread_wake_member));

        match ready_rx.recv() {
            Ok(()) => Ok((
                NativeLoop {
                    thread: Some(thread),
                    wake_conn,
                    wake_member,
                },
                notify_rx,
            )),
            Err(_) => {
                let _ = thread.join();
                Err(NativeLoopError::ThreadDiedBeforeReady)
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            // Unblocks the reader thread's blocking `MessageIterator`
            // wait promptly instead of waiting for the next real logind
            // event, which could be arbitrarily far in the future — see
            // module doc comment. Uses this instance's own unique
            // `wake_member`, not a shared one — see that function's doc
            // comment for the real bug this fixes.
            let _ = self.wake_conn.emit_signal(
                None::<&str>,
                "/",
                WAKE_INTERFACE,
                self.wake_member.as_str(),
                &(),
            );
            let _ = thread.join();
        }
    }
}

impl Drop for NativeLoop {
    fn drop(&mut self) {
        self.stop();
    }
}

fn add_match(conn: &Connection, rule: &str) -> Result<(), NativeLoopError> {
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "AddMatch",
        &(rule,),
    )?;
    Ok(())
}

fn run(
    conn: Connection,
    notify_tx: mpsc::Sender<PowerEvent>,
    ready_tx: mpsc::Sender<()>,
    wake_member: String,
) {
    let mut running = true;
    let mut stop_requested = false;
    let iter = zbus::blocking::MessageIterator::from(&conn);

    if ready_tx.send(()).is_err() {
        return;
    }

    for msg in iter {
        let Ok(msg) = msg else { break };
        let header = msg.header();
        let Some(member) = header.member() else {
            continue;
        };
        let Some(interface) = header.interface() else {
            continue;
        };

        if interface.as_str() == WAKE_INTERFACE && member.as_str() == wake_member {
            stop_requested = true;
        } else if interface.as_str() == "org.freedesktop.login1.Manager" {
            match member.as_str() {
                "PrepareForSleep" => {
                    if let Ok(going_to_sleep) = msg.body().deserialize::<bool>() {
                        let event = if going_to_sleep {
                            PowerEvent::Suspend
                        } else {
                            PowerEvent::Resume
                        };
                        let _ = notify_tx.send(event);
                    }
                }
                "PrepareForShutdown" => {
                    if let Ok(true) = msg.body().deserialize::<bool>() {
                        let _ = notify_tx.send(PowerEvent::EndSession);
                    }
                }
                _ => {}
            }
        } else if interface.as_str() == "org.freedesktop.login1.Session" {
            match member.as_str() {
                "Lock" => {
                    let _ = notify_tx.send(PowerEvent::SessionLock);
                }
                "Unlock" => {
                    let _ = notify_tx.send(PowerEvent::SessionUnlock);
                }
                _ => {}
            }
        }

        if stop_requested {
            running = false;
        }
        if !running {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn recv_with_timeout(rx: &Receiver<PowerEvent>) -> PowerEvent {
        rx.recv_timeout(Duration::from_secs(10))
            .expect("expected a translated PowerEvent within 10s")
    }

    /// Real D-Bus, real systemd-logind, real `Session.Lock`/`Unlock`
    /// signals — triggered via a real `loginctl lock-session`/
    /// `unlock-session` call against the actual session this test runs
    /// in, not a synthetic message (unlike the Windows `native_loop.rs`
    /// tests, which had to synthesize `PostMessageW` because there was
    /// no safe way to trigger a real Windows lock without disrupting the
    /// interactive session — `loginctl` on a specific, named,
    /// non-interactive-desktop WSL session is safe to actually lock and
    /// unlock for real, since nothing renders a visible lock screen).
    #[test]
    fn real_session_lock_and_unlock_round_trip() {
        let (mut native_loop, rx) = NativeLoop::start().expect("start must succeed");

        let session_id =
            std::env::var("LINUX_COLLECTOR_TEST_SESSION_ID").unwrap_or_else(|_| "self".to_string());

        let lock = std::process::Command::new("loginctl")
            .args(["lock-session", &session_id])
            .status()
            .expect("run loginctl lock-session");
        assert!(lock.success(), "loginctl lock-session failed");
        assert_eq!(recv_with_timeout(&rx), PowerEvent::SessionLock);

        let unlock = std::process::Command::new("loginctl")
            .args(["unlock-session", &session_id])
            .status()
            .expect("run loginctl unlock-session");
        assert!(unlock.success(), "loginctl unlock-session failed");
        assert_eq!(recv_with_timeout(&rx), PowerEvent::SessionUnlock);

        native_loop.stop();
    }

    #[test]
    fn stop_returns_promptly_without_a_real_logind_event() {
        let (mut native_loop, _rx) = NativeLoop::start().expect("start must succeed");
        let start = std::time::Instant::now();
        native_loop.stop();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "stop() must not wait for a real logind event to arrive"
        );
    }

    #[test]
    fn stop_is_idempotent() {
        let (mut native_loop, _rx) = NativeLoop::start().expect("start must succeed");
        native_loop.stop();
        native_loop.stop(); // must not panic/hang
    }
}
