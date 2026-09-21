import tempfile
import unittest
from pathlib import Path
from unittest import mock

from herdr_estate.errors import ConfigError
from herdr_estate.prompt import prompt_path, write_prompt


class PromptPath(unittest.TestCase):
    def test_empty_list_is_none(self):
        self.assertIsNone(prompt_path("home", "garden", []))

    def test_no_io_for_path_only(self):
        with tempfile.TemporaryDirectory() as d:
            with mock.patch.dict("os.environ", {"HERDR_PLUGIN_STATE_DIR": d}):
                p = prompt_path("home", "garden", [Path("/does/not/exist.md")])
            self.assertFalse(p.exists())
            self.assertEqual(p.name, "home-garden.prompt.md")


class WritePrompt(unittest.TestCase):
    def test_concatenates_in_order(self):
        with tempfile.TemporaryDirectory() as d:
            base = Path(d) / "base.md"
            harness = Path(d) / "harness.md"
            base.write_text("BASE\n")
            harness.write_text("HARNESS\n")
            with mock.patch.dict("os.environ", {"HERDR_PLUGIN_STATE_DIR": d}):
                out = write_prompt("home", "garden", [base, harness])
            text = out.read_text()
            self.assertLess(text.index("BASE"), text.index("HARNESS"))

    def test_missing_prompt_file_raises(self):
        with tempfile.TemporaryDirectory() as d:
            with mock.patch.dict("os.environ", {"HERDR_PLUGIN_STATE_DIR": d}):
                with self.assertRaises(ConfigError):
                    write_prompt("home", "garden", [Path(d) / "missing.md"])

    def test_no_prompt_files_returns_none(self):
        self.assertIsNone(write_prompt("home", "garden", []))


if __name__ == "__main__":
    unittest.main()
