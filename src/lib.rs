//! herdr-estate: durable campaigns for herdr. See `docs/vocabulary.md` for
//! the six words this crate uses in their one defined sense.

pub mod campaign;
pub mod config;
pub mod errors;
pub mod frontmatter;
pub mod paths;
pub mod verb;
pub mod worktree;

/// Test-only support for the tests scattered across this crate's modules
/// that read or write process environment variables. `std::env` is
/// process-global, and `cargo test` runs tests on multiple threads by
/// default, so every test that touches it goes through one `EnvGuard`
/// that serializes on a shared lock and restores whatever it changed when
/// it drops.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        /// Apply every `(key, value)` in `sets`, then remove every key in
        /// `unsets`, holding the shared lock until the guard drops, at
        /// which point every touched key's original value (or absence)
        /// is restored. A test that needs both together makes ONE
        /// `EnvGuard::new` call — never two guards live at once, which
        /// would deadlock on this non-reentrant lock.
        pub fn new(sets: &[(&str, &str)], unsets: &[&str]) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut saved = Vec::new();
            for (k, v) in sets {
                saved.push((k.to_string(), std::env::var(k).ok()));
                std::env::set_var(k, v);
            }
            for k in unsets {
                saved.push((k.to_string(), std::env::var(k).ok()));
                std::env::remove_var(k);
            }
            EnvGuard { _lock: lock, saved }
        }

        pub fn set(pairs: &[(&str, &str)]) -> Self {
            Self::new(pairs, &[])
        }

        pub fn unset(keys: &[&str]) -> Self {
            Self::new(&[], keys)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in self.saved.drain(..) {
                match v {
                    Some(val) => std::env::set_var(&k, val),
                    None => std::env::remove_var(&k),
                }
            }
        }
    }

    /// A fresh, empty directory under the OS temp dir, removed when the
    /// guard drops. Stands in for the `tempfile` crate, which is outside
    /// this crate's dependency budget.
    pub struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        pub fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "herdr-estate-test-{}-{n}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create temp dir for test");
            TempDir { path }
        }

        pub fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Default for TempDir {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
