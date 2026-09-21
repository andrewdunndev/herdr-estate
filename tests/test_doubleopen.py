import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from herdr_estate.doubleopen import (live_workspace, registry_holder,
                                      transcript_exists, workspace_agent_running)
from herdr_estate.herdrcli import HerdrResult


class RegistryHolder(unittest.TestCase):
    def _home_with_session(self, session_id, pid, name="andrew"):
        home = tempfile.TemporaryDirectory()
        sessions = Path(home.name) / ".claude" / "sessions"
        sessions.mkdir(parents=True)
        (sessions / f"{pid}.json").write_text(
            json.dumps({"sessionId": session_id, "name": name}))
        return home

    def test_live_pid_is_a_holder(self):
        home = self._home_with_session("sid-1", os.getpid())
        with home, mock.patch.dict("os.environ", {"HOME": home.name}):
            holder = registry_holder("sid-1")
        self.assertIsNotNone(holder)
        self.assertIn(str(os.getpid()), holder)

    def test_dead_pid_is_not_a_holder(self):
        home = self._home_with_session("sid-1", 999999)
        with home, mock.patch.dict("os.environ", {"HOME": home.name}):
            with mock.patch("herdr_estate.doubleopen.os.kill",
                             side_effect=ProcessLookupError):
                holder = registry_holder("sid-1")
        self.assertIsNone(holder)

    def test_no_match_is_none(self):
        home = self._home_with_session("sid-other", os.getpid())
        with home, mock.patch.dict("os.environ", {"HOME": home.name}):
            self.assertIsNone(registry_holder("sid-1"))

    def test_missing_sessions_dir_is_none(self):
        with tempfile.TemporaryDirectory() as home:
            with mock.patch.dict("os.environ", {"HOME": home}):
                self.assertIsNone(registry_holder("sid-1"))


class TranscriptExists(unittest.TestCase):
    def test_transcript_found(self):
        with tempfile.TemporaryDirectory() as home:
            proj = Path(home) / ".claude" / "projects" / "some-project"
            proj.mkdir(parents=True)
            (proj / "sid-1.jsonl").write_text("{}\n")
            with mock.patch.dict("os.environ", {"HOME": home}):
                self.assertTrue(transcript_exists("sid-1"))

    def test_transcript_missing(self):
        with tempfile.TemporaryDirectory() as home:
            with mock.patch.dict("os.environ", {"HOME": home}):
                self.assertFalse(transcript_exists("sid-1"))


class LiveWorkspace(unittest.TestCase):
    def test_finds_matching_label(self):
        data = {"workspaces": [{"label": "home/taxes", "workspace_id": "w1"},
                                {"label": "home/garden", "workspace_id": "w2"}]}
        with mock.patch("herdr_estate.doubleopen.herdrcli.call_herdr") as call:
            call.return_value = HerdrResult(ok=True, returncode=0, data=data)
            ws = live_workspace("home/garden")
        self.assertEqual(ws["workspace_id"], "w2")

    def test_no_match_is_none(self):
        data = {"workspaces": [{"label": "home/taxes", "workspace_id": "w1"}]}
        with mock.patch("herdr_estate.doubleopen.herdrcli.call_herdr") as call:
            call.return_value = HerdrResult(ok=True, returncode=0, data=data)
            self.assertIsNone(live_workspace("home/garden"))

    def test_probe_failure_is_none(self):
        with mock.patch("herdr_estate.doubleopen.herdrcli.call_herdr") as call:
            call.return_value = HerdrResult(ok=False, returncode=1)
            self.assertIsNone(live_workspace("home/garden"))


class WorkspaceAgentRunning(unittest.TestCase):
    def test_running_agent_found(self):
        data = {"agents": [{"workspace_id": "w2"}]}
        with mock.patch("herdr_estate.doubleopen.herdrcli.call_herdr") as call:
            call.return_value = HerdrResult(ok=True, returncode=0, data=data)
            self.assertTrue(workspace_agent_running("w2"))

    def test_no_agent_for_workspace(self):
        data = {"agents": [{"workspace_id": "w1"}]}
        with mock.patch("herdr_estate.doubleopen.herdrcli.call_herdr") as call:
            call.return_value = HerdrResult(ok=True, returncode=0, data=data)
            self.assertFalse(workspace_agent_running("w2"))

    def test_probe_failure_is_false(self):
        with mock.patch("herdr_estate.doubleopen.herdrcli.call_herdr") as call:
            call.return_value = HerdrResult(ok=False, returncode=1)
            self.assertFalse(workspace_agent_running("w2"))


if __name__ == "__main__":
    unittest.main()
