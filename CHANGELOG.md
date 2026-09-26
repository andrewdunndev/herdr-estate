# Changelog

## Unreleased

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
  then names its plugin log entry.
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
- An estate's `prepare` that runs past 60 seconds is killed and the
  launch refused, where a hung one used to hang `resume`, `fresh` and
  the picker with it.

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
