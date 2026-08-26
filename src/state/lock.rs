//! Advisory file locking for exclusive state access.
//!
//! Uses [`fs2::FileExt`] for cross-platform advisory locks.
//! The lock is released automatically when the [`StateLock`] guard
//! is dropped.

use std::{fs::File, path::Path};

use fs2::FileExt as _;

use crate::error::ForgeError;

/// Lock file name within the state directory.
const LOCK_FILE: &str = "lock";

/// RAII guard that holds an exclusive advisory lock on a file.
///
/// The lock is released when this guard is dropped (the underlying
/// file handle closes, releasing the advisory lock).
pub struct StateLock {
    /// Held open for the lock lifetime.
    _file: File,
}

/// Acquire an exclusive lock on `<state_dir>/lock`.
///
/// Creates the lock file and state directory if they do not exist.
/// Blocks until the lock is acquired.
///
/// # Errors
///
/// Returns [`ForgeError::Lock`] if the lock file cannot be created
/// or the lock cannot be acquired.
pub fn acquire(state_dir: &Path) -> Result<StateLock, ForgeError> {
    crate::state::ensure_dir(state_dir)?;
    let path = lock_path(state_dir);
    let file = open_lock_file(&path)?;
    lock_exclusive(&file, &path)?;
    write_holder_pid(&file, &path)?;
    Ok(StateLock { _file: file })
}

/// Build the lock file path.
fn lock_path(state_dir: &Path) -> std::path::PathBuf {
    state_dir.join(LOCK_FILE)
}

/// Open or create the lock file.
fn open_lock_file(path: &Path) -> Result<File, ForgeError> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|err| ForgeError::Lock(format!("cannot open lock file {}: {err}", path.display())))
}

/// Lock the file exclusively, blocking until acquired.
///
/// Tries a non-blocking acquisition first; on contention a one-line
/// notice is printed to stderr (a `forge up` can hold the lock for
/// minutes) before falling back to the blocking lock.
fn lock_exclusive(file: &File, path: &Path) -> Result<(), ForgeError> {
    match file.try_lock_exclusive() {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
            report_waiting(path);
            file.lock_exclusive()
                .map_err(|block_err| ForgeError::Lock(format!("cannot acquire lock: {block_err}")))
        },
        Err(err) => Err(ForgeError::Lock(format!("cannot acquire lock: {err}"))),
    }
}

/// Print a one-line stderr notice that the lock is contended.
///
/// Names the holder's PID when the lock file records one, so the user
/// can find the other forge process instead of guessing why this one
/// appears frozen.
#[expect(
    clippy::print_stderr,
    reason = "user-facing wait notice; the lock may be held for minutes"
)]
fn report_waiting(path: &Path) {
    let holder = std::fs::read_to_string(path)
        .ok()
        .and_then(|content| content.trim().parse::<u32>().ok());
    match holder {
        Some(pid) => eprintln!("forge: waiting for lock at {} held by pid {pid} ...", path.display()),
        None => eprintln!("forge: waiting for lock at {} ...", path.display()),
    }
}

/// Record the holder's PID in the locked file.
///
/// Lets a contending forge invocation report who holds the lock. The
/// PID is left behind after release; it is only ever read while the
/// lock is contended, so a stale value is never shown.
fn write_holder_pid(mut file: &File, path: &Path) -> Result<(), ForgeError> {
    use std::io::{Seek as _, Write as _};

    let map_lock_err =
        |err: std::io::Error| ForgeError::Lock(format!("cannot record pid in lock file {}: {err}", path.display()));
    file.set_len(0).map_err(map_lock_err)?;
    file.seek(std::io::SeekFrom::Start(0)).map_err(map_lock_err)?;
    file.write_all(format!("{}\n", std::process::id()).as_bytes())
        .map_err(map_lock_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_creates_lock_file() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| {
            std::process::abort();
            #[expect(unreachable_code, reason = "abort prevents reaching this")]
            {
                unreachable!()
            }
        });
        let state_dir = dir.path().join("state");
        let _lock = acquire(&state_dir).unwrap_or_else(|_| {
            std::process::abort();
            #[expect(unreachable_code, reason = "abort prevents reaching this")]
            {
                unreachable!()
            }
        });
        assert!(state_dir.join(LOCK_FILE).exists(), "lock file should exist");
    }

    #[test]
    fn acquire_records_holder_pid_in_lock_file() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| std::process::abort());
        let state_dir = dir.path().join("state");
        let _lock = acquire(&state_dir).unwrap_or_else(|_| std::process::abort());
        let content = std::fs::read_to_string(state_dir.join(LOCK_FILE)).unwrap_or_else(|_| std::process::abort());
        assert_eq!(
            content.trim().parse::<u32>().ok(),
            Some(std::process::id()),
            "lock file should record the holder's pid, got {content:?}"
        );
    }

    #[test]
    fn acquire_creates_state_dir() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| {
            std::process::abort();
            #[expect(unreachable_code, reason = "abort prevents reaching this")]
            {
                unreachable!()
            }
        });
        let state_dir = dir.path().join("nested").join("state");
        let _lock = acquire(&state_dir).unwrap_or_else(|_| {
            std::process::abort();
            #[expect(unreachable_code, reason = "abort prevents reaching this")]
            {
                unreachable!()
            }
        });
        assert!(state_dir.exists(), "state directory should be created");
    }
}
