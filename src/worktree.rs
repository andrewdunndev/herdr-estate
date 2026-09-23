//! Optional worktrees.
//!
//! `ensure_worktree` is idempotent and additive only. It shells out to
//! plain git, never `herdr worktree` — that family takes no `--env`, so
//! the address could never reach the agent, and it registers a stray
//! base workspace. It never removes a worktree either: herdr's own
//! `worktree remove --force` can delete uncommitted work, and removal
//! stays a human act.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::errors::EstateError;
use crate::paths::{expand, state_dir};

struct GitOutput {
    ok: bool,
    stdout: String,
}

/// Run `git -C <repo> <args>` without checking the exit code, for probes
/// where a non-zero exit is an ordinary "no" (a missing ref, an unset
/// upstream), not a failure.
fn run_git(repo: &Path, args: &[&str]) -> Result<GitOutput, EstateError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| {
            EstateError::Config(format!(
                "failed to run git -C {} {}: {e}",
                repo.display(),
                args.join(" ")
            ))
        })?;
    Ok(GitOutput {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
    })
}

/// The `check=True` half: a non-zero exit is a real failure here.
fn git_checked(repo: &Path, args: &[&str]) -> Result<GitOutput, EstateError> {
    let out = run_git(repo, args)?;
    if !out.ok {
        return Err(EstateError::Config(format!(
            "git -C {} {} failed",
            repo.display(),
            args.join(" ")
        )));
    }
    Ok(out)
}

fn default_branch(repo: &Path) -> Result<String, EstateError> {
    let out = run_git(repo, &["symbolic-ref", "refs/remotes/origin/HEAD"])?;
    if out.ok {
        let s = out.stdout.trim();
        if !s.is_empty() {
            return Ok(match s.rfind('/') {
                Some(i) => s[i + 1..].to_string(),
                None => s.to_string(),
            });
        }
    }
    Ok("main".to_string())
}

/// A path from `git worktree list --porcelain` whose branch matches, or
/// `None`. Porcelain output is blank-line-separated blocks, each starting
/// with `worktree <path>`, followed by a `branch refs/heads/<name>` line
/// for a normal (non-detached) worktree.
fn existing_worktree(repo: &Path, branch: &str) -> Result<Option<String>, EstateError> {
    let out = git_checked(repo, &["worktree", "list", "--porcelain"])?;
    let mut path: Option<String> = None;
    for line in out.stdout.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            if let Some(p) = &path {
                if b == branch {
                    return Ok(Some(p.clone()));
                }
            }
        } else if line.is_empty() {
            path = None;
        }
    }
    Ok(None)
}

fn wt_dir_for(repo: &Path, branch: &str) -> PathBuf {
    let repo_name = repo
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    state_dir()
        .join("worktrees")
        .join(repo_name)
        .join(branch.replace('/', "-"))
}

/// The read-only half of `ensure_worktree`: validates the repo exists and
/// returns the path a real run would use, without running
/// `git worktree add`, `worktree prune`, or creating a branch — so
/// `--dry-run` can print it without mutating the repo.
pub fn would_ensure_worktree(repo_path: &str, branch: &str) -> Result<PathBuf, EstateError> {
    let repo = expand(repo_path);
    if !repo.is_dir() {
        return Err(EstateError::Config(format!(
            "worktree repo missing: {}",
            repo.display()
        )));
    }
    if let Some(existing) = existing_worktree(&repo, branch)? {
        return Ok(PathBuf::from(existing));
    }
    Ok(wt_dir_for(&repo, branch))
}

/// Idempotent: `git worktree add` only when `branch`'s worktree isn't
/// already registered. A new branch is cut from the remote default
/// branch when one exists, else from HEAD. Never removes a worktree.
pub fn ensure_worktree(repo_path: &str, branch: &str) -> Result<PathBuf, EstateError> {
    let repo = expand(repo_path);
    if !repo.is_dir() {
        return Err(EstateError::Config(format!(
            "worktree repo missing: {}",
            repo.display()
        )));
    }
    if let Some(existing) = existing_worktree(&repo, branch)? {
        return Ok(PathBuf::from(existing));
    }

    git_checked(&repo, &["worktree", "prune"])?;
    let branch_ref = format!("refs/heads/{branch}");
    let branch_exists = run_git(&repo, &["show-ref", "--verify", "--quiet", &branch_ref])?.ok;
    let wt_dir = wt_dir_for(&repo, branch);
    if let Some(parent) = wt_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            EstateError::Config(format!("could not create {}: {e}", parent.display()))
        })?;
    }
    let wt_str = wt_dir.to_string_lossy().to_string();
    if branch_exists {
        git_checked(&repo, &["worktree", "add", &wt_str, branch])?;
    } else {
        let base = format!("origin/{}", default_branch(&repo)?);
        let base_ok = run_git(&repo, &["rev-parse", "--verify", "--quiet", &base])?.ok;
        if base_ok {
            git_checked(&repo, &["worktree", "add", &wt_str, "-b", branch, &base])?;
        } else {
            git_checked(&repo, &["worktree", "add", &wt_str, "-b", branch])?;
        }
    }
    // git may register the worktree under its realpath (e.g. resolving a
    // symlinked temp dir): read the path back from git itself so a later
    // idempotent call returns the identical value.
    Ok(existing_worktree(&repo, branch)?
        .map(PathBuf::from)
        .unwrap_or(wt_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, TempDir};

    fn init_repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        run(&["add", "README.md"]);
        run(&["commit", "-m", "init"]);
    }

    struct Fixture {
        _tmp: TempDir,
        repo: PathBuf,
        _env: EnvGuard,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new();
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let state = tmp.path().join("state");
        let env = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", state.to_str().unwrap())]);
        Fixture {
            _tmp: tmp,
            repo,
            _env: env,
        }
    }

    #[test]
    fn new_branch_is_cut_from_the_real_origin_default_not_main() {
        // Every other test's repo has no `origin` remote, so
        // `default_branch`'s `symbolic-ref refs/remotes/origin/HEAD`
        // always misses and falls back to the literal "main". Here the
        // remote's default branch is "trunk", named "main" nowhere, and
        // the local checkout's *current* branch (a local-only "main",
        // diverged from trunk) carries content trunk never had — so a
        // new worktree branched from the wrong base would carry the
        // wrong marker file.
        let tmp = TempDir::new();
        let run = |dir: &Path, args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        };

        let bare = tmp.path().join("origin.git");
        run(
            tmp.path(),
            &["init", "--bare", "-b", "trunk", bare.to_str().unwrap()],
        );

        let seed = tmp.path().join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        run(&seed, &["init", "-b", "trunk"]);
        run(&seed, &["config", "user.email", "test@example.com"]);
        run(&seed, &["config", "user.name", "Test"]);
        std::fs::write(seed.join("marker-trunk.txt"), "trunk\n").unwrap();
        run(&seed, &["add", "marker-trunk.txt"]);
        run(&seed, &["commit", "-m", "init"]);
        run(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]);
        run(&seed, &["push", "origin", "trunk"]);

        let repo = tmp.path().join("repo");
        run(
            tmp.path(),
            &["clone", bare.to_str().unwrap(), repo.to_str().unwrap()],
        );
        // A local-only branch, diverged from trunk, left checked out —
        // the wrong base if `default_branch` fell back to its name.
        run(&repo, &["checkout", "-b", "main"]);
        run(&repo, &["config", "user.email", "test@example.com"]);
        run(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("marker-main-only.txt"), "main\n").unwrap();
        run(&repo, &["add", "marker-main-only.txt"]);
        run(&repo, &["commit", "-m", "local-only"]);

        let state = tmp.path().join("state");
        let _env = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", state.to_str().unwrap())]);
        let wt = ensure_worktree(repo.to_str().unwrap(), "wt/derived").unwrap();
        assert!(wt.join("marker-trunk.txt").is_file());
        assert!(!wt.join("marker-main-only.txt").exists());
    }

    #[test]
    fn creates_new_branch_worktree() {
        let f = fixture();
        let wt = ensure_worktree(f.repo.to_str().unwrap(), "wt/site-work").unwrap();
        assert!(wt.is_dir());
        assert!(wt.join("README.md").is_file());
        let out = Command::new("git")
            .arg("-C")
            .arg(&wt)
            .args(["branch", "--show-current"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "wt/site-work");
    }

    #[test]
    fn idempotent_second_call_returns_same_path() {
        let f = fixture();
        let first = ensure_worktree(f.repo.to_str().unwrap(), "wt/site-work").unwrap();
        let second = ensure_worktree(f.repo.to_str().unwrap(), "wt/site-work").unwrap();
        assert_eq!(first, second);
        let listing = Command::new("git")
            .arg("-C")
            .arg(&f.repo)
            .args(["worktree", "list"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&listing.stdout);
        assert_eq!(text.matches("wt/site-work").count(), 1);
    }

    #[test]
    fn reuses_existing_local_branch() {
        let f = fixture();
        let status = Command::new("git")
            .arg("-C")
            .arg(&f.repo)
            .args(["branch", "wt/existing"])
            .status()
            .unwrap();
        assert!(status.success());
        let wt = ensure_worktree(f.repo.to_str().unwrap(), "wt/existing").unwrap();
        assert!(wt.is_dir());
        let out = Command::new("git")
            .arg("-C")
            .arg(&f.repo)
            .args(["branch", "--list", "wt/existing"])
            .output()
            .unwrap();
        let count = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn missing_repo_raises() {
        let f = fixture();
        let nope = f.repo.parent().unwrap().join("nope");
        assert!(ensure_worktree(nope.to_str().unwrap(), "wt/x").is_err());
    }

    #[test]
    fn never_removes_a_worktree() {
        let f = fixture();
        let wt = ensure_worktree(f.repo.to_str().unwrap(), "wt/site-work").unwrap();
        std::fs::write(wt.join("scratch.txt"), "uncommitted work\n").unwrap();
        ensure_worktree(f.repo.to_str().unwrap(), "wt/site-work").unwrap();
        assert!(wt.join("scratch.txt").is_file());
    }

    fn snapshot(repo: &Path) -> (String, String) {
        let branches = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["branch", "--list"])
            .output()
            .unwrap();
        let worktrees = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "list"])
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&branches.stdout).to_string(),
            String::from_utf8_lossy(&worktrees.stdout).to_string(),
        )
    }

    #[test]
    fn would_ensure_new_branch_case_mutates_nothing() {
        let f = fixture();
        let before = snapshot(&f.repo);
        let path = would_ensure_worktree(f.repo.to_str().unwrap(), "wt/site-work").unwrap();
        let after = snapshot(&f.repo);
        assert_eq!(before, after);
        assert!(!path.exists());
    }

    #[test]
    fn would_ensure_existing_worktree_case_is_read_only() {
        let f = fixture();
        let real = ensure_worktree(f.repo.to_str().unwrap(), "wt/site-work").unwrap();
        let before = snapshot(&f.repo);
        let path = would_ensure_worktree(f.repo.to_str().unwrap(), "wt/site-work").unwrap();
        let after = snapshot(&f.repo);
        assert_eq!(before, after);
        assert_eq!(path, real);
    }

    #[test]
    fn would_ensure_missing_repo_raises() {
        let f = fixture();
        let nope = f.repo.parent().unwrap().join("nope");
        assert!(would_ensure_worktree(nope.to_str().unwrap(), "wt/x").is_err());
    }
}
