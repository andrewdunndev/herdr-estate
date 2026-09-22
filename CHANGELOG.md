# Changelog

## Unreleased

- The `pick` action opens the picker popup. herdr runs an action with
  no terminal, so a key bound to it failed with "could not start the
  terminal". The README binds it to `prefix+shift+e`, a key herdr
  leaves free; its old suggestions `prefix+r` and `prefix+p` shadowed
  herdr's `resize_mode` and `previous_tab`.
- `resume` and `fresh` print a line to stderr before each step that
  changes the board, including the wait while `agent start` blocks
  until the agent is ready, so the picker popup no longer sits blank
  through it.
- A failed dispatch from the picker holds its error in the popup until
  a key press instead of closing with it. Keys typed during the wait
  are discarded first, so they can't dismiss it unread.
- A failed herdr call's message masks estate `env` values, as a
  `--dry-run` plan does. A failed `workspace create` used to print them
  in the clear.
- Every run appends its argv, how it was launched, its progress lines
  and any error to `events.log` in the state dir: one timestamped
  record across the picker popup, plugin actions and the shell.

## v0.1.0

First release. A herdr plugin that gives agent work a durable address:
`estate/campaign`, resolved from one estate table and one campaign file.

- Verbs `resume`, `fresh` and `list`, addressed by `estate/campaign`.
  Resume reopens the campaign's latest conversation; fresh starts a new one.
- Guards on both verbs: ownership by recorded workspace id, an alive check
  that treats only `unknown` as dead, refusal on a duplicated label, and a
  stop when a close cannot be confirmed. The plugin never closes a
  workspace it did not create.
- A continuity log per campaign, one line per launch, read on resume.
- A popup picker (`pick`) that filters campaigns by address and dispatches
  through the same verbs and guards, re-reading the workspace at Enter.
- Optional worktrees, ensure-only with plain git.
- Estate-level paths and per-kind flags as defaults, overridden key by key
  in the campaign file.
- Three direct dependencies for the verbs; the picker adds ratatui and
  crossterm.
