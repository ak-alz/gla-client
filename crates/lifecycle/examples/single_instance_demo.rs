//! Manual verification tool (not shipped, not part of the crate's public
//! API): the unit tests in `single_instance.rs` prove the lock logic works
//! across THREADS within one test process, but the real acceptance
//! criterion ("Нет duplicate processes") is about separate OS PROCESSES —
//! this binary is meant to be launched twice, as two real, independent
//! processes, to prove the underlying OS-level advisory lock (the same
//! primitive already proven in `durable-queue`'s directory lock, AG-004)
//! actually excludes a second real process, not just a second thread.
//!
//! Usage: `single_instance_demo <lock_path>` — prints `ACQUIRED` and
//! sleeps for a few seconds if it got the lock, or `ALREADY_RUNNING` and
//! exits immediately if it didn't.
use lifecycle::acquire;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let lock_path = PathBuf::from(
        args.get(1)
            .expect("usage: single_instance_demo <lock_path>"),
    );

    match acquire(&lock_path) {
        Ok(_guard) => {
            println!("ACQUIRED pid={}", std::process::id());
            std::thread::sleep(std::time::Duration::from_secs(3));
            println!("RELEASING pid={}", std::process::id());
        }
        Err(err) => {
            println!("ALREADY_RUNNING pid={} ({err})", std::process::id());
        }
    }
}
