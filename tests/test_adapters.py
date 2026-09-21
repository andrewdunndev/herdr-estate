import io
import unittest
from contextlib import redirect_stderr
from pathlib import Path

from herdr_estate.adapters import build_argv, claude_argv
from herdr_estate.campaign import Campaign


def _campaign(**overrides) -> Campaign:
    base = dict(address="home/garden", estate="home", name="garden",
                dir=Path("/tmp/garden"), workdir=Path("/tmp/home"),
                kind="claude", add_dirs=[], prompt_files=[], kind_flags={},
                env={})
    base.update(overrides)
    return Campaign(**base)


class ClaudeArgv(unittest.TestCase):
    def test_resume_uses_resume_flag(self):
        argv = claude_argv(_campaign(), verb="resume", conversation_id="SID",
                            prompt_file=None)
        self.assertIn("--resume", argv)
        self.assertEqual(argv[argv.index("--resume") + 1], "SID")
        self.assertNotIn("--session-id", argv)

    def test_fresh_uses_session_id_flag(self):
        argv = claude_argv(_campaign(), verb="fresh", conversation_id="NEWSID",
                            prompt_file=None)
        self.assertIn("--session-id", argv)
        self.assertEqual(argv[argv.index("--session-id") + 1], "NEWSID")
        self.assertNotIn("--resume", argv)

    def test_name_is_address(self):
        argv = claude_argv(_campaign(address="home/taxes"), verb="fresh",
                            conversation_id="S", prompt_file=None)
        self.assertEqual(argv[argv.index("--name") + 1], "home/taxes")

    def test_add_dir_per_path(self):
        c = _campaign(add_dirs=[Path("/a"), Path("/b")])
        argv = claude_argv(c, verb="fresh", conversation_id="S", prompt_file=None)
        self.assertEqual(argv.count("--add-dir"), 2)
        self.assertIn("/a", argv)
        self.assertIn("/b", argv)

    def test_prompt_file_appended(self):
        argv = claude_argv(_campaign(), verb="fresh", conversation_id="S",
                            prompt_file=Path("/tmp/x.prompt.md"))
        self.assertIn("--append-system-prompt-file", argv)
        self.assertEqual(argv[argv.index("--append-system-prompt-file") + 1],
                          "/tmp/x.prompt.md")

    def test_no_prompt_file_omitted(self):
        argv = claude_argv(_campaign(), verb="fresh", conversation_id="S",
                            prompt_file=None)
        self.assertNotIn("--append-system-prompt-file", argv)

    def test_kind_flag_true_is_bare_flag(self):
        c = _campaign(kind_flags={"remote-control": True})
        argv = claude_argv(c, verb="fresh", conversation_id="S", prompt_file=None)
        self.assertIn("--remote-control", argv)

    def test_kind_flag_value_takes_argument(self):
        c = _campaign(kind_flags={"mcp-config": ".mcp.json"})
        argv = claude_argv(c, verb="fresh", conversation_id="S", prompt_file=None)
        self.assertEqual(argv[argv.index("--mcp-config") + 1], ".mcp.json")

    def test_kind_flag_false_is_omitted(self):
        c = _campaign(kind_flags={"remote-control": False})
        argv = claude_argv(c, verb="fresh", conversation_id="S", prompt_file=None)
        self.assertNotIn("--remote-control", argv)


class UnknownKind(unittest.TestCase):
    def test_no_adapter_warns_and_returns_bare_kind(self):
        c = _campaign(kind="opencode")
        stderr = io.StringIO()
        with redirect_stderr(stderr):
            argv = build_argv(c, verb="fresh", conversation_id="S", prompt_file=None)
        self.assertEqual(argv, ["opencode"])
        self.assertIn("no adapter", stderr.getvalue())

    def test_claude_kind_uses_claude_adapter(self):
        c = _campaign(kind="claude")
        argv = build_argv(c, verb="resume", conversation_id="S", prompt_file=None)
        self.assertEqual(argv[0], "claude")


if __name__ == "__main__":
    unittest.main()
