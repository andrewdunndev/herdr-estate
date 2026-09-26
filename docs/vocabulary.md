# Vocabulary

Eight words, each defined once. herdr's own words (session, workspace,
tab, pane, agent) are only ever used in herdr's sense.

- **estate**: one repo that holds a set of campaigns, named by one
  `[estate.<name>]` table.
- **campaign**: a durable thread of work with its own directory inside
  the estate: a campaign file, a log, notes.
- **conversation**: one agent transcript. A campaign has many over its
  life, one at a time.
- **address**: `estate/campaign`, e.g. `home/garden`. It is the
  workspace label and it is exported to the agent.
- **paths**: extra directories the agent may reach, listed in the
  estate table and the campaign's frontmatter. One that doesn't exist
  is skipped, with a warning once the launch passes its guards.
- **prepare**: an estate's optional argv command, run in the
  campaign's workdir before any workspace is closed or created. A
  non-zero exit, or a run past 60 seconds, refuses the launch.
- **resume**: continue the campaign's latest conversation.
- **fresh**: retire the latest conversation and start a new one at the
  same address, with the same paths, flags and prompts. The campaign's
  `log.md` is what carries the work across.
