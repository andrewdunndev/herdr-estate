# Changelog

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
