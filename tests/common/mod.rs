use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "herdr-estate-test-{}-{n}-{nanos}-{tag}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("create test temp dir");
    path
}

pub struct FakeHerdr {
    dir: PathBuf,
}

impl FakeHerdr {
    pub fn new() -> Self {
        let dir = unique_dir("fake-herdr");
        std::fs::create_dir_all(dir.join("calls")).unwrap();
        let script = dir.join("herdr");
        let body = "#!/bin/sh\n\
            DIR=\"$(cd \"$(dirname \"$0\")\" && pwd)/calls\"\n\
            IDXFILE=\"$DIR/count\"\n\
            if [ -f \"$IDXFILE\" ]; then IDX=$(cat \"$IDXFILE\"); else IDX=0; fi\n\
            echo $((IDX + 1)) > \"$IDXFILE\"\n\
            for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$DIR/$IDX.argv\"\n\
            if [ -f \"$DIR/$IDX.stdout\" ]; then cat \"$DIR/$IDX.stdout\"; fi\n\
            if [ -f \"$DIR/$IDX.exit\" ]; then exit \"$(cat \"$DIR/$IDX.exit\")\"; fi\n\
            exit 0\n";
        std::fs::write(&script, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        FakeHerdr { dir }
    }

    pub fn bin_path(&self) -> PathBuf {
        self.dir.join("herdr")
    }

    pub fn set_response(&self, index: usize, stdout: &str, exit_code: i32) {
        std::fs::write(
            self.dir.join("calls").join(format!("{index}.stdout")),
            stdout,
        )
        .unwrap();
        std::fs::write(
            self.dir.join("calls").join(format!("{index}.exit")),
            exit_code.to_string(),
        )
        .unwrap();
    }

    pub fn call_count(&self) -> usize {
        std::fs::read_to_string(self.dir.join("calls").join("count"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn argv(&self, index: usize) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("calls").join(format!("{index}.argv")))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

pub struct Fixture {
    pub home: PathBuf,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub fake: FakeHerdr,
}

pub fn write_estate_toml(config_dir: &Path, contents: &str) {
    std::fs::create_dir_all(config_dir).unwrap();
    std::fs::write(config_dir.join("estates.toml"), contents).unwrap();
}

pub fn make_campaign(root: &Path, name: &str, frontmatter: &str) -> PathBuf {
    let cdir = root.join("campaigns").join(name);
    std::fs::create_dir_all(&cdir).unwrap();
    std::fs::write(cdir.join("campaign.md"), frontmatter).unwrap();
    cdir
}

pub fn fixture() -> (Fixture, PathBuf) {
    let base = unique_dir("fixture");
    let root = base.join("home");
    let home = base.join("userhome");
    std::fs::create_dir_all(&home).unwrap();
    let config_dir = base.join("config");
    write_estate_toml(
        &config_dir,
        &format!("[estate.home]\nroot = {:?}\n", root.to_string_lossy()),
    );
    let f = Fixture {
        home,
        config_dir,
        state_dir: base.join("state"),
        fake: FakeHerdr::new(),
    };
    (f, root)
}

pub fn run(f: &Fixture, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_herdr-estate"));
    cmd.args(args)
        .env_clear()
        .env("HOME", &f.home)
        .env("HERDR_PLUGIN_CONFIG_DIR", &f.config_dir)
        .env("HERDR_PLUGIN_STATE_DIR", &f.state_dir)
        .env("HERDR_BIN_PATH", f.fake.bin_path())
        .env("PATH", "/usr/bin:/bin");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("run the built herdr-estate binary")
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}
