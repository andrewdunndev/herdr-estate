import tempfile
import unittest
from pathlib import Path

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


if __name__ == "__main__":
    unittest.main()
