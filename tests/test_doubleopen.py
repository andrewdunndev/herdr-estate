import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from herdr_estate import ownership
from herdr_estate.doubleopen import (live_workspace, registry_holder,
                                      transcript_exists, workspace_alive)
from herdr_estate.errors import ProtocolError
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
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self._patch = mock.patch.dict(
            "os.environ", {"HERDR_PLUGIN_STATE_DIR": self._tmp.name})
        self._patch.start()
        self.addCleanup(self._patch.stop)

    def test_finds_matching_label_via_workspace_list(self):
        data = {"workspaces": [{"label": "home/taxes", "workspace_id": "w1"},
                                {"label": "home/garden", "workspace_id": "w2"}]}
        with mock.patch("herdr_estate.doubleopen.herdrcli.call_herdr") as call:
            call.return_value = HerdrResult(ok=True, returncode=0, data=data)
            ws = live_workspace("home/garden")
        self.assertEqual(ws["workspace_id"], "w2")
        # exact argv sent to the herdr boundary
        call.assert_called_once_with(["workspace", "list"])

    def test_no_match_is_none(self):
        data = {"workspaces": [{"label": "home/taxes", "workspace_id": "w1"}]}
        with mock.patch("herdr_estate.doubleopen.herdrcli.call_herdr") as call:
            call.return_value = HerdrResult(ok=True, returncode=0, data=data)
            self.assertIsNone(live_workspace("home/garden"))
        call.assert_called_once_with(["workspace", "list"])

    def test_missing_workspaces_array_raises(self):
        """A 0 exit whose stdout didn't parse into JSON leaves
        HerdrResult.data == {} (herdrcli.call_herdr's own behaviour on
        malformed JSON) — that must stop the verb, not read as zero
        workspaces."""
        with mock.patch("herdr_estate.doubleopen.herdrcli.call_herdr") as call:
            call.return_value = HerdrResult(ok=True, returncode=0, data={})
            with self.assertRaises(ProtocolError):
                live_workspace("home/garden")

    def test_prunes_ownership_records_not_in_the_listing(self):
        ownership.record("home/gone", "stale-id")
        ownership.record("home/garden", "w2")
        data = {"workspaces": [{"label": "home/garden", "workspace_id": "w2",
                                 "agent_status": "unknown"}]}
        with mock.patch("herdr_estate.doubleopen.herdrcli.call_herdr") as call:
            call.return_value = HerdrResult(ok=True, returncode=0, data=data)
            live_workspace("home/garden")
        self.assertFalse(ownership.is_ours("stale-id"))
        self.assertTrue(ownership.is_ours("w2"))


class WorkspaceAlive(unittest.TestCase):
    """Reads agent_status off the row live_workspace already fetched —
    no separate herdr call. Fails closed: only exactly "unknown" is not
    alive."""

    def test_unknown_is_not_alive(self):
        self.assertFalse(workspace_alive({"agent_status": "unknown"}))

    def test_working_is_alive(self):
        self.assertTrue(workspace_alive({"agent_status": "working"}))

    def test_idle_is_alive(self):
        self.assertTrue(workspace_alive({"agent_status": "idle"}))

    def test_done_is_alive(self):
        self.assertTrue(workspace_alive({"agent_status": "done"}))

    def test_missing_key_is_alive(self):
        self.assertTrue(workspace_alive({}))

    def test_null_value_is_alive(self):
        self.assertTrue(workspace_alive({"agent_status": None}))

    def test_unrecognised_value_is_alive(self):
        self.assertTrue(workspace_alive({"agent_status": "something-new"}))


if __name__ == "__main__":
    unittest.main()
