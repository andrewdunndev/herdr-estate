# Changelog

## Unreleased

- Ctrl-f's stop-then-fresh now works in a workspace with more than one
  pane, such as a shell or a review pane beside the agent. It stops only
  the agent's pane with Ctrl-C, waits for the agent to leave, and starts
  the new conversation in that same pane; every other pane keeps running
  and keeps its place. It finds the agent by its herdr name, and refuses,
  listing the panes, when none or more than one in the workspace carries
  it, when the agent is not idle or done, or when it does not exit.
  A one-pane workspace is replaced as before.
- A campaign `worktree` table missing `repo` or `branch` is now a config
  error naming the campaign and the key, not a silent run in the main
  checkout.

## v0.3.0 - 2026-10-08

- Tested against herdr 0.9.3, which changes none of the commands,
  replies or paths the plugin uses. It still needs herdr 0.9.1 or later.
- A new workspace now puts herdr's workspace list in label order: `~`
  first, the rest ignoring case. The manifest runs the new `sort` verb
  on herdr's `workspace.created` event; it moves only the workspaces out
  of place, and a move herdr refuses is logged and the rest left as is.
- Sort spaces runs the same sort on demand: a row at the foot of the
  picker (type `sort` to reach it), which keeps the picker open and
  shows what it did, and the manifest action `herdr-estate.sort` for a
  key binding.

## v0.2.3 - 2026-09-27

- The picker run from a pane's shell now hands a row for its own
  workspace to herdr, as the popup does, reading the workspace from
  `$HERDR_WORKSPACE_ID`. v0.2.2's Ctrl-f fix held only in the popup.
- A `prepare` step that touches the terminal, such as
  `stty -echo </dev/tty`, now fails at once instead of sitting stopped
  until the 60-second timeout. `prepare` keeps its own process group and
  drops the terminal. It stays in the verb's session, so herdr closing
  the pane, popup or workspace still ends it.
- A failed `prepare` that left a process holding its output open is
  refused with what it wrote, not only its exit status.

## v0.2.2 - 2026-09-26

- The picker no longer stops halfway when the row it acts on is the
  workspace it was opened from. Closing that workspace ended the popup
  and the run inside it, after the close and before the create, so
  Ctrl-f's second press stopped the agent and started nothing. The
  popup now hands such a row to herdr as one of three new manifest
  actions, `dispatch-open`, `dispatch-fresh` and `dispatch-stop`, which
  herdr runs outside every workspace. The replacement is created,
  started and focused as it is from any other workspace, and a refusal
  still holds in the popup, as does herdr's own error for a run that
  never started. The popup waits on the run for at most 120 seconds,
  then names its plugin log entry. Before invoking, it checks herdr's
  API protocol with `ping` and refuses one older than herdr 0.9.1's,
  naming both; any other protocol error names them too.
- `resume` or `fresh` typed in the campaign's own workspace once its
  agent has exited no longer closes that workspace and stops, taking
  the shell running it down with it. It hands the run to herdr through
  the same `dispatch-*` actions, and the replacement is created,
  started and focused.
- Two launches of one address at once, from two popups or a popup and
  a shell, no longer both create a workspace under its label. A launch
  takes a lock for its campaign in the state dir, and the second one
  refuses, naming the first one's pid, until the first finishes. A run
  that dies, even by SIGKILL, releases its lock with it.
- Launches of different addresses at once no longer drop each other's
  records from `owned.json`. Every change to it holds `owned.lock`
  beside it, and a launch prunes only records that existed before its
  workspace listing was read.
- `--help` and `-h` print the verbs and their flags, and write
  nothing to `events.log`.
- A shell run now finds the estate table where the picker does, in
  herdr's config dir for the plugin
  (`~/.config/herdr/plugins/config/herdr-estate/estates.toml` by
  default). `~/.config/herdr-estate/estates.toml` is still read when
  that file isn't there, so an existing table needs no move.
- A refused `resume` or `fresh` no longer creates the campaign's
  worktree first. The worktree is created once every guard has passed,
  just before `prepare` runs in it.
- A `paths` entry that doesn't exist is reported once the launch has
  passed its guards and `prepare`, not while the campaign resolves.
  `--dry-run` and a refused launch no longer write that warning to
  `events.log`; a dry run lists the path in its plan.
- An estate's `prepare` that runs past 60 seconds is killed, with
  every process it started, and the launch refused, where a hung one
  used to hang `resume`, `fresh` and the picker with it. `prepare` now
  runs in its own process group, so a Ctrl-C in the terminal running
  the verb no longer reaches it.
- The package license reads `MIT AND Apache-2.0`: the picker's theme
  code adapts herdr's under Apache-2.0, as the README's new License
  section says.

## v0.2.1 - 2026-09-25

- A bare shell invocation now resolves the state dir to the same
  `.../herdr/plugins/herdr-estate` directory herdr itself hands the
  picker, instead of a separate `.../herdr-estate`. The picker no
  longer refuses a workspace the CLI launched. An older
  `~/.local/state/herdr-estate` from before this fix isn't migrated;
  merge its `owned.json` into the new dir by hand, or delete it.

## v0.2.0 - 2026-09-24

- An estate row's `prepare` runs an argv command, no shell, in the
  campaign's workdir before the workspace is closed or created; a
  non-zero exit or a spawn failure refuses the launch and leaves a
  running workspace untouched.
- A `paths` entry that doesn't exist on disk is skipped, with a
  warning, instead of being passed on to the agent.

## v0.1.2 - 2026-09-24

- The picker's Ctrl-f now doubles as a stop: on a row whose agent is
  idle or done, a first press arms and a second press on the same row
  stops it and starts fresh. It refuses a working or blocked agent, a
  workspace with more than one pane, and a conversation open in a
  second process. The CLI's `fresh` still refuses a running agent.

## v0.1.1 - 2026-09-23

- `{address}` in an estate `env` value becomes the campaign's address,
  so one estate row can give each workspace a value naming its campaign.

## v0.1.0 - 2026-09-23

First release. Long-running campaigns for herdr: threads of agent work,
addressed as estate/campaign, that outlive every conversation.

- `resume` continues a campaign's latest conversation, or starts one if
  none is on record; `fresh` retires it and starts a new one at the
  same address. Both refuse on a duplicate label, a running agent, a
  workspace they didn't create or an unconfirmed close.
- `list` prints every campaign. `create <address> --brief <text>`
  writes a new campaign's `campaign.md` and `log.md`, uncommitted, and
  refuses to adopt anything already at the path.
- `pick` opens a popup picker drawn in herdr's style and theme, each
  row showing its campaign's brief. Enter focuses, resumes or starts
  fresh; ctrl+f starts fresh; a filter `estate/name` naming no campaign
  offers `+ create`. A failed dispatch stays on screen until a key
  press.
- A campaign's `sessions.log` keeps one line per conversation. Optional
  worktrees are made with plain git and never removed.
- Steps that change the board print progress lines, and every run is
  logged to `events.log` in the state dir.
- Estate `env` values are masked in `--dry-run` plans and in the
  message of a failed herdr call.
- `--version` prints the version.
- herdr builds the plugin with `cargo build --release --locked`.
- Each tagged release carries cosign-signed binaries for Linux (x86_64,
  arm64) and macOS (arm64), with CycloneDX SBOMs and SLSA provenance.
