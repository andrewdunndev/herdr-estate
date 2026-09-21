import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from herdr_estate.errors import ConfigError
from herdr_estate.worktree import ensure_worktree, would_ensure_worktree


def _run(*args, cwd):
    subprocess.run(list(args), cwd=cwd, check=True, capture_output=True, text=True)


def _init_repo(path: Path) -> None:
    path.mkdir(parents=True)
    _run("git", "init", "-b", "main", cwd=path)
    _run("git", "config", "user.email", "test@example.com", cwd=path)
    _run("git", "config", "user.name", "Test", cwd=path)
    (path / "README.md").write_text("hello\n")
    _run("git", "add", "README.md", cwd=path)
    _run("git", "commit", "-m", "init", cwd=path)


class EnsureWorktree(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = Path(self._tmp.name) / "repo"
        _init_repo(self.repo)
        self.state = tempfile.TemporaryDirectory()
        self.addCleanup(self.state.cleanup)
        self._env_patch = mock.patch.dict(
            "os.environ", {"HERDR_PLUGIN_STATE_DIR": self.state.name})
        self._env_patch.start()
        self.addCleanup(self._env_patch.stop)

    def test_creates_new_branch_worktree(self):
        wt = ensure_worktree(str(self.repo), "wt/site-work")
        self.assertTrue(wt.is_dir())
        self.assertTrue((wt / "README.md").is_file())
        branch = subprocess.run(["git", "-C", str(wt), "branch", "--show-current"],
                                 capture_output=True, text=True, check=True).stdout.strip()
        self.assertEqual(branch, "wt/site-work")

    def test_idempotent_second_call_returns_same_path(self):
        first = ensure_worktree(str(self.repo), "wt/site-work")
        second = ensure_worktree(str(self.repo), "wt/site-work")
        self.assertEqual(first, second)
        listing = subprocess.run(["git", "-C", str(self.repo), "worktree", "list"],
                                  capture_output=True, text=True, check=True).stdout
        self.assertEqual(listing.count("wt/site-work"), 1)

    def test_reuses_existing_local_branch(self):
        _run("git", "branch", "wt/existing", cwd=self.repo)
        wt = ensure_worktree(str(self.repo), "wt/existing")
        self.assertTrue(wt.is_dir())
        # exactly one branch named wt/existing — worktree add did not
        # create a second one
        out = subprocess.run(["git", "-C", str(self.repo), "branch", "--list",
                               "wt/existing"], capture_output=True, text=True,
                              check=True).stdout
        self.assertEqual(len([ln for ln in out.splitlines() if ln.strip()]), 1)

    def test_missing_repo_raises(self):
        with self.assertRaises(ConfigError):
            ensure_worktree(str(Path(self._tmp.name) / "nope"), "wt/x")

    def test_never_removes_a_worktree(self):
        wt = ensure_worktree(str(self.repo), "wt/site-work")
        (wt / "scratch.txt").write_text("uncommitted work\n")
        ensure_worktree(str(self.repo), "wt/site-work")
        self.assertTrue((wt / "scratch.txt").is_file())


class WouldEnsureWorktree(unittest.TestCase):
    """--dry-run must not mutate git — would_ensure_worktree is the
    read-only half ensure_worktree calls into for the case that already
    has one, but never itself runs `git worktree add`, `worktree prune`,
    or creates a branch."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = Path(self._tmp.name) / "repo"
        _init_repo(self.repo)
        self.state = tempfile.TemporaryDirectory()
        self.addCleanup(self.state.cleanup)
        self._env_patch = mock.patch.dict(
            "os.environ", {"HERDR_PLUGIN_STATE_DIR": self.state.name})
        self._env_patch.start()
        self.addCleanup(self._env_patch.stop)

    def _snapshot(self):
        branches = subprocess.run(["git", "-C", str(self.repo), "branch", "--list"],
                                   capture_output=True, text=True, check=True).stdout
        worktrees = subprocess.run(["git", "-C", str(self.repo), "worktree", "list"],
                                    capture_output=True, text=True, check=True).stdout
        return branches, worktrees

    def test_new_branch_case_mutates_nothing(self):
        before = self._snapshot()
        path = would_ensure_worktree(str(self.repo), "wt/site-work")
        after = self._snapshot()
        self.assertEqual(before, after)
        self.assertFalse(path.exists())  # never created

    def test_existing_worktree_case_returns_the_real_path_read_only(self):
        real = ensure_worktree(str(self.repo), "wt/site-work")
        before = self._snapshot()
        path = would_ensure_worktree(str(self.repo), "wt/site-work")
        after = self._snapshot()
        self.assertEqual(before, after)
        self.assertEqual(path, real)

    def test_missing_repo_raises(self):
        with self.assertRaises(ConfigError):
            would_ensure_worktree(str(Path(self._tmp.name) / "nope"), "wt/x")


if __name__ == "__main__":
    unittest.main()
