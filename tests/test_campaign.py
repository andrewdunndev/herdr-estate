import tempfile
import unittest
from pathlib import Path
from unittest import mock

from herdr_estate.campaign import resolve
from herdr_estate.errors import ConfigError


def _make_estate(root: Path, campaign: str, frontmatter: str = "---\n---\n"):
    cdir = root / "campaigns" / campaign
    cdir.mkdir(parents=True)
    (cdir / "campaign.md").write_text(frontmatter)
    return cdir


class ResolveCampaign(unittest.TestCase):
    def test_basic_resolution(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden")
            estates = {"home": {"root": str(root), "agent": "claude"}}
            c = resolve(estates, "home/garden")
            self.assertEqual(c.estate, "home")
            self.assertEqual(c.name, "garden")
            self.assertEqual(c.kind, "claude")
            self.assertEqual(c.workdir, root)
            self.assertEqual(c.add_dirs, [])
            self.assertEqual(c.kind_flags, {})

    def test_unknown_estate_raises(self):
        with self.assertRaises(ConfigError):
            resolve({}, "nope/garden")

    def test_uncloned_root_raises(self):
        estates = {"home": {"root": "/nonexistent-estate-root-xyz"}}
        with self.assertRaises(ConfigError):
            resolve(estates, "home/garden")

    def test_missing_campaign_md_raises(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            (root / "campaigns").mkdir(parents=True)
            estates = {"home": {"root": str(root)}}
            with self.assertRaises(ConfigError):
                resolve(estates, "home/garden")

    def test_campaign_overrides_agent(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden", "---\nagent: codex\n---\n")
            estates = {"home": {"root": str(root), "agent": "claude"}}
            c = resolve(estates, "home/garden")
            self.assertEqual(c.kind, "codex")

    def test_paths_expanded(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden", "---\npaths: [/a, /b]\n---\n")
            estates = {"home": {"root": str(root)}}
            c = resolve(estates, "home/garden")
            self.assertEqual([str(p) for p in c.add_dirs], ["/a", "/b"])

    def test_kind_flags_selected_by_active_kind_only(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden",
                         "---\nclaude:\n  remote-control: true\n"
                         "codex:\n  some-flag: 1\n---\n")
            estates = {"home": {"root": str(root), "agent": "claude"}}
            c = resolve(estates, "home/garden")
            self.assertEqual(c.kind_flags, {"remote-control": True})

    def test_estate_env_carried_through(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden")
            estates = {"home": {"root": str(root), "env": {"K": "V"}}}
            c = resolve(estates, "home/garden")
            self.assertEqual(c.env, {"K": "V"})

    def test_no_worktree_key_uses_estate_root(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden")
            estates = {"home": {"root": str(root)}}
            with mock.patch("herdr_estate.campaign.worktree_mod.ensure_worktree") as ensure:
                c = resolve(estates, "home/garden")
            ensure.assert_not_called()
            self.assertEqual(c.workdir, root)

    def test_worktree_key_ensures_and_becomes_workdir(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden",
                         "---\nworktree:\n  repo: /some/repo\n"
                         "  branch: wt/site-work\n---\n")
            estates = {"home": {"root": str(root)}}
            wt_path = Path(d) / "the-worktree"
            with mock.patch("herdr_estate.campaign.worktree_mod.ensure_worktree",
                             return_value=wt_path) as ensure:
                c = resolve(estates, "home/garden")
            ensure.assert_called_once_with("/some/repo", "wt/site-work")
            self.assertEqual(c.workdir, wt_path)
            self.assertIn(wt_path, c.add_dirs)


if __name__ == "__main__":
    unittest.main()
