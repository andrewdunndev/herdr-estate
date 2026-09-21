import io
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

from herdr_estate import continuity, verbs
from herdr_estate.errors import GuardError, UsageError
from herdr_estate.herdrcli import HerdrResult

CREATE_OK = HerdrResult(ok=True, returncode=0, data={
    "workspace": {"workspace_id": "w9", "label": "home/garden"},
    "root_pane": {"pane_id": "p1"},
})


def _estate(root: Path) -> dict:
    return {"home": {"root": str(root), "agent": "claude"}}


def _make_campaign_dir(root: Path, name: str) -> Path:
    cdir = root / "campaigns" / name
    cdir.mkdir(parents=True)
    (cdir / "campaign.md").write_text("---\n---\n")
    return cdir


class AgentName(unittest.TestCase):
    def test_sanitizes_and_lowercases(self):
        self.assertEqual(verbs.agent_name("Home/Garden"), "home-garden")

    def test_truncates_to_32(self):
        name = verbs.agent_name("home/" + "x" * 40)
        self.assertLessEqual(len(name), 32)

    def test_leading_digit_gets_prefixed(self):
        self.assertTrue(verbs.agent_name("9lives/x")[0].isalpha())


class CommandShapes(unittest.TestCase):
    def _campaign(self):
        from herdr_estate.campaign import Campaign
        return Campaign(address="home/garden", estate="home", name="garden",
                         dir=Path("/tmp/c"), workdir=Path("/tmp/w"), kind="claude",
                         add_dirs=[], prompt_files=[], kind_flags={},
                         env={"K": "V"})

    def test_create_args_has_label_and_env(self):
        args = verbs.create_args(self._campaign())
        self.assertIn("--label", args)
        self.assertEqual(args[args.index("--label") + 1], "home/garden")
        self.assertIn("HERDR_ESTATE_ADDRESS=home/garden", args)
        self.assertIn("K=V", args)

    def test_start_args_shape(self):
        args = verbs.start_args(self._campaign(), "p1", ["--resume", "sid"])
        self.assertEqual(args[:2], ["agent", "start"])
        self.assertIn("--kind", args)
        self.assertEqual(args[args.index("--kind") + 1], "claude")
        self.assertIn("--pane", args)
        self.assertEqual(args[args.index("--pane") + 1], "p1")
        self.assertEqual(args[-2:], ["--resume", "sid"])


class DeriveAddress(unittest.TestCase):
    def test_missing_workspace_id_raises(self):
        with mock.patch.dict("os.environ", {}, clear=False):
            import os
            os.environ.pop("HERDR_WORKSPACE_ID", None)
            with self.assertRaises(UsageError):
                verbs.derive_address()

    def test_reads_label_from_workspace_get(self):
        result = HerdrResult(ok=True, returncode=0,
                              data={"workspace": {"label": "home/garden"}})
        with mock.patch.dict("os.environ", {"HERDR_WORKSPACE_ID": "w1"}):
            with mock.patch("herdr_estate.verbs.herdrcli.call_herdr",
                             return_value=result):
                self.assertEqual(verbs.derive_address(), "home/garden")


class CloseIfOurs(unittest.TestCase):
    def test_closes_on_label_match(self):
        get_result = HerdrResult(ok=True, returncode=0,
                                  data={"workspace": {"label": "home/garden"}})
        with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
            call.return_value = get_result
            verbs.close_if_ours("w1", "home/garden")
        self.assertEqual(call.call_count, 2)
        close_call_args = call.call_args_list[1].args[0]
        self.assertEqual(close_call_args, ["workspace", "close", "w1"])

    def test_skips_on_label_mismatch(self):
        get_result = HerdrResult(ok=True, returncode=0,
                                  data={"workspace": {"label": "someone-elses"}})
        with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
            call.return_value = get_result
            verbs.close_if_ours("w1", "home/garden")
        self.assertEqual(call.call_count, 1)  # only the get, never the close


def _no_guard_blocks():
    """Patch the double-open guard functions so nothing refuses."""
    return mock.patch.multiple(
        "herdr_estate.verbs.doubleopen",
        transcript_exists=mock.DEFAULT,
        registry_holder=mock.DEFAULT,
        live_workspace=mock.DEFAULT,
        workspace_agent_running=mock.DEFAULT,
    )


class ResumeHappyPath(unittest.TestCase):
    def test_resume_creates_and_starts_no_log_write(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            with _no_guard_blocks() as mocks:
                mocks["transcript_exists"].return_value = True
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = None
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    call.return_value = CREATE_OK
                    rc = verbs.resume(estates, "home/garden")
            self.assertEqual(rc, 0)
            self.assertEqual(call.call_count, 2)
            create_argv = call.call_args_list[0].args[0]
            start_argv = call.call_args_list[1].args[0]
            self.assertIn("old-sid", " ".join(start_argv))
            self.assertIn("--resume", start_argv)
            # resume never appends — the conversation id is unchanged
            self.assertEqual(continuity.read(cdir), continuity.read(cdir))
            self.assertEqual(len(continuity.read(cdir)), 1)

    def test_resume_falls_back_to_fresh_with_no_recorded_conversation(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            estates = _estate(root)
            with _no_guard_blocks() as mocks:
                mocks["transcript_exists"].return_value = True
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = None
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    call.return_value = CREATE_OK
                    out = io.StringIO()
                    with redirect_stdout(out):
                        rc = verbs.resume(estates, "home/garden")
            self.assertEqual(rc, 0)
            self.assertIn("behaves as fresh", out.getvalue())
            self.assertEqual(len(continuity.read(cdir)), 1)  # fresh appended one


class ResumeGuards(unittest.TestCase):
    def test_missing_transcript_refuses(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            with mock.patch("herdr_estate.verbs.doubleopen.transcript_exists",
                             return_value=False):
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    with self.assertRaises(GuardError):
                        verbs.resume(estates, "home/garden")
                    call.assert_not_called()

    def test_registry_holder_refuses(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            with mock.patch("herdr_estate.verbs.doubleopen.transcript_exists",
                             return_value=True):
                with mock.patch("herdr_estate.verbs.doubleopen.registry_holder",
                                 return_value="andrew (pid 123)"):
                    with self.assertRaises(GuardError):
                        verbs.resume(estates, "home/garden")

    def test_running_workspace_refuses(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            with _no_guard_blocks() as mocks:
                mocks["transcript_exists"].return_value = True
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = {"workspace_id": "w1",
                                                          "label": "home/garden"}
                mocks["workspace_agent_running"].return_value = True
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    with self.assertRaises(GuardError):
                        verbs.resume(estates, "home/garden")
                    call.assert_not_called()


class FreshHappyPath(unittest.TestCase):
    def test_fresh_closes_matching_workspace_and_appends_log(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            get_ok = HerdrResult(ok=True, returncode=0,
                                  data={"workspace": {"label": "home/garden"}})
            with _no_guard_blocks() as mocks:
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = {"workspace_id": "w1",
                                                          "label": "home/garden"}
                mocks["workspace_agent_running"].return_value = False
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    call.side_effect = [get_ok,  # close_if_ours: workspace get
                                         HerdrResult(ok=True, returncode=0),  # close
                                         CREATE_OK,  # workspace create
                                         HerdrResult(ok=True, returncode=0)]  # agent start
                    rc = verbs.fresh(estates, "home/garden")
            self.assertEqual(rc, 0)
            self.assertEqual(call.call_count, 4)
            close_argv = call.call_args_list[1].args[0]
            self.assertEqual(close_argv, ["workspace", "close", "w1"])
            start_argv = call.call_args_list[3].args[0]
            self.assertIn("--session-id", start_argv)
            lines = continuity.read(cdir)
            self.assertEqual(len(lines), 2)
            self.assertNotEqual(lines[-1].split()[-1], "old-sid")

    def test_fresh_with_no_prior_workspace_skips_close(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            estates = _estate(root)
            with _no_guard_blocks() as mocks:
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = None
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    call.side_effect = [CREATE_OK, HerdrResult(ok=True, returncode=0)]
                    rc = verbs.fresh(estates, "home/garden")
            self.assertEqual(rc, 0)
            self.assertEqual(call.call_count, 2)  # create + start only


class FreshGuards(unittest.TestCase):
    def test_old_conversation_open_elsewhere_refuses(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            with mock.patch("herdr_estate.verbs.doubleopen.registry_holder",
                             return_value="andrew (pid 42)"):
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    with self.assertRaises(GuardError):
                        verbs.fresh(estates, "home/garden")
                    call.assert_not_called()

    def test_running_workspace_refuses_and_does_not_close(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            estates = _estate(root)
            with _no_guard_blocks() as mocks:
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = {"workspace_id": "w1",
                                                          "label": "home/garden"}
                mocks["workspace_agent_running"].return_value = True
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    with self.assertRaises(GuardError):
                        verbs.fresh(estates, "home/garden")
                    call.assert_not_called()


class DryRun(unittest.TestCase):
    def test_resume_dry_run_touches_no_herdr(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            with mock.patch("herdr_estate.verbs.doubleopen.transcript_exists",
                             return_value=True):
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    out = io.StringIO()
                    with redirect_stdout(out):
                        rc = verbs.resume(estates, "home/garden", dry_run=True)
                    call.assert_not_called()
            self.assertEqual(rc, 0)
            text = out.getvalue()
            self.assertIn("workspace create", text)
            self.assertIn("agent start", text)
            self.assertIn("--resume old-sid", text)

    def test_fresh_dry_run_shows_conditional_close(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                out = io.StringIO()
                with redirect_stdout(out):
                    rc = verbs.fresh(estates, "home/garden", dry_run=True)
                call.assert_not_called()
            self.assertEqual(rc, 0)
            self.assertIn("would close", out.getvalue())

    def test_fresh_dry_run_no_close_line_without_prior_conversation(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            _make_campaign_dir(root, "garden")
            estates = _estate(root)
            with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                out = io.StringIO()
                with redirect_stdout(out):
                    verbs.fresh(estates, "home/garden", dry_run=True)
                call.assert_not_called()
            self.assertNotIn("would close", out.getvalue())


class ListCampaigns(unittest.TestCase):
    def test_lists_new_and_dormant(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            cdir = _make_campaign_dir(root, "garden")
            _make_campaign_dir(root, "taxes")
            continuity.append(cdir, "sid")
            estates = _estate(root)
            rows = verbs.list_campaigns(estates)
            addresses = {r["address"] for r in rows}
            self.assertEqual(addresses, {"home/garden", "home/taxes"})
            garden = next(r for r in rows if r["address"] == "home/garden")
            taxes = next(r for r in rows if r["address"] == "home/taxes")
            self.assertNotEqual(garden["last"], "")
            self.assertEqual(taxes["last"], "")


if __name__ == "__main__":
    unittest.main()
