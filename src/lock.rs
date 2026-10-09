use std::fs::{File, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::errors::EstateError;
use crate::paths::state_dir;

const HOLD_WAIT: Duration = Duration::from_secs(10);
const HOLD_POLL: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub struct FileLock {
    _file: File,
}

fn lock_path(address: &str) -> PathBuf {
    let name = address.replace('%', "%25").replace('/', "%2F");
    state_dir().join("locks").join(format!("{name}.lock"))
}

fn owned_lock_path() -> PathBuf {
    state_dir().join("owned.lock")
}

fn sort_lock_path() -> PathBuf {
    state_dir().join("sort.lock")
}

fn try_take(path: &Path) -> Result<Option<FileLock>, EstateError> {
    let failed =
        |e: std::io::Error| EstateError::Config(format!("could not lock {}: {e}", path.display()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(failed)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(failed)?;
    match file.try_lock() {
        Ok(()) => {
            if file.set_len(0).is_ok() {
                let _ = write!(file, "{}", std::process::id());
            }
            Ok(Some(FileLock { _file: file }))
        }
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(failed(e)),
    }
}

fn holder(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|pid| format!(" (pid {pid})"))
        .unwrap_or_default()
}

pub fn acquire(address: &str) -> Result<FileLock, EstateError> {
    #[cfg(test)]
    assert!(
        crate::paths::non_empty_env("HERDR_PLUGIN_STATE_DIR").is_some(),
        "lock::acquire reached with HERDR_PLUGIN_STATE_DIR unset"
    );
    let path = lock_path(address);
    try_take(&path)?.ok_or_else(|| {
        EstateError::Guard(format!(
            "{address}: another herdr-estate run{} is opening this campaign; try again once it finishes",
            holder(&path)
        ))
    })
}

pub fn hold_owned() -> std::io::Result<FileLock> {
    hold(&owned_lock_path())
}

pub fn hold_sort() -> std::io::Result<FileLock> {
    hold(&sort_lock_path())
}

fn hold(path: &Path) -> std::io::Result<FileLock> {
    let gives_up = std::time::Instant::now() + HOLD_WAIT;
    loop {
        match try_take(path) {
            Ok(Some(lock)) => return Ok(lock),
            Ok(None) if std::time::Instant::now() < gives_up => std::thread::sleep(HOLD_POLL),
            Ok(None) => {
                return Err(std::io::Error::other(format!(
                    "{} is still held{} after {}s",
                    path.display(),
                    holder(path),
                    HOLD_WAIT.as_secs()
                )))
            }
            Err(e) => return Err(std::io::Error::other(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, TempDir};

    fn isolated() -> (TempDir, EnvGuard) {
        let d = TempDir::new();
        let g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        (d, g)
    }

    #[test]
    fn a_held_lock_refuses_a_second_taker_naming_its_pid_and_frees_on_drop() {
        let (_d, _g) = isolated();
        let held = acquire("home/garden").unwrap();
        let err = acquire("home/garden").unwrap_err();
        assert!(
            matches!(&err, EstateError::Guard(m) if m.contains(&format!("(pid {})", std::process::id()))),
            "{err}"
        );
        assert!(acquire("home/attic").is_ok());
        drop(held);
        assert!(acquire("home/garden").is_ok());
    }

    #[test]
    fn a_lock_file_left_behind_with_any_contents_is_taken_when_nobody_holds_it() {
        let (_d, _g) = isolated();
        let path = lock_path("home/garden");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for contents in ["", "garbage", "1 2 3", "99999999"] {
            std::fs::write(&path, contents).unwrap();
            let lock = acquire("home/garden").unwrap();
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                std::process::id().to_string()
            );
            drop(lock);
        }
    }

    #[test]
    fn a_refusal_whose_holder_wrote_no_pid_still_reads_cleanly() {
        let (_d, _g) = isolated();
        let path = lock_path("home/garden");
        let held = acquire("home/garden").unwrap();
        std::fs::write(&path, "").unwrap();
        let err = acquire("home/garden").unwrap_err();
        assert!(
            matches!(&err, EstateError::Guard(m) if m.starts_with("home/garden: another herdr-estate run is opening")),
            "{err}"
        );
        drop(held);
    }

    #[test]
    fn the_owned_lock_waits_for_its_holder() {
        let (d, _g) = isolated();
        let held = hold_owned().unwrap();
        assert!(d.path().join("owned.lock").exists());
        let waiter = std::thread::spawn(|| {
            let started = std::time::Instant::now();
            drop(hold_owned().unwrap());
            started.elapsed()
        });
        std::thread::sleep(Duration::from_millis(200));
        drop(held);
        assert!(waiter.join().unwrap() >= Duration::from_millis(150));
    }

    #[test]
    fn a_held_sort_lock_makes_a_second_sort_wait_and_never_blocks_owned() {
        let (d, _g) = isolated();
        let held = hold_sort().unwrap();
        assert!(d.path().join("sort.lock").exists());
        drop(hold_owned().unwrap());
        let waiter = std::thread::spawn(|| {
            let started = std::time::Instant::now();
            drop(hold_sort().unwrap());
            started.elapsed()
        });
        std::thread::sleep(Duration::from_millis(200));
        drop(held);
        assert!(waiter.join().unwrap() >= Duration::from_millis(150));
    }

    #[test]
    fn an_address_with_slashes_maps_to_one_file_in_the_lock_dir() {
        let (d, _g) = isolated();
        assert_eq!(
            lock_path("home/a/b%c"),
            d.path().join("locks").join("home%2Fa%2Fb%25c.lock")
        );
    }
}
