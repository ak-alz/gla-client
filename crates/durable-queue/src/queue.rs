use crate::config::QueueConfig;
use crate::error::QueueError;
use chrono::{DateTime, Utc};
use event_contract::{Envelope, EventId};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

/// Bumped when the on-disk layout changes (subdirectory names, filename
/// scheme, or what a pending-file's content means). `DurableQueue::open`
/// refuses to open a directory stamped with a different version rather than
/// silently misinterpreting its contents — this is the "migration version"
/// mechanism AG-004 requires; there is only one version today; a future
/// bump must ship an explicit migration, not just change this constant.
///
/// Bumped from "1" to "2" in this revision: `leased/` is a new subdirectory
/// an old-format reader would not know to recover on open (see `open`'s
/// lease-recovery step) — a real layout change, not cosmetic.
pub const QUEUE_FORMAT_VERSION: &str = "2";
const FORMAT_VERSION_FILE: &str = "QUEUE_FORMAT_VERSION";

const PENDING_SUBDIR: &str = "pending";
const LEASED_SUBDIR: &str = "leased";
const ACKED_SUBDIR: &str = "acked";
const QUARANTINE_SUBDIR: &str = "quarantine";
const LOCK_FILE: &str = ".lock";

static ATTEMPT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// Newly written and durable. Only returned after the record's
    /// containing directory entry itself has been made durable (see
    /// `fsync_dir`), not merely after the file write returned.
    Enqueued,
    /// An event with this `event_id` was already pending, leased, acked, or
    /// quarantined — treated as success, not an error: enqueuing the same
    /// event twice (e.g. a collector retry after a timeout whose first
    /// attempt actually landed) must be safe, not accumulate a duplicate.
    AlreadyPresent,
}

#[derive(Debug, Clone)]
pub struct QueuedRecord {
    pub envelope: Envelope,
    /// Best-effort: the pending file's own filesystem modified time at the
    /// moment it was dequeued, used only to order `dequeue_batch`'s
    /// results — not a substitute for `envelope.occurred_at`, which is
    /// authoritative for the record's own content.
    pub enqueued_at: DateTime<Utc>,
}

/// Safe to share across threads (`Arc<DurableQueue>`) and call concurrently:
/// every mutating operation (`enqueue`, `dequeue_batch`, `ack`, the prune
/// methods) takes `lock` for its whole duration. This is load-bearing, not
/// defensive-programming caution: an isolated repro during this crate's own
/// development proved that concurrent `std::fs::rename` calls racing on the
/// SAME source path are **not** atomic on this Windows filesystem — multiple
/// threads independently observed `Ok(())` for renaming one file, which on
/// POSIX would be impossible (rename(2) is atomic w.r.t. the source). The
/// one-file-per-record design (hard-link for create-if-absent enqueue,
/// pending→leased rename as the dequeue "checkout") is still what makes
/// dedup/crash-recovery/corruption-isolation correct — but only the mutex
/// makes those filesystem operations safe to issue from multiple threads of
/// the same process concurrently.
///
/// Exactly one live `DurableQueue` per directory, enforced (not just
/// documented): `open()` holds an OS-level advisory lock on `.lock` for the
/// handle's entire lifetime (released automatically on drop, and by the OS
/// if the process dies without a clean shutdown). A second `open()` call on
/// the same directory — from another thread of this process, or another
/// process entirely — fails with `QueueError::AlreadyOpen` instead of
/// succeeding. This was added after independent review found that two
/// `open()` calls on the same directory, if both allowed to succeed, would
/// each run `open()`'s lease-recovery step with no way to tell "genuinely
/// abandoned by a dead process" apart from "actively held by the other,
/// still-live handle" — silently reintroducing the same double-delivery
/// class of bug the `Mutex` above was added to close, one level up.
pub struct DurableQueue {
    dir: PathBuf,
    pending_dir: PathBuf,
    leased_dir: PathBuf,
    acked_dir: PathBuf,
    quarantine_dir: PathBuf,
    config: QueueConfig,
    lock: Mutex<()>,
    // Never read after construction — its only job is to stay alive (and
    // therefore keep the OS-level advisory lock held) for as long as this
    // `DurableQueue` is. Dropping it (implicitly, when `DurableQueue` drops)
    // releases the lock.
    _directory_lock: fs::File,
}

impl DurableQueue {
    /// Opens (creating on first use) the queue at `config.dir`. Fails with
    /// `QueueError::AlreadyOpen` if another live `DurableQueue` handle
    /// already holds this directory's lock (see the struct doc comment) —
    /// this makes the crash-recovery steps below actually safe, not just
    /// plausible: they only ever run once it's certain no other live handle
    /// could be concurrently relying on the state being "recovered".
    ///
    /// Runs two kinds of crash recovery: any `*.json.tmp` file left in
    /// `pending/` is a write interrupted before its hard-link (by
    /// definition `enqueue()` never returned success for it, so removing it
    /// loses nothing that was ever confirmed durable); and any file left in
    /// `leased/` is a record a *previous* process dequeued but never acked
    /// before stopping — since holding the directory lock rules out a
    /// still-running consumer of THIS directory, it is safe to move it back
    /// to `pending/` to be retried, not lost or double-held.
    pub fn open(config: QueueConfig) -> Result<Self, QueueError> {
        let dir = config.dir.clone();
        fs::create_dir_all(&dir)?;

        // Acquired FIRST, before touching anything else: everything after
        // this point (format-version check, lease recovery) assumes no
        // other live handle can be concurrently doing the same thing.
        let lock_path = dir.join(LOCK_FILE);
        let directory_lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        match directory_lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(QueueError::AlreadyOpen { dir }),
            Err(fs::TryLockError::Error(err)) => return Err(err.into()),
        }

        let version_path = dir.join(FORMAT_VERSION_FILE);
        match fs::read_to_string(&version_path) {
            Ok(found) if found.trim() == QUEUE_FORMAT_VERSION => {}
            Ok(found) => {
                return Err(QueueError::UnsupportedFormatVersion {
                    dir,
                    found: found.trim().to_string(),
                    expected: QUEUE_FORMAT_VERSION.to_string(),
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                fs::write(&version_path, QUEUE_FORMAT_VERSION)?;
            }
            Err(err) => return Err(err.into()),
        }

        let pending_dir = dir.join(PENDING_SUBDIR);
        let leased_dir = dir.join(LEASED_SUBDIR);
        let acked_dir = dir.join(ACKED_SUBDIR);
        let quarantine_dir = dir.join(QUARANTINE_SUBDIR);
        fs::create_dir_all(&pending_dir)?;
        fs::create_dir_all(&leased_dir)?;
        fs::create_dir_all(&acked_dir)?;
        fs::create_dir_all(&quarantine_dir)?;

        cleanup_stale_tmp_files(&pending_dir)?;
        recover_stale_leases(&leased_dir, &pending_dir)?;

        Ok(DurableQueue {
            dir,
            pending_dir,
            leased_dir,
            acked_dir,
            quarantine_dir,
            config,
            lock: Mutex::new(()),
            _directory_lock: directory_lock,
        })
    }

    /// Durably enqueues `envelope`. Each call writes to its OWN
    /// process-and-attempt-unique temp file (never shared across concurrent
    /// callers, even for the same `event_id`), fsyncs it, then attempts
    /// `fs::hard_link` — not `rename` — into `pending/{event_id}.json`.
    /// `hard_link` is the atomic primitive that actually matters here: it
    /// fails with `AlreadyExists` if the destination is already taken,
    /// instead of silently overwriting it the way `rename` would. That is
    /// what makes concurrent `enqueue` calls for the identical `event_id`
    /// safe: exactly one attempt's content becomes the file at that path,
    /// every other attempt observes the failure and reports
    /// `AlreadyPresent`, and no attempt can ever have its own `Ok(Enqueued)`
    /// paired with another attempt's content on disk.
    pub fn enqueue(&self, envelope: &Envelope) -> Result<EnqueueOutcome, QueueError> {
        let _guard = self.lock.lock().unwrap();

        let event_id = envelope.event_id.to_string();
        let name = format!("{event_id}.json");
        if self.pending_dir.join(&name).exists()
            || self.leased_dir.join(&name).exists()
            || self.acked_dir.join(&name).exists()
            || self.quarantine_dir.join(&name).exists()
        {
            return Ok(EnqueueOutcome::AlreadyPresent);
        }

        let current = self.pending_bytes_locked()?;
        if current >= self.config.max_pending_bytes {
            return Err(QueueError::Full {
                current_bytes: current,
                max_bytes: self.config.max_pending_bytes,
            });
        }

        let attempt = ATTEMPT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = self
            .pending_dir
            .join(format!("{event_id}.{}.{attempt}.tmp", std::process::id()));
        let json = serde_json::to_vec(envelope)
            .expect("Envelope serialization is infallible: all fields are plain owned data");
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(&json)?;
            file.sync_all()?;
        }

        let final_path = self.pending_dir.join(&name);
        let outcome = match fs::hard_link(&tmp_path, &final_path) {
            Ok(()) => {
                fsync_dir(&self.pending_dir)?;
                Ok(EnqueueOutcome::Enqueued)
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(EnqueueOutcome::AlreadyPresent)
            }
            Err(err) => Err(QueueError::Io(err)),
        };
        // The temp file's content is only ever meaningful before the
        // hard-link attempt (win or lose, `final_path` now holds whichever
        // attempt's content is canonical) — always clean up our own copy.
        let _ = fs::remove_file(&tmp_path);

        outcome
    }

    /// Returns up to `max` records, oldest-enqueued first, each atomically
    /// moved from `pending/` into `leased/` as part of being selected — this
    /// rename is the "checkout": only one concurrent `dequeue_batch` call
    /// (from any thread) can win the rename for a given file, so no two
    /// calls ever return the same record. A file that fails to parse is
    /// moved to `quarantine/` instead (preserved, not lost) and skipped —
    /// one corrupt record must never block every record behind it.
    pub fn dequeue_batch(&self, max: usize) -> Result<Vec<QueuedRecord>, QueueError> {
        let _guard = self.lock.lock().unwrap();

        let mut entries: Vec<(PathBuf, SystemTime)> = Vec::new();
        for entry in fs::read_dir(&self.pending_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue; // skips stray `.tmp` files and anything else unexpected
            }
            let modified = entry
                .metadata()?
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH);
            entries.push((path, modified));
        }
        entries.sort_by_key(|(_, modified)| *modified);

        let mut records = Vec::with_capacity(max.min(entries.len()));
        for (pending_path, modified) in entries {
            if records.len() >= max {
                break;
            }
            let Some(name) = pending_path.file_name().map(|n| n.to_owned()) else {
                continue;
            };
            let leased_path = self.leased_dir.join(&name);
            // The actual checkout: if another concurrent caller (or a
            // fast-running competing thread) already renamed this file
            // away, this fails with NotFound and we simply move on to the
            // next candidate — that is the race being resolved, not an error.
            if let Err(err) = fs::rename(&pending_path, &leased_path) {
                if err.kind() == std::io::ErrorKind::NotFound {
                    continue;
                }
                return Err(err.into());
            }
            match self.read_record(&leased_path, modified) {
                Ok(record) => records.push(record),
                Err(_) => self.quarantine(&leased_path)?,
            }
        }
        Ok(records)
    }

    fn read_record(&self, path: &Path, modified: SystemTime) -> Result<QueuedRecord, QueueError> {
        let raw = fs::read(path)?;
        let envelope: Envelope = serde_json::from_slice(&raw).map_err(|err| {
            QueueError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err))
        })?;
        Ok(QueuedRecord {
            envelope,
            enqueued_at: DateTime::<Utc>::from(modified),
        })
    }

    fn quarantine(&self, path: &Path) -> Result<(), QueueError> {
        if let Some(name) = path.file_name() {
            let dest = self.quarantine_dir.join(name);
            fs::rename(path, dest)?;
            fsync_dir(&self.quarantine_dir)?;
        }
        Ok(())
    }

    /// Marks `event_id` as durably uploaded: moves its file from `leased/`
    /// (where `dequeue_batch` put it) to `acked/`. A subsequent `enqueue` of
    /// the same `event_id` is recognized as `AlreadyPresent`, not re-added —
    /// this is what makes "duplicate upload safe" hold at the queue layer,
    /// not just at the uploader. Acking an `event_id` that is not currently
    /// leased (never dequeued, already acked, or already recovered back to
    /// `pending/` after a crash) is a no-op, not an error — an uploader
    /// retrying an ack after its own response was lost must be able to call
    /// this again safely.
    pub fn ack(&self, event_id: &EventId) -> Result<(), QueueError> {
        let _guard = self.lock.lock().unwrap();

        let name = format!("{event_id}.json");
        let leased_path = self.leased_dir.join(&name);
        if !leased_path.exists() {
            return Ok(());
        }
        let acked_path = self.acked_dir.join(&name);
        fs::rename(&leased_path, &acked_path)?;
        fsync_dir(&self.acked_dir)?;
        Ok(())
    }

    /// Deletes `acked/` files older than `config.acked_retention`. Never
    /// touches `pending/`/`leased/` — retention is about not keeping upload
    /// receipts forever, not about bounding unacked data by age (that would
    /// risk silently discarding data that was never actually delivered).
    /// Returns the number of files pruned.
    pub fn prune_expired_acks(&self) -> Result<usize, QueueError> {
        let _guard = self.lock.lock().unwrap();
        prune_dir_older_than(&self.acked_dir, self.config.acked_retention)
    }

    /// Deletes `quarantine/` files older than `config.acked_retention` (the
    /// same retention window as acked receipts — quarantine is not exempt
    /// from disk-usage bounds just because it holds corrupt data rather
    /// than valid receipts). Never touches `pending/`/`leased/`.
    pub fn prune_expired_quarantine(&self) -> Result<usize, QueueError> {
        let _guard = self.lock.lock().unwrap();
        prune_dir_older_than(&self.quarantine_dir, self.config.acked_retention)
    }

    /// Sum of `pending/` file sizes — what `enqueue`'s backpressure check
    /// compares against `config.max_pending_bytes`. Deliberately excludes
    /// `leased/` (already checked out, being processed, not "waiting");
    /// under heavy concurrent enqueueing this is a best-effort bound, not
    /// an exact atomic one (two enqueues can both read the size before
    /// either writes) — acceptable for a soft backpressure threshold, not
    /// for the crash-durability or dedup guarantees, which do not rely on it.
    pub fn pending_bytes(&self) -> Result<u64, QueueError> {
        let _guard = self.lock.lock().unwrap();
        self.pending_bytes_locked()
    }

    /// Same computation as `pending_bytes`, but assumes the caller already
    /// holds `lock` — used internally by `enqueue`'s backpressure check.
    /// `std::sync::Mutex` is not reentrant, so `enqueue` must call this, not
    /// the public `pending_bytes`, or it would deadlock against itself.
    fn pending_bytes_locked(&self) -> Result<u64, QueueError> {
        let mut total = 0u64;
        for entry in fs::read_dir(&self.pending_dir)? {
            let entry = entry?;
            if entry.path().extension().and_then(|e| e.to_str()) == Some("json") {
                total += entry.metadata()?.len();
            }
        }
        Ok(total)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn prune_dir_older_than(dir: &Path, retention: chrono::Duration) -> Result<usize, QueueError> {
    let cutoff = SystemTime::now()
        .checked_sub(
            retention
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(0)),
        )
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let mut pruned = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let modified = entry.metadata()?.modified().unwrap_or(SystemTime::now());
        if modified < cutoff {
            fs::remove_file(&path)?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

fn cleanup_stale_tmp_files(pending_dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(pending_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Any file still in `leased/` at `open()` time was checked out by a
/// process that is, by definition, no longer the one calling `open()` now —
/// nothing else holds a reference to it, so it is safe (and necessary, for
/// "retry") to return it to `pending/` rather than leave it stranded forever.
fn recover_stale_leases(leased_dir: &Path, pending_dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(leased_dir)? {
        let entry = entry?;
        let path = entry.path();
        if let Some(name) = path.file_name() {
            fs::rename(&path, pending_dir.join(name))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    let f = fs::File::open(dir)?;
    f.sync_all()
}

#[cfg(not(unix))]
fn fsync_dir(_dir: &Path) -> std::io::Result<()> {
    // Windows' `std::fs::File::open` cannot open a directory handle
    // (ERROR_ACCESS_DENIED without `FILE_FLAG_BACKUP_SEMANTICS`, which
    // `std::fs` does not expose) — this is a documented, deliberate
    // platform difference, not an oversight. NTFS's own transactional MFT
    // update model durably persists a completed, fsynced file rename
    // without requiring an explicit directory handle sync the way ext4/XFS
    // do; this function is a documented no-op on this platform family
    // rather than a silently-skipped requirement.
    Ok(())
}
