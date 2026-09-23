---
agent: claude
paths: [~/code/garden-notes]
claude:
  remote-control: true
---

# garden

## What this is

Plan the spring beds and keep the planting notes in ~/code/garden-notes.

A stand-in campaign under the invented `home` estate. Replace this
file's frontmatter and body with your own. The frontmatter applies to
this campaign alone: `agent` names the herdr agent kind (the estate's
default when absent), `paths` lists extra directories the agent may
work in, and the `claude:` table passes flags straight through to the
claude adapter (`remote-control: true` becomes a bare
`--remote-control`).
