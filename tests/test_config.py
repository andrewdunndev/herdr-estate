import tempfile
import unittest
from pathlib import Path
from unittest import mock

from herdr_estate.config import config_path, load_estates, parse_address
from herdr_estate.errors import ConfigError

ESTATES_TOML = """
[estate.home]
root = "~/estates/home"
campaigns = "campaigns"
agent = "claude"
prompts = ["prompts/base.md"]

[estate.home.env]
GREETING = "hi"
"""


class ParseAddress(unittest.TestCase):
    def test_splits_on_first_slash(self):
        self.assertEqual(parse_address("home/garden"), ("home", "garden"))

    def test_extra_slashes_go_to_campaign(self):
        self.assertEqual(parse_address("home/garden/sub"), ("home", "garden/sub"))

    def test_missing_slash_rejected(self):
        with self.assertRaises(ConfigError):
            parse_address("home")

    def test_empty_estate_rejected(self):
        with self.assertRaises(ConfigError):
            parse_address("/garden")

    def test_empty_campaign_rejected(self):
        with self.assertRaises(ConfigError):
            parse_address("home/")


class LoadEstates(unittest.TestCase):
    def test_loads_estate_table(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "estates.toml"
            p.write_text(ESTATES_TOML)
            estates = load_estates(p)
            self.assertEqual(estates["home"]["root"], "~/estates/home")
            self.assertEqual(estates["home"]["prompts"], ["prompts/base.md"])
            self.assertEqual(estates["home"]["env"], {"GREETING": "hi"})

    def test_missing_file_raises(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(ConfigError):
                load_estates(Path(d) / "nope.toml")


class ConfigPathResolution(unittest.TestCase):
    def test_uses_plugin_config_dir_when_file_exists(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "estates.toml"
            p.write_text(ESTATES_TOML)
            with mock.patch.dict("os.environ", {"HERDR_PLUGIN_CONFIG_DIR": d}):
                self.assertEqual(config_path(), p)

    def test_falls_back_when_plugin_config_dir_file_missing(self):
        with tempfile.TemporaryDirectory() as d, tempfile.TemporaryDirectory() as home:
            with mock.patch.dict("os.environ",
                                  {"HERDR_PLUGIN_CONFIG_DIR": d, "HOME": home}):
                self.assertEqual(config_path(),
                                  Path(home) / ".config" / "herdr-estate" / "estates.toml")

    def test_falls_back_when_plugin_config_dir_unset(self):
        with tempfile.TemporaryDirectory() as home:
            with mock.patch.dict("os.environ", {"HOME": home}, clear=False):
                import os
                os.environ.pop("HERDR_PLUGIN_CONFIG_DIR", None)
                self.assertEqual(config_path(),
                                  Path(home) / ".config" / "herdr-estate" / "estates.toml")


if __name__ == "__main__":
    unittest.main()
