import json
import subprocess
import unittest
from unittest import mock

from herdr_estate.herdrcli import HerdrError, call_herdr, scrub_env


class ScrubEnv(unittest.TestCase):
    def test_drops_anthropic_api_key(self):
        env = scrub_env({"ANTHROPIC_API_KEY": "sk-x", "PATH": "/bin"})
        self.assertNotIn("ANTHROPIC_API_KEY", env)
        self.assertIn("PATH", env)

    def test_drops_tmux_prefixed(self):
        env = scrub_env({"TMUX": "1", "TMUX_PANE": "%3", "PATH": "/bin"})
        self.assertNotIn("TMUX", env)
        self.assertNotIn("TMUX_PANE", env)

    def test_drops_claude_prefixed(self):
        env = scrub_env({"CLAUDECODE": "1", "CLAUDE_FOO": "x", "PATH": "/bin"})
        self.assertNotIn("CLAUDECODE", env)
        self.assertNotIn("CLAUDE_FOO", env)

    def test_control_socket_rides_through_untouched(self):
        env = scrub_env({"HERDR_SOCKET_PATH": "/tmp/sock",
                          "HERDR_WORKSPACE_ID": "w1",
                          "ANTHROPIC_API_KEY": "sk-x"})
        self.assertEqual(env["HERDR_SOCKET_PATH"], "/tmp/sock")
        self.assertEqual(env["HERDR_WORKSPACE_ID"], "w1")


def _completed(returncode=0, stdout="", stderr=""):
    return subprocess.CompletedProcess(args=["herdr"], returncode=returncode,
                                        stdout=stdout, stderr=stderr)


class CallHerdr(unittest.TestCase):
    def test_uses_herdr_bin_path(self):
        with mock.patch.dict("os.environ", {"HERDR_BIN_PATH": "/opt/herdr/herdr"}):
            with mock.patch("herdr_estate.herdrcli.subprocess.run") as run:
                run.return_value = _completed(stdout='{"result": {}}')
                call_herdr(["--version"])
                called_argv = run.call_args.args[0]
                self.assertEqual(called_argv[0], "/opt/herdr/herdr")

    def test_parses_result_envelope(self):
        payload = json.dumps({"result": {"workspace": {"label": "home/garden"}}})
        with mock.patch("herdr_estate.herdrcli.subprocess.run") as run:
            run.return_value = _completed(stdout=payload)
            result = call_herdr(["workspace", "get", "w1"])
        self.assertTrue(result.ok)
        self.assertEqual(result.data["workspace"]["label"], "home/garden")

    def test_check_true_raises_on_failure(self):
        with mock.patch("herdr_estate.herdrcli.subprocess.run") as run:
            run.return_value = _completed(returncode=1, stderr="boom")
            with self.assertRaises(HerdrError):
                call_herdr(["workspace", "close", "w1"])

    def test_check_false_reports_failure_without_raising(self):
        with mock.patch("herdr_estate.herdrcli.subprocess.run") as run:
            run.return_value = _completed(returncode=1, stderr="boom")
            result = call_herdr(["workspace", "get", "w1"], check=False)
        self.assertFalse(result.ok)
        self.assertEqual(result.data, {})

    def test_malformed_json_yields_empty_data(self):
        with mock.patch("herdr_estate.herdrcli.subprocess.run") as run:
            run.return_value = _completed(stdout="not json")
            result = call_herdr(["workspace", "list"])
        self.assertTrue(result.ok)
        self.assertEqual(result.data, {})

    def test_scrubbed_env_passed_to_subprocess(self):
        with mock.patch.dict("os.environ", {"ANTHROPIC_API_KEY": "sk-x",
                                             "HERDR_SOCKET_PATH": "/tmp/sock"}):
            with mock.patch("herdr_estate.herdrcli.subprocess.run") as run:
                run.return_value = _completed(stdout='{"result": {}}')
                call_herdr(["--version"])
                passed_env = run.call_args.kwargs["env"]
        self.assertNotIn("ANTHROPIC_API_KEY", passed_env)
        self.assertEqual(passed_env["HERDR_SOCKET_PATH"], "/tmp/sock")


if __name__ == "__main__":
    unittest.main()
