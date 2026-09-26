![hero](hero.svg)

# dunn.dev/herdr-estate

[![pipeline](https://gitlab.com/dunn.dev/herdr-estate/badges/main/pipeline.svg)](https://gitlab.com/dunn.dev/herdr-estate/-/pipelines)
[![license](https://img.shields.io/badge/license-MIT_AND_Apache--2.0-green)](#license)
![built with GitLab](https://img.shields.io/badge/built_with-GitLab-FC6D26?logo=gitlab)

herdr-estate is a plugin for the [herdr](https://herdr.dev) terminal
workspace manager: long-running campaigns, threads of agent work
addressed as `estate/campaign`, that outlive every conversation. Each
word is defined once in [docs/vocabulary.md](docs/vocabulary.md).

## Long-running work

A campaign runs for weeks, and a conversation costs more per turn the
longer it runs, so recycle often: ask the agent for a dated entry in
the campaign's `log.md` saying where to pick up; once it exits, `fresh`
retires the conversation and starts a new one at the same address,
with the same paths, flags and prompts. The new one reads the log, not
the old transcript. The estate's `prompts` teach the agent to keep the
log ([example](examples/home/prompts/base.md)). `resume` continues a
conversation that isn't finished.

## Install

Requirements: herdr 0.9.1+, macOS or Linux, Rust 1.88+; herdr builds
the plugin on install.

```sh
herdr plugin install andrewdunndev/herdr-estate
```

Bind the picker to a free key in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+shift+e"
type = "plugin_action"
command = "herdr-estate.pick"
```

`resume` and `fresh` bind the same way. For shell use:

```sh
cargo install --locked --git https://github.com/andrewdunndev/herdr-estate
```

or a [release binary](https://gitlab.com/dunn.dev/herdr-estate/-/releases) puts `herdr-estate` on your PATH.

## Configure

![An address names one estate table and one campaign directory, whose campaign.md sets the agent, its flags and extra paths.](diagrams/two-files.svg)

The estate table is `estates.toml` in the config dir herdr gives the
plugin, `~/.config/herdr/plugins/config/herdr-estate/` by default
(`herdr plugin config-dir herdr-estate` prints it); a shell run reads
the same file. When it isn't there, `~/.config/herdr-estate/estates.toml`
is read instead. Only `root` is required:

```toml
[estate.home]
root = "~/estates/home"
```

Each campaign is a directory under `campaigns/` holding a
`campaign.md`, whose YAML frontmatter overrides the agent kind and its
flags, and adds `paths`. A `paths` entry that doesn't exist is skipped,
with a warning, rather than passed on. The agent works from the estate
root, with the address in `HERDR_ESTATE_ADDRESS`.
[examples/](examples/) is an annotated estate.

An estate's optional `prepare` runs an argv command, no shell, in the
campaign's workdir before the workspace is closed or created; a
non-zero exit, or a run past 60 seconds, refuses the launch and leaves
a running workspace untouched. The agent then has 30 seconds to be
ready, and a picker row handed to herdr (below) is waited on for 120.

## Worktrees

```yaml
worktree:
  repo: ~/code/site
  branch: wt/site-work
```

Once their guards pass, `resume` and `fresh` create it if missing (a
new branch starts from the remote default; an existing one is checked
out as is), then run `prepare` and the agent in it and add it to its
paths. List the estate root in `paths`
so the agent can still reach `log.md`.

## The verbs

![Both verbs read the workspace list first and refuse on a duplicate label, a running agent, a workspace they didn't create or an unconfirmed close.](diagrams/verbs.svg)

- `resume [<address>]`: continue the latest conversation, or start
  fresh if none is on record.
- `fresh [<address>]`: retire the latest conversation and start a new
  one.
- `list`: every campaign in the estate table.
- `create <address> --brief <text>`: write a new campaign's
  `campaign.md` and `log.md`, uncommitted.

With no address, the current workspace's label is used. `--dry-run`
prints the plan and changes nothing; it needs an address. `--help`
lists the verbs and their flags. Progress and
the run log: [docs/events.md](docs/events.md).

## Picker

`herdr-estate pick`, on the key bound above, opens a popup of every
campaign, marked live, dormant or new. Type to filter; Enter focuses,
resumes or starts fresh; Ctrl-f starts fresh, and on an idle or done
agent a second press stops it first. The footer lists its keys.

## Boundaries

- Never closes a workspace it didn't create, and never removes a
  worktree.
- Keeps its own continuity in each campaign's `sessions.log`
  (timestamp, hostname, conversation id) and never uses herdr's resume.
- Workspaces inherit the herdr server's environment, so start it from
  a clean shell. `ANTHROPIC_API_KEY`, `TMUX` and `TMUX_PANE` are
  blanked unless the estate's `env` sets them.
- `claude` is the only adapter; other kinds start with no mapped
  flags.

## Develop

Development happens on
[GitLab](https://gitlab.com/dunn.dev/herdr-estate), where review, CI
and releases run; the
[GitHub repository](https://github.com/andrewdunndev/herdr-estate) is
a mirror. If the plugin helps you, we'd like to hear how you use it:
open an issue on either, or start a discussion on GitHub.

```sh
cargo build --release && herdr plugin link "$PWD"
cargo fmt --check && cargo clippy --release --all-targets --all-features -- -D warnings && cargo test
```

`plugin link` skips `[[build]]`; a rebuild takes effect on herdr's
next call. Tests drive a fake herdr through `$HERDR_BIN_PATH`.
`pick --render-once` prints one frame, 80x20 unless `--width` and
`--height` say otherwise. When a row's workspace is the
popup's own, the popup hands it to a `dispatch-*` action, which runs
`pick --dispatch`, over the API socket herdr names in
`HERDR_SOCKET_PATH`.

## License

MIT, in [LICENSE](LICENSE), except `src/picker/theme.rs`, which adapts
herdr's palettes and theme resolution under Apache-2.0, in
[LICENSE-APACHE-HERDR](LICENSE-APACHE-HERDR).

---

Built by Andrew Dunn, 2026.
