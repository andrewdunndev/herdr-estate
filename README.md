# dunn.dev/herdr-estate

[![pipeline](https://gitlab.com/dunn.dev/herdr-estate/badges/main/pipeline.svg)](https://gitlab.com/dunn.dev/herdr-estate/-/pipelines)
[![license](https://img.shields.io/badge/license-MIT-green)](LICENSE)
![built with GitLab](https://img.shields.io/badge/built_with-GitLab-FC6D26?logo=gitlab)

herdr-estate is a plugin for the [herdr](https://herdr.dev) terminal
workspace manager. It gives herdr *estates* and *campaigns*: durable
threads of agent work, addressed as `estate/campaign`, that resume
where they left off instead of starting cold every time. The plugin
owns exactly two files — an estate table and a per-campaign frontmatter
block — plus a continuity log it reads and writes itself; herdr's own
resume is never used.

Brand artifacts (`avatar.svg`, `hero.svg`, `favicon.svg`, `brand.png`)
are deferred to this repo's pre-public sweep — see `.deviations.yaml`.

## Install

```sh
herdr plugin link /path/to/herdr-estate
```

`plugin link` is dev mode: it registers the path as-is, with no build
step, and edits take effect on the next herdr invocation. Then bind the
actions to keys in your own `~/.config/herdr/config.toml`:

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
prompts = ["prompts/base.md"]  # appended to every agent
env = { }                      # exported into every workspace
```

See [`examples/estates.toml`](examples/estates.toml).

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
adapter starts with no mapped flags and prints one warning line. See
[`docs/vocabulary.md`](docs/vocabulary.md) for the six words this
plugin uses and what each one means.

Continuity itself lives in `campaigns/<name>/sessions.log`: one line
per conversation — an ISO-8601 UTC timestamp, the hostname that opened
it, and the conversation id, space-separated. `resume` reads the last
line; `fresh` appends a new one. A campaign with no recorded
conversation resumes as fresh, and says so.

## The verbs

- `herdr-estate resume [<address>]` — continue the campaign's latest
  conversation in a new workspace.
- `herdr-estate fresh [<address>]` — retire the latest conversation and
  start a new one in the same space; closes the campaign's existing
  workspace first, but only once a live check confirms it's still ours.
- `herdr-estate list` — list every campaign the estate table knows.
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

bin/herdr-estate list
# home/garden                 new
# home/taxes                  new

bin/herdr-estate fresh home/garden --dry-run
# would create:
#   herdr workspace create --cwd ... --label home/garden ...
# would start:
#   herdr agent start home-garden --kind claude --pane <PANE_ID> ...
```

## Tests

Standard library only, `unittest`, mocking the one herdr-call boundary
(`herdr_estate.herdrcli.call_herdr`) and using temp directories for
every filesystem-touching test:

```sh
PYTHONPATH=src python3 -m unittest discover -s tests
```
