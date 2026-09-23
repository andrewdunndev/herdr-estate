# Vocabulary

Six words, each defined once. herdr's own words (session, workspace,
tab, pane, agent) are only ever used in herdr's sense.

- **estate**: one repo that holds all your campaigns.
- **campaign**: a durable thread of work with its own directory inside
  the estate: a campaign file, a log, notes.
- **conversation**: one agent transcript. A campaign has many over its
  life, one at a time.
- **address**: `estate/campaign`, e.g. `home/garden`. It is the
  workspace label and it is exported to the agent.
- **resume**: continue the campaign's latest conversation.
- **fresh**: retire the latest conversation and start a new one at the
  same address, with the same paths, flags and prompts. The campaign's
  `log.md` is what carries the work across.
