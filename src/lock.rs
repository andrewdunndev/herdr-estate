use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::doubleopen::pid_alive;
use crate::errors::EstateError;
use crate::paths::state_dir;

const STALE_AFTER: Duration = Duration::from_secs(120);
const OWNED_WAIT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub struct FileLock {
    path: PathBuf,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_path(address: &str) -> PathBuf {
    let name = address.replace('%', "%25").replace('/', "%2F");
    state_dir().join("locks").join(format!("{name}.lock"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn holder(contents: &str) -> Option<(u32, u64)> {
    let mut fields = contents.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let since = fields.next()?.parse().ok()?;
    Some((pid, since))
}

fn is_stale(contents: &str) -> bool {
    match holder(contents) {
        Some((pid, since)) => {
            !pid_alive(pid) || now_secs().saturating_sub(since) > STALE_AFTER.as_secs()
        }
        None => true,
    }
}

fn take(path: &std::path::Path) -> Result<Result<FileLock, u32>, EstateError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| EstateError::Config(format!("could not create {}: {e}", dir.display())))?;
    }
    let mut holder_pid = 0;
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "{} {}", std::process::id(), now_secs());
                return Ok(Ok(FileLock {
                    path: path.to_path_buf(),
                }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let contents = std::fs::read_to_string(path).unwrap_or_default();
                if !is_stale(&contents) {
                    holder_pid = holder(&contents).map(|(p, _)| p).unwrap_or(0);
                    return Ok(Err(holder_pid));
                }
                if std::fs::read_to_string(path).unwrap_or_default() == contents {
                    let _ = std::fs::remove_file(path);
                }
            }
            Err(e) => {
                return Err(EstateError::Config(format!(
                    "could not create {}: {e}",
                    path.display()
                )))
            }
        }
    }
    Ok(Err(holder_pid))
}

pub fn acquire(address: &str) -> Result<FileLock, EstateError> {
    #[cfg(test)]
    assert!(
        crate::paths::non_empty_env("HERDR_PLUGIN_STATE_DIR").is_some(),
        "lock::acquire reached with HERDR_PLUGIN_STATE_DIR unset"
    );
    take(&lock_path(address))?.map_err(|pid| {
        EstateError::Guard(format!(
            "{address}: another herdr-estate run (pid {pid}) is opening this campaign; try again once it finishes"
        ))
    })
}

pub fn hold_owned() -> std::io::Result<FileLock> {
    let path = state_dir().join("owned.lock");
    let gives_up = std::time::Instant::now() + OWNED_WAIT;
    loop {
        match take(&path) {
            Ok(Ok(lock)) => return Ok(lock),
            Ok(Err(_)) if std::time::Instant::now() < gives_up => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(Err(pid)) => {
                return Err(std::io::Error::other(format!(
                    "{} is held by pid {pid}",
                    path.display()
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
    fn a_held_lock_refuses_a_second_taker_and_is_released_on_drop() {
        let (_d, _g) = isolated();
        let held = acquire("home/garden").unwrap();
        let err = acquire("home/garden").unwrap_err();
        assert!(
            matches!(&err, EstateError::Guard(m) if m.contains(&format!("pid {}", std::process::id()))),
            "{err}"
        );
        assert!(acquire("home/attic").is_ok());
        drop(held);
        assert!(acquire("home/garden").is_ok());
        assert!(!lock_path("home/garden").exists());
    }

    #[test]
    fn a_lock_whose_holder_is_gone_or_too_old_is_taken_over() {
        let (_d, _g) = isolated();
        let path = lock_path("home/garden");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let dead = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = dead.id();
        let _ = dead.wait_with_output();
        for contents in [
            format!("{dead_pid} {}\n", now_secs()),
            format!("{} {}\n", std::process::id(), now_secs() - 121),
            "garbage".to_string(),
        ] {
            std::fs::write(&path, &contents).unwrap();
            let lock = acquire("home/garden").unwrap();
            let now = std::fs::read_to_string(&path).unwrap();
            assert!(
                now.starts_with(&format!("{} ", std::process::id())),
                "{contents:?}"
            );
            drop(lock);
        }
    }

    #[test]
    fn the_owned_lock_waits_for_its_holder_then_takes_over() {
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
    fn an_address_with_slashes_maps_to_one_file_in_the_lock_dir() {
        let (d, _g) = isolated();
        assert_eq!(
            lock_path("home/a/b%c"),
            d.path().join("locks").join("home%2Fa%2Fb%25c.lock")
        );
    }
}
