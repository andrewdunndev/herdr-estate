import os
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

    def test_estate_paths_precede_campaign_paths(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden", "---\npaths: [/b]\n---\n")
            estates = {"home": {"root": str(root), "paths": ["/a"]}}
            c = resolve(estates, "home/garden")
            self.assertEqual([str(p) for p in c.add_dirs], ["/a", "/b"])

    def test_duplicate_estate_and_campaign_path_collapses(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden", "---\npaths: [~/code, /b]\n---\n")
            estates = {"home": {"root": str(root), "paths": ["~/code"]}}
            c = resolve(estates, "home/garden")
            self.assertEqual([str(p) for p in c.add_dirs],
                              [str(Path.home() / "code"), "/b"])

    def test_estate_kind_default_appears_in_flags(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden")
            estates = {"home": {"root": str(root),
                                 "claude": {"remote-control": True}}}
            c = resolve(estates, "home/garden")
            self.assertEqual(c.kind_flags, {"remote-control": True})

    def test_campaign_kind_flag_overrides_estate_default(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden",
                         "---\nclaude:\n  mcp-config: campaign.json\n---\n")
            estates = {"home": {"root": str(root),
                                 "claude": {"mcp-config": "estate.json"}}}
            c = resolve(estates, "home/garden")
            self.assertEqual(c.kind_flags, {"mcp-config": "campaign.json"})

    def test_campaign_false_turns_off_estate_kind_default(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden",
                         "---\nclaude:\n  remote-control: false\n---\n")
            estates = {"home": {"root": str(root),
                                 "claude": {"remote-control": True}}}
            c = resolve(estates, "home/garden")
            self.assertEqual(c.kind_flags, {"remote-control": False})

    def test_estate_with_neither_key_matches_prior_behavior(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden")
            estates = {"home": {"root": str(root), "agent": "claude"}}
            c = resolve(estates, "home/garden")
            self.assertEqual(c.add_dirs, [])
            self.assertEqual(c.kind_flags, {})

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

    def test_dry_run_uses_would_ensure_worktree_not_ensure_worktree(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden",
                         "---\nworktree:\n  repo: /some/repo\n"
                         "  branch: wt/site-work\n---\n")
            estates = {"home": {"root": str(root)}}
            wt_path = Path(d) / "the-worktree"
            with mock.patch("herdr_estate.campaign.worktree_mod.ensure_worktree") as ensure, \
                 mock.patch("herdr_estate.campaign.worktree_mod.would_ensure_worktree",
                             return_value=wt_path) as would_ensure:
                c = resolve(estates, "home/garden", dry_run=True)
            ensure.assert_not_called()
            would_ensure.assert_called_once_with("/some/repo", "wt/site-work")
            self.assertEqual(c.workdir, wt_path)
            self.assertIn(wt_path, c.add_dirs)


class RelativePathsRootAgainstEstate(unittest.TestCase):
    def test_relative_prompts_resolve_against_root_not_cwd(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden")
            estates = {"home": {"root": str(root), "prompts": ["prompts/base.md"]}}
            elsewhere = Path(d)
            cwd = os.getcwd()
            os.chdir(elsewhere)
            try:
                c = resolve(estates, "home/garden")
            finally:
                os.chdir(cwd)
            self.assertEqual(c.prompt_files, [root / "prompts" / "base.md"])

    def test_absolute_prompts_pass_through(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden")
            estates = {"home": {"root": str(root), "prompts": ["/etc/base.md"]}}
            c = resolve(estates, "home/garden")
            self.assertEqual(c.prompt_files, [Path("/etc/base.md")])

    def test_relative_campaign_paths_resolve_against_root_not_cwd(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden", "---\npaths: [notes]\n---\n")
            estates = {"home": {"root": str(root)}}
            cwd = os.getcwd()
            os.chdir(Path(d))
            try:
                c = resolve(estates, "home/garden")
            finally:
                os.chdir(cwd)
            self.assertEqual(c.add_dirs, [root / "notes"])

    def test_absolute_campaign_paths_pass_through(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_estate(root, "garden", "---\npaths: [/a]\n---\n")
            estates = {"home": {"root": str(root)}}
            c = resolve(estates, "home/garden")
            self.assertEqual(c.add_dirs, [Path("/a")])


if __name__ == "__main__":
    unittest.main()
