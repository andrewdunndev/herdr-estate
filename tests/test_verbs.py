import io
import subprocess
import tempfile
import unittest
from contextlib import contextmanager, redirect_stdout
from pathlib import Path
from unittest import mock

from herdr_estate import continuity, ownership, verbs
from herdr_estate.errors import GuardError, ProtocolError, UsageError
from herdr_estate.herdrcli import HerdrResult

CREATE_OK = HerdrResult(ok=True, returncode=0, data={
    "workspace": {"workspace_id": "w9", "label": "home/garden"},
    "root_pane": {"pane_id": "p1"},
})


@contextmanager
def _isolated():
    """A temp estate root plus an isolated HERDR_PLUGIN_STATE_DIR, so
    ownership.py's on-disk record never touches the real machine."""
    with tempfile.TemporaryDirectory() as d:
        with mock.patch.dict("os.environ",
                              {"HERDR_PLUGIN_STATE_DIR": str(Path(d) / "state")}):
            yield Path(d) / "home"


def _estate(root: Path) -> dict:
    return {"home": {"root": str(root), "agent": "claude"}}


def _make_campaign_dir(root: Path, name: str, frontmatter: str = "---\n---\n") -> Path:
    cdir = root / "campaigns" / name
    cdir.mkdir(parents=True)
    (cdir / "campaign.md").write_text(frontmatter)
    return cdir


def _run(*args, cwd):
    subprocess.run(list(args), cwd=cwd, check=True, capture_output=True, text=True)


def _init_git_repo(path: Path) -> None:
    path.mkdir(parents=True)
    _run("git", "init", "-b", "main", cwd=path)
    _run("git", "config", "user.email", "test@example.com", cwd=path)
    _run("git", "config", "user.name", "Test", cwd=path)
    (path / "README.md").write_text("hello\n")
    _run("git", "add", "README.md", cwd=path)
    _run("git", "commit", "-m", "init", cwd=path)


def _git_snapshot(repo: Path):
    branches = subprocess.run(["git", "-C", str(repo), "branch", "--list"],
                               capture_output=True, text=True, check=True).stdout
    worktrees = subprocess.run(["git", "-C", str(repo), "worktree", "list"],
                                capture_output=True, text=True, check=True).stdout
    return branches, worktrees


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

    def test_create_args_has_no_focus(self):
        self.assertIn("--no-focus", verbs.create_args(self._campaign()))

    def test_create_args_scrubs_workspace_env_before_estate_env(self):
        args = verbs.create_args(self._campaign())
        for k in verbs.WORKSPACE_ENV_SCRUB:
            self.assertIn(f"{k}=", args)
            self.assertLess(args.index(f"{k}="), args.index("K=V"))

    def test_create_args_redact_masks_estate_env_not_address(self):
        args = verbs.create_args(self._campaign(), redact=True)
        self.assertIn("K=<from estates.toml>", args)
        self.assertNotIn("K=V", args)
        self.assertIn("HERDR_ESTATE_ADDRESS=home/garden", args)

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


class CreatedIds(unittest.TestCase):
    """workspace create's response is validated before use."""

    def test_missing_workspace_id_raises_protocol_error(self):
        bad = HerdrResult(ok=True, returncode=0,
                           data={"root_pane": {"pane_id": "p1"}})
        with self.assertRaises(ProtocolError):
            verbs._created_ids(bad)

    def test_missing_pane_id_raises_protocol_error(self):
        bad = HerdrResult(ok=True, returncode=0,
                           data={"workspace": {"workspace_id": "w1"}})
        with self.assertRaises(ProtocolError):
            verbs._created_ids(bad)

    def test_valid_response_returns_both_ids(self):
        self.assertEqual(verbs._created_ids(CREATE_OK), ("w9", "p1"))


def _no_guard_blocks():
    """Patch the double-open guard functions so nothing refuses."""
    return mock.patch.multiple(
        "herdr_estate.verbs.doubleopen",
        transcript_exists=mock.DEFAULT,
        registry_holder=mock.DEFAULT,
        live_workspace=mock.DEFAULT,
        workspace_alive=mock.DEFAULT,
    )


class ResumeHappyPath(unittest.TestCase):
    def test_resume_creates_and_starts_no_log_write(self):
        with _isolated() as root:
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            before = continuity.read(cdir)
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
            start_argv = call.call_args_list[1].args[0]
            self.assertIn("old-sid", " ".join(start_argv))
            self.assertIn("--resume", start_argv)
            # resume never appends — the log is byte-for-byte unchanged
            self.assertEqual(continuity.read(cdir), before)
            self.assertEqual(len(continuity.read(cdir)), 1)

    def test_resume_falls_back_to_fresh_with_no_recorded_conversation(self):
        with _isolated() as root:
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
        with _isolated() as root:
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
        with _isolated() as root:
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
        with _isolated() as root:
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            with _no_guard_blocks() as mocks:
                mocks["transcript_exists"].return_value = True
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = {"workspace_id": "w1",
                                                          "label": "home/garden",
                                                          "agent_status": "working"}
                mocks["workspace_alive"].return_value = True
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    with self.assertRaises(GuardError):
                        verbs.resume(estates, "home/garden")
                    call.assert_not_called()

    def test_not_ours_workspace_refuses_and_creates_nothing(self):
        """A live workspace under this address that herdr-estate did
        not create refuses — nothing is closed, nothing is created."""
        with _isolated() as root:
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            with _no_guard_blocks() as mocks:
                mocks["transcript_exists"].return_value = True
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = {"workspace_id": "unowned-1",
                                                          "label": "home/garden",
                                                          "agent_status": "unknown"}
                mocks["workspace_alive"].return_value = False
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    with self.assertRaises(GuardError) as ctx:
                        verbs.resume(estates, "home/garden")
                    call.assert_not_called()
            self.assertIn("did not create", str(ctx.exception))


class FreshHappyPath(unittest.TestCase):
    def test_fresh_closes_matching_workspace_and_appends_log(self):
        with _isolated() as root:
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            ownership.record("home/garden", "w1")
            get_ok = HerdrResult(ok=True, returncode=0,
                                  data={"workspace": {"label": "home/garden"}})
            with _no_guard_blocks() as mocks:
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = {"workspace_id": "w1",
                                                          "label": "home/garden",
                                                          "agent_status": "unknown"}
                mocks["workspace_alive"].return_value = False
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
            # the closed workspace's record is gone; the new one is owned
            self.assertFalse(ownership.is_ours("w1"))
            self.assertTrue(ownership.is_ours("w9"))

    def test_fresh_with_no_prior_workspace_skips_close(self):
        with _isolated() as root:
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
        with _isolated() as root:
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
        with _isolated() as root:
            estates = _estate(root)
            _make_campaign_dir(root, "garden")
            with _no_guard_blocks() as mocks:
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = {"workspace_id": "w1",
                                                          "label": "home/garden",
                                                          "agent_status": "idle"}
                mocks["workspace_alive"].return_value = True
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    with self.assertRaises(GuardError):
                        verbs.fresh(estates, "home/garden")
                    call.assert_not_called()

    def test_not_ours_workspace_refuses(self):
        with _isolated() as root:
            estates = _estate(root)
            _make_campaign_dir(root, "garden")
            with _no_guard_blocks() as mocks:
                mocks["registry_holder"].return_value = None
                mocks["live_workspace"].return_value = {"workspace_id": "unowned-1",
                                                          "label": "home/garden",
                                                          "agent_status": "unknown"}
                mocks["workspace_alive"].return_value = False
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    with self.assertRaises(GuardError) as ctx:
                        verbs.fresh(estates, "home/garden")
                    call.assert_not_called()
            self.assertIn("did not create", str(ctx.exception))


class KindGatedGuards(unittest.TestCase):
    """The two checks reading Claude Code's own local state run only
    for campaigns whose agent kind is "claude"."""

    def test_resume_skips_claude_checks_for_other_kind(self):
        with _isolated() as root:
            cdir = _make_campaign_dir(root, "garden", "---\nagent: codex\n---\n")
            continuity.append(cdir, "old-sid")
            estates = {"home": {"root": str(root)}}
            with _no_guard_blocks() as mocks:
                mocks["live_workspace"].return_value = None
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    call.return_value = CREATE_OK
                    rc = verbs.resume(estates, "home/garden")
                mocks["transcript_exists"].assert_not_called()
                mocks["registry_holder"].assert_not_called()
            self.assertEqual(rc, 0)

    def test_fresh_skips_claude_checks_for_other_kind(self):
        with _isolated() as root:
            cdir = _make_campaign_dir(root, "garden", "---\nagent: codex\n---\n")
            continuity.append(cdir, "old-sid")
            estates = {"home": {"root": str(root)}}
            with _no_guard_blocks() as mocks:
                mocks["live_workspace"].return_value = None
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    call.return_value = CREATE_OK
                    rc = verbs.fresh(estates, "home/garden")
                mocks["registry_holder"].assert_not_called()
            self.assertEqual(rc, 0)


class DryRun(unittest.TestCase):
    def test_resume_dry_run_makes_no_mutating_herdr_call(self):
        with _isolated() as root:
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            list_ok = HerdrResult(ok=True, returncode=0, data={"workspaces": []})
            with mock.patch("herdr_estate.verbs.doubleopen.transcript_exists",
                             return_value=True):
                with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                    call.return_value = list_ok
                    out = io.StringIO()
                    with redirect_stdout(out):
                        rc = verbs.resume(estates, "home/garden", dry_run=True)
            self.assertEqual(rc, 0)
            # the only herdr call a dry run makes is the read that decides
            # whether it would close — never create, close or start
            call.assert_called_once_with(["workspace", "list"])
            text = out.getvalue()
            self.assertIn("workspace create", text)
            self.assertIn("agent start", text)
            self.assertIn("--resume old-sid", text)

    def test_fresh_dry_run_shows_close_when_a_live_owned_workspace_exists(self):
        with _isolated() as root:
            cdir = _make_campaign_dir(root, "garden")
            continuity.append(cdir, "old-sid")
            estates = _estate(root)
            ownership.record("home/garden", "w1")
            list_ok = HerdrResult(ok=True, returncode=0, data={
                "workspaces": [{"workspace_id": "w1", "label": "home/garden",
                                 "agent_status": "unknown"}]})
            with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                call.return_value = list_ok
                out = io.StringIO()
                with redirect_stdout(out):
                    rc = verbs.fresh(estates, "home/garden", dry_run=True)
                call.assert_called_once_with(["workspace", "list"])
            self.assertEqual(rc, 0)
            text = out.getvalue()
            self.assertIn("would close", text)
            self.assertIn("w1", text)

    def test_fresh_dry_run_shows_close_even_without_a_recorded_conversation(self):
        """The close line tracks live workspace state, never whether a
        conversation happens to be recorded — the bug being fixed here
        tied it to the latter."""
        with _isolated() as root:
            _make_campaign_dir(root, "garden")  # no continuity.append
            estates = _estate(root)
            ownership.record("home/garden", "w1")
            list_ok = HerdrResult(ok=True, returncode=0, data={
                "workspaces": [{"workspace_id": "w1", "label": "home/garden",
                                 "agent_status": "unknown"}]})
            with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                call.return_value = list_ok
                out = io.StringIO()
                with redirect_stdout(out):
                    rc = verbs.fresh(estates, "home/garden", dry_run=True)
            self.assertEqual(rc, 0)
            self.assertIn("would close", out.getvalue())

    def test_fresh_dry_run_no_close_line_without_a_live_workspace(self):
        with _isolated() as root:
            _make_campaign_dir(root, "garden")
            estates = _estate(root)
            list_ok = HerdrResult(ok=True, returncode=0, data={"workspaces": []})
            with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                call.return_value = list_ok
                out = io.StringIO()
                with redirect_stdout(out):
                    rc = verbs.fresh(estates, "home/garden", dry_run=True)
            self.assertEqual(rc, 0)
            self.assertNotIn("would close", out.getvalue())

    def test_dry_run_refuses_when_the_live_workspace_is_alive(self):
        """A dry run raises the same GuardError the real run would —
        the refusal line — instead of printing a create/start plan."""
        with _isolated() as root:
            _make_campaign_dir(root, "garden")
            estates = _estate(root)
            list_ok = HerdrResult(ok=True, returncode=0, data={
                "workspaces": [{"workspace_id": "w1", "label": "home/garden",
                                 "agent_status": "working"}]})
            with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                call.return_value = list_ok
                with self.assertRaises(GuardError):
                    verbs.fresh(estates, "home/garden", dry_run=True)

    def test_dry_run_refuses_when_the_live_workspace_is_not_ours(self):
        with _isolated() as root:
            _make_campaign_dir(root, "garden")
            estates = _estate(root)
            list_ok = HerdrResult(ok=True, returncode=0, data={
                "workspaces": [{"workspace_id": "unowned-1", "label": "home/garden",
                                 "agent_status": "unknown"}]})
            with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                call.return_value = list_ok
                with self.assertRaises(GuardError):
                    verbs.resume(estates, "home/garden", dry_run=True)

    def test_dry_run_leaks_no_env_value_and_mutates_no_git(self):
        """A dry run's create-command line masks every estate-
        table env value, and a worktree campaign's dry run never touches
        git (no `worktree add`, no branch, no `worktree prune`)."""
        with _isolated() as root:
            repo_path = root / "the-repo"
            _init_git_repo(repo_path)
            _make_campaign_dir(
                root, "garden",
                f"---\nworktree:\n  repo: {repo_path}\n  branch: wt/site-work\n---\n")
            estates = {"home": {"root": str(root),
                                 "env": {"SECRET_TOKEN": "sk-do-not-print-me"}}}
            before = _git_snapshot(repo_path)
            list_ok = HerdrResult(ok=True, returncode=0, data={"workspaces": []})
            with mock.patch("herdr_estate.verbs.herdrcli.call_herdr") as call:
                call.return_value = list_ok
                out = io.StringIO()
                with redirect_stdout(out):
                    rc = verbs.fresh(estates, "home/garden", dry_run=True)
            self.assertEqual(rc, 0)
            text = out.getvalue()
            self.assertNotIn("sk-do-not-print-me", text)
            self.assertIn("SECRET_TOKEN=<from estates.toml>", text)
            after = _git_snapshot(repo_path)
            self.assertEqual(before, after)


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
