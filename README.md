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

![Each conversation runs in a workspace labeled home/garden; before it ends the agent writes log.md, and fresh starts the next conversation, which reads it.](diagrams/loop.svg)

A conversation costs more per turn the longer it runs, so recycle
often. The estate's `prompts` teach the agent to keep the log
([example](examples/home/prompts/base.md)).

## Install

Needs herdr 0.9.1+, macOS or Linux, and Rust 1.89+; herdr builds the
plugin on install.

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

For shell use, `cargo install --locked --git https://github.com/andrewdunndev/herdr-estate`
or a [release binary](https://gitlab.com/dunn.dev/herdr-estate/-/releases)
puts `herdr-estate` on your PATH.

## Configure

![estates.toml lives in herdr's plugin config dir; campaigns live under the estate root; logs, ownership, prompts, worktrees and locks live in the plugin's state dir.](diagrams/files.svg)

![An address names one estate table and one campaign directory, whose campaign.md sets the agent, its flags, paths and worktree.](diagrams/two-files.svg)

```toml
[estate.home]
root = "~/estates/home"
```

[examples/](examples/) is an annotated estate.

## Picker

![In the picker, Enter focuses, resumes or starts a campaign fresh; Ctrl-f starts fresh, and on an idle or done agent a second press stops it first.](diagrams/picker.svg)

## Verbs

![Both verbs read the workspace list first and refuse on a duplicate label, a running agent, a workspace they didn't create or an unconfirmed close.](diagrams/verbs.svg)

`herdr-estate --help` lists the verbs and their flags, and
[docs/events.md](docs/events.md) the progress lines they print.

## Develop

Review, CI and releases run on
[GitLab](https://gitlab.com/dunn.dev/herdr-estate);
[GitHub](https://github.com/andrewdunndev/herdr-estate) is a mirror.
Issues are welcome on either.

```sh
cargo build --release && herdr plugin link "$PWD"
cargo fmt --check && cargo clippy --release --all-targets --all-features -- -D warnings && cargo test
```

## License

MIT, in [LICENSE](LICENSE), except `src/picker/theme.rs`, adapted from
herdr under Apache-2.0, in [LICENSE-APACHE-HERDR](LICENSE-APACHE-HERDR).

---

Built by Andrew Dunn, 2026.
