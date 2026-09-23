//! herdr-estate: durable campaigns for herdr. See `docs/vocabulary.md` for
//! the six words this crate uses in their one defined sense.

mod adapters;
mod campaign;
pub mod cli;
mod config;
mod continuity;
mod dispatch;
mod doubleopen;
mod errors;
mod frontmatter;
mod herdrcli;
mod name;
mod ownership;
mod paths;
pub mod picker;
mod progress;
mod prompt;
mod verb;
mod verbs;
mod worktree;

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

    /// A fake `herdr` executable for tests that cross the herdr CLI
    /// boundary. Point `HERDR_BIN_PATH` at `bin_path()`; every call the
    /// code under test makes records its exact argv and environment to
    /// this directory, in call order, and replays whatever canned
    /// response `set_response` queued for that call index — so a test
    /// asserts the real thing (what herdr was actually asked to do)
    /// rather than that a mock was called.
    pub struct FakeHerdr {
        dir: TempDir,
    }

    impl FakeHerdr {
        pub fn new() -> Self {
            let dir = TempDir::new();
            let script = dir.path().join("herdr");
            let calls = dir.path().join("calls");
            std::fs::create_dir_all(&calls).expect("create fake herdr calls dir");
            let body = "#!/bin/sh\n\
                DIR=\"$(cd \"$(dirname \"$0\")\" && pwd)/calls\"\n\
                IDXFILE=\"$DIR/count\"\n\
                if [ -f \"$IDXFILE\" ]; then IDX=$(cat \"$IDXFILE\"); else IDX=0; fi\n\
                echo $((IDX + 1)) > \"$IDXFILE\"\n\
                for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$DIR/$IDX.argv\"\n\
                env > \"$DIR/$IDX.env\"\n\
                if [ -f \"$DIR/$IDX.stdout\" ]; then cat \"$DIR/$IDX.stdout\"; fi\n\
                if [ -f \"$DIR/$IDX.exit\" ]; then exit \"$(cat \"$DIR/$IDX.exit\")\"; fi\n\
                exit 0\n";
            std::fs::write(&script, body).expect("write fake herdr script");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&script)
                    .expect("stat fake herdr script")
                    .permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&script, perms).expect("chmod fake herdr script");
            }
            FakeHerdr { dir }
        }

        pub fn bin_path(&self) -> std::path::PathBuf {
            self.dir.path().join("herdr")
        }

        fn calls_dir(&self) -> std::path::PathBuf {
            self.dir.path().join("calls")
        }

        /// Queue the canned stdout and exit code for the call at
        /// `index` (0-based, in the order the code under test makes
        /// them), before that code runs.
        pub fn set_response(&self, index: usize, stdout: &str, exit_code: i32) {
            std::fs::write(self.calls_dir().join(format!("{index}.stdout")), stdout)
                .expect("write fake herdr canned stdout");
            std::fs::write(
                self.calls_dir().join(format!("{index}.exit")),
                exit_code.to_string(),
            )
            .expect("write fake herdr canned exit code");
        }

        pub fn call_count(&self) -> usize {
            std::fs::read_to_string(self.calls_dir().join("count"))
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0)
        }

        pub fn argv(&self, index: usize) -> Vec<String> {
            std::fs::read_to_string(self.calls_dir().join(format!("{index}.argv")))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        pub fn env_value(&self, index: usize, key: &str) -> Option<String> {
            let text =
                std::fs::read_to_string(self.calls_dir().join(format!("{index}.env"))).ok()?;
            text.lines().find_map(|line| {
                line.split_once('=')
                    .filter(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            })
        }
    }

    impl Default for FakeHerdr {
        fn default() -> Self {
            Self::new()
        }
    }
}
