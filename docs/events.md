# Progress and the event log

`resume` and `fresh` print one line to stderr before each step that
changes the board: the conversation they open, a close when one comes
first, the workspace create, the agent start, and the moment the agent
is ready. `agent start` returns only once herdr sees the agent ready for
input, several seconds for claude, so in the picker popup these lines
are what shows during that wait:

```
herdr-estate: home/garden: resuming conversation 4e2ef329...
herdr-estate: home/garden: creating a workspace
herdr-estate: home/garden: starting claude in w9, waiting up to 30s for it to be ready
herdr-estate: home/garden: claude is ready in w9
herdr-estate: home/garden: focusing w9
```

When a dispatch from the picker fails, the popup stays open on the error
until a key press. Keys typed during the wait are discarded first, so
they can't dismiss it unread.

A `--dry-run` prints no progress lines, and neither does a refusal made
before anything changes: a running agent, a duplicate label, a workspace
herdr-estate did not create. A close that can't be confirmed is refused
after its `closing` line, and nothing is created.

## events.log

Every run also appends to `events.log` in the state dir: a first line
with its argv and how it was launched (`herdr action <id>`, `herdr pane
<id>`, or `shell`), then the same progress lines, timestamped, then any
error. herdr keeps each plugin action's own output in `herdr plugin log
list`; the picker popup's output is gone once it closes. `events.log`
covers every launch path, one timestamped line per step.

The state dir, which also holds the ownership record, is
`$HERDR_PLUGIN_STATE_DIR` when herdr sets it (an action, or the picker
pane), otherwise `$XDG_STATE_HOME/herdr-estate` or
`~/.local/state/herdr-estate`. The log is created readable by you only,
and a failed herdr call's message masks every estate `env` value the way
a `--dry-run` plan does, on stderr and in the log alike. The log only
grows; delete it whenever.
