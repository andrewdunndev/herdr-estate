# dunn.dev/herdr-estate

[![pipeline](https://gitlab.com/dunn.dev/herdr-estate/badges/main/pipeline.svg)](https://gitlab.com/dunn.dev/herdr-estate/-/pipelines)
[![license](https://img.shields.io/badge/license-MIT-green)](LICENSE)
![built with GitLab](https://img.shields.io/badge/built_with-GitLab-FC6D26?logo=gitlab)

herdr-estate is a plugin for the [herdr](https://herdr.dev) terminal
workspace manager. It gives herdr *estates* and *campaigns*: durable
threads of agent work, addressed as `estate/campaign`, that resume
where they left off instead of starting cold every time. The plugin
owns exactly two files, an estate table and a per-campaign frontmatter
block, plus a continuity log it reads and writes itself; herdr's own
resume is never used.

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

**The estate table**, `estates.toml`, lives at
`$HERDR_PLUGIN_CONFIG_DIR/estates.toml` when herdr sets that (running as
a plugin action), or `~/.config/herdr-estate/estates.toml` for a bare
shell invocation. One `[estate.<name>]` table per estate:

```toml
[estate.home]
root = "~/estates/home"        # the repo that holds the campaigns
campaigns = "campaigns"        # directory under root
agent = "claude"               # default herdr agent kind
paths = ["~/code"]             # extra directories, added before each campaign's own
prompts = ["prompts/base.md"]  # appended to every agent
env = { }                      # exported into every workspace

[estate.home.claude]
remote-control = true          # default for every campaign in this estate
```

A relative `prompts` entry resolves against the estate's `root`, not the
directory herdr-estate happens to run from; `~` and `$VAR` expand
first, so an absolute or `~`-prefixed entry is unaffected. See
[`examples/estates.toml`](examples/estates.toml).

An estate's `paths` list is added ahead of each campaign's own list,
with a path repeated in both (after expansion) kept once; a
`[estate.<name>.<kind>]` table such as `[estate.home.claude]` sets
per-kind defaults for the whole estate that a campaign's own `claude:`
table overrides key by key, including turning a default off with an
explicit `false`.

**The campaign file**, `campaigns/<name>/campaign.md`, carries YAML
frontmatter: generic keys plus one table per agent kind for flags only
that kind understands.

```yaml
paths: [~/code/a, ~/code/b]   # extra directories the agent may work in
agent: claude                 # optional, overrides the estate default
claude:                       # flags only this agent kind understands
  remote-control: true
  mcp-config: .mcp.json
```

Unknown keys are ignored, not rejected, so an existing campaign file
keeps working after this plugin starts reading it. Generic keys map
through a small per-kind adapter (shipped: `claude`); a kind with no
adapter starts with no mapped flags and prints one warning line. A
relative `paths` entry resolves against the estate's `root`, the same
rule `prompts` follows above. See
[`docs/vocabulary.md`](docs/vocabulary.md) for the six words this
plugin uses and what each one means.

Continuity itself lives in `campaigns/<name>/sessions.log`: one line
per conversation, an ISO-8601 UTC timestamp, the hostname that opened
it, and the conversation id, space-separated. `resume` reads the last
line; `fresh` appends a new one. A campaign with no recorded
conversation resumes as fresh, and says so.

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

- `herdr-estate resume [<address>]`: continue the campaign's latest
  conversation in a new workspace.
- `herdr-estate fresh [<address>]`: retire the latest conversation and
  start a new one in the same space; closes the campaign's existing
  workspace first, but only once a live check confirms it's still ours.
  If that close cannot be confirmed, the verb stops instead of creating
  a second workspace under the same address. If more than one workspace
  already carries the address, both verbs refuse and name them for you
  to close by hand.
- `herdr-estate list`: list every campaign the estate table knows.
- `--dry-run` on `resume`/`fresh` prints the exact herdr commands and
  agent argv that would run and exits 0 without calling herdr; it
  requires an explicit `<address>`, since deriving one from the current
  workspace is itself a herdr call.

`<address>` is `estate/campaign`. Run as a herdr plugin action with no
address, it's derived from the current workspace's label.

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
