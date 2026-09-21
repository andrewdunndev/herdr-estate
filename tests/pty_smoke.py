#!/usr/bin/env python3
"""PTY smoke test for `herdr-estate pick`, driving the real built binary.

Not part of `cargo fmt`/`cargo clippy`/`cargo test` — those three stay
Rust-only. This is a separate, optional check: build the binary first
(`cargo build`), then run `python3 tests/pty_smoke.py`. It needs only the
Python standard library (the `pty` module), which behaves the same on
macOS and Linux, unlike the `script` utility's differing flags.

It opens a pseudo-terminal, runs `herdr-estate pick` against a throwaway
estate and a fake `herdr` (so nothing real is ever touched), types a
filter that narrows to one seeded "new" campaign, presses Enter, and
checks that the fake recorded the exact fresh-then-focus sequence and
that the process exited 0. Everything the child sees is built here, not
inherited: `PATH` is a poison directory first (any accidental real
`herdr` call is caught, not silently run), and no `HERDR_*` variable
rides along from this process's own environment.
"""

import os
import pty
import select
import shutil
import subprocess
import sys
import tempfile
import time

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_BIN = os.path.join(REPO_ROOT, "target", "debug", "herdr-estate")


def write_fake_herdr(path, calls_dir):
    os.makedirs(calls_dir, exist_ok=True)
    body = """#!/bin/sh
DIR="$(cd "$(dirname "$0")" && pwd)/calls"
IDXFILE="$DIR/count"
if [ -f "$IDXFILE" ]; then IDX=$(cat "$IDXFILE"); else IDX=0; fi
echo $((IDX + 1)) > "$IDXFILE"
for a in "$@"; do printf '%s\\n' "$a"; done > "$DIR/$IDX.argv"
if [ -f "$DIR/$IDX.stdout" ]; then cat "$DIR/$IDX.stdout"; fi
if [ -f "$DIR/$IDX.exit" ]; then exit "$(cat "$DIR/$IDX.exit")"; fi
exit 0
"""
    with open(path, "w") as f:
        f.write(body)
    os.chmod(path, 0o755)


def set_response(calls_dir, index, stdout, exit_code):
    with open(os.path.join(calls_dir, f"{index}.stdout"), "w") as f:
        f.write(stdout)
    with open(os.path.join(calls_dir, f"{index}.exit"), "w") as f:
        f.write(str(exit_code))


def read_argv(calls_dir, index):
    p = os.path.join(calls_dir, f"{index}.argv")
    if not os.path.isfile(p):
        return None
    with open(p) as f:
        return [line.rstrip("\n") for line in f if line.strip() != ""]


def call_count(calls_dir):
    p = os.path.join(calls_dir, "count")
    if not os.path.isfile(p):
        return 0
    with open(p) as f:
        return int(f.read().strip() or "0")


def drain(fd, timeout):
    end = time.time() + timeout
    out = b""
    while time.time() < end:
        remaining = max(0.0, end - time.time())
        r, _, _ = select.select([fd], [], [], remaining)
        if fd not in r:
            break
        try:
            chunk = os.read(fd, 4096)
        except OSError:
            break
        if not chunk:
            break
        out += chunk
    return out


def main():
    binary = os.environ.get("HERDR_ESTATE_BIN", DEFAULT_BIN)
    if not os.path.isfile(binary):
        print(f"PTY_SMOKE_SKIP: no binary at {binary} — run `cargo build` first")
        return 0

    base = tempfile.mkdtemp(prefix="herdr-estate-pty-smoke-")
    try:
        root = os.path.join(base, "home")
        campaigns = os.path.join(root, "campaigns", "attic")
        os.makedirs(campaigns)
        with open(os.path.join(campaigns, "campaign.md"), "w") as f:
            f.write("---\n---\n")

        config_dir = os.path.join(base, "config")
        os.makedirs(config_dir)
        with open(os.path.join(config_dir, "estates.toml"), "w") as f:
            f.write(f'[estate.home]\nroot = "{root}"\n')

        state_dir = os.path.join(base, "state")
        user_home = os.path.join(base, "userhome")
        os.makedirs(user_home)

        fake_dir = os.path.join(base, "fake-herdr")
        calls_dir = os.path.join(fake_dir, "calls")
        fake_bin = os.path.join(fake_dir, "herdr")
        write_fake_herdr(fake_bin, calls_dir)

        # Full flow, two `workspace list` calls: `rows::build_rows` makes
        # its own (call 0), then dispatching the "new" row into
        # `fresh_with_ids` makes another as part of its own
        # live-workspace check (call 1), before create (2), agent start
        # (3), and dispatch's own focus call (4, left to the fake's
        # default 0-exit empty-stdout response).
        set_response(calls_dir, 0, '{"result": {"workspaces": []}}', 0)
        set_response(calls_dir, 1, '{"result": {"workspaces": []}}', 0)
        set_response(
            calls_dir,
            2,
            '{"result": {"workspace": {"workspace_id": "w-pty"},'
            ' "root_pane": {"pane_id": "p1"}}}',
            0,
        )
        set_response(calls_dir, 3, '{"result": {}}', 0)

        poison_dir = os.path.join(base, "poison-bin")
        os.makedirs(poison_dir)
        poison_log = os.path.join(base, "poison.log")
        with open(os.path.join(poison_dir, "herdr"), "w") as f:
            f.write(
                "#!/bin/sh\n"
                "{ printf '%s argv:' \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"; "
                "for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'; } "
                f">> {poison_log}\n"
                "exit 97\n"
            )
        os.chmod(os.path.join(poison_dir, "herdr"), 0o755)

        env = {
            "HOME": user_home,
            "HERDR_PLUGIN_CONFIG_DIR": config_dir,
            "HERDR_PLUGIN_STATE_DIR": state_dir,
            "HERDR_BIN_PATH": fake_bin,
            "PATH": f"{poison_dir}:/usr/bin:/bin",
            "TERM": "xterm",
        }

        master_fd, slave_fd = pty.openpty()
        proc = subprocess.Popen(
            [binary, "pick"],
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
            env=env,
            close_fds=True,
        )
        os.close(slave_fd)

        # Let the picker draw its first frame, then type a filter that
        # narrows to exactly the one seeded campaign, then Enter
        # (raw-mode terminals send CR, not LF, for the Enter key).
        drain(master_fd, 1.0)
        os.write(master_fd, b"attic\r")
        drain(master_fd, 1.0)

        try:
            exit_code = proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            exit_code = None
        finally:
            try:
                os.close(master_fd)
            except OSError:
                pass

        problems = []

        if exit_code != 0:
            problems.append(f"exit code was {exit_code!r}, expected 0")

        n = call_count(calls_dir)
        if n != 5:
            problems.append(f"fake herdr call_count was {n}, expected 5")

        expected_prefixes = [
            ["workspace", "list"],
            ["workspace", "list"],
            ["workspace", "create"],
            ["agent", "start"],
            ["workspace", "focus", "w-pty"],
        ]
        for i, prefix in enumerate(expected_prefixes):
            argv = read_argv(calls_dir, i)
            if argv is None or argv[: len(prefix)] != prefix:
                problems.append(f"call {i} argv was {argv!r}, expected prefix {prefix!r}")

        if os.path.isfile(poison_log):
            with open(poison_log) as f:
                poison_lines = sum(1 for _ in f)
            if poison_lines:
                problems.append(
                    f"poison log has {poison_lines} line(s) — a real herdr was invoked"
                )

        if problems:
            print("PTY_SMOKE_FAIL")
            for p in problems:
                print(f"  - {p}")
            return 1
        print("PTY_SMOKE_OK")
        return 0
    finally:
        shutil.rmtree(base, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
