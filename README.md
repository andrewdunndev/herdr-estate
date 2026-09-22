# dunn.dev/herdr-estate

[![pipeline](https://gitlab.com/dunn.dev/herdr-estate/badges/main/pipeline.svg)](https://gitlab.com/dunn.dev/herdr-estate/-/pipelines)
[![license](https://img.shields.io/badge/license-MIT-green)](LICENSE)
![built with GitLab](https://img.shields.io/badge/built_with-GitLab-FC6D26?logo=gitlab)

herdr-estate is a plugin for the [herdr](https://herdr.dev) terminal
workspace manager. It gives herdr *estates* and *campaigns*: durable
threads of agent work, addressed as `estate/campaign`, that resume
where they left off instead of starting cold every time. The plugin
owns two files plus a continuity log it reads and writes itself;
herdr's own resume is never used.

Brand artifacts (`avatar.svg`, `hero.svg`, `favicon.svg`, `brand.png`)
are not shipped yet; see `.deviations.yaml`.

## Install

Build the binary, then link the plugin:

```sh
cargo build --release
herdr plugin link /path/to/herdr-estate
```

`plugin link` is dev mode: it registers the path as-is and never runs
the manifest's `[[build]]` table, so the binary needs to exist before
you link it. Rebuilding after a source change takes effect on herdr's
next invocation, no relink needed. `herdr plugin install` (a GitHub
install, not dev mode) runs `[[build]]` itself. Then bind the actions
to keys in your own `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+r"
type = "plugin_action"
command = "herdr-estate.resume"

[[keys.command]]
key = "prefix+f"
type = "plugin_action"
command = "herdr-estate.fresh"
```

## The two files

![An address names one estate table and one campaign directory inside it, where campaign.md overrides the estate's keys one at a time.](diagrams/two-files.svg)

**The estate table**, `estates.toml`, lives at
`$HERDR_PLUGIN_CONFIG_DIR/estates.toml` when herdr sets that (running as
a plugin action), or `~/.config/herdr-estate/estates.toml` for a bare
shell invocation:

```toml
[estate.home]
root = "~/estates/home"
campaigns = "campaigns"
agent = "claude"
paths = ["~/code"]
prompts = ["prompts/base.md"]
env = { }

[estate.home.claude]
remote-control = true
```

A relative `prompts` entry resolves against the estate's `root`, not the
directory herdr-estate happens to run from; `~` and `$VAR` expand
first, so an absolute or `~`-prefixed entry is unaffected. See
[`examples/estates.toml`](examples/estates.toml).

**The campaign file**, `campaigns/<name>/campaign.md`, carries YAML
frontmatter:

```yaml
paths: [~/code/a, ~/code/b]   # extra directories the agent may work in
agent: claude
claude:
  remote-control: true
  mcp-config: .mcp.json
```

The shipped adapter is `claude`; a kind with no adapter starts with no
mapped flags and prints one warning line. A relative `paths` entry
resolves against the estate's `root`, the same rule `prompts` follows
above, and a path listed by both the estate and the campaign is kept
once. See [`docs/vocabulary.md`](docs/vocabulary.md) for the words this
plugin uses.

Continuity lives in `campaigns/<name>/sessions.log`, its timestamp
ISO-8601 UTC and its fields space-separated. A campaign with no
recorded conversation resumes as fresh, and says so.

## Worktrees (optional)

A campaign with no `worktree` key runs in its plain directory. With
one:

```yaml
worktree:
  repo: ~/code/site     # the repository to take a worktree of
  branch: wt/site-work  # created from the remote default branch if missing
```

`resume` and `fresh` ensure the worktree exists, use it as the
workspace cwd, and add it to the agent's paths. This plugin never
removes a worktree: herdr's own `worktree remove --force` deletes
uncommitted work, and removal stays a human act. It uses plain git, not
`herdr worktree`, because that family takes no `--env` (the address
could not be exported) and registers a stray base workspace.

## Environment

The agent process started in a workspace inherits the environment of
that workspace's own shell, which the herdr server spawns, not the
environment of whatever shell ran `herdr-estate` or `herdr`. Start the
herdr server from a clean shell. On every `workspace create`,
herdr-estate empties three names before adding the estate's own `env`
table, so a value from your own login session never rides along by
accident: `ANTHROPIC_API_KEY`, `TMUX`, `TMUX_PANE`. An estate table
entry for one of these wins over the empty value, since it's added
after. No other variable is touched.

## The verbs

![Both verbs read the workspace list first and refuse on a duplicate label, a running agent, a workspace they did not create, or a close they cannot confirm.](diagrams/verbs.svg)

- `herdr-estate resume [<address>]`: continue the campaign's latest
  conversation in a new workspace.
- `herdr-estate fresh [<address>]`: retire the latest conversation and
  start a new one in the same space.
- `herdr-estate list`: list every campaign the estate table knows.
- `--dry-run` prints the plan for `resume`/`fresh` and requires an
  explicit `<address>`, since deriving one from the current workspace
  is itself a herdr call.

Run as a herdr plugin action with no address, `<address>` is derived
from the current workspace's label.

## Picker

`herdr-estate pick` lists every campaign of every estate the table
knows, one row per address, in a small terminal UI:

- **live**: a workspace currently carries the address; its agent status
  (working, idle, done or unknown) shows next to it.
- **dormant**: no live workspace, but `sessions.log` records a
  conversation; the date of its last line shows next to it.
- **new**: neither.

Rows sort live first, then dormant by most recent, then new. Type to
filter by substring on the address; up/down or ctrl-p/ctrl-n move the
selection; Esc or ctrl-c quits without doing anything. Enter acts on the
selected row: live focuses the existing workspace, dormant resumes the
campaign and then focuses the workspace it opens, new does the same
through `fresh`. On Enter the row's workspace is read again at dispatch
time and routed through the same verb functions and guards, never
trusted from the drawn list. When the filter matches nothing, the
picker shows "no match" and Enter does nothing.

Open it as a popup over the active pane:

```sh
herdr plugin pane open --plugin herdr-estate --entrypoint picker
```

or bind a key to the `pick` action in your own
`~/.config/herdr/config.toml`, the same way `resume`/`fresh` are bound
above:

```toml
[[keys.command]]
key = "prefix+p"
type = "plugin_action"
command = "herdr-estate.pick"
```

`herdr-estate pick --render-once --width <W> --height <H>` prints a
single frame as plain text, with no terminal needed (defaults to 80x20).

## Worked example

Using the invented names in [`examples/`](examples/):

```sh
export HERDR_PLUGIN_CONFIG_DIR=$(mktemp -d)
cp examples/estates.toml "$HERDR_PLUGIN_CONFIG_DIR/estates.toml"
# edit its `root` to point at a real clone, e.g. one holding
# examples/home/campaigns/{garden,taxes}/campaign.md

cargo build --release
target/release/herdr-estate list
# home/garden                 new
# home/taxes                  new

target/release/herdr-estate fresh home/garden --dry-run
# would create:
#   herdr workspace create --cwd ... --label home/garden ...
# would start:
#   herdr agent start home-garden --kind claude --pane <PANE_ID> ...
```

## Tests

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Unit tests cover pure logic. Everything that crosses the herdr CLI
boundary is exercised against a fake `herdr` executable, pointed to by
`$HERDR_BIN_PATH`, that records its exact argv and environment per call
and replays canned responses, so a test asserts what herdr was actually
asked to do rather than that a mock was called. The integration suite
in `tests/cli.rs` drives the built binary itself: subcommands,
`--dry-run`, exit codes, and address derivation from
`$HERDR_WORKSPACE_ID`.
