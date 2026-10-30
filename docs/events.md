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

A stop-then-fresh in a workspace with other panes keeps the workspace:
it prints a `stopping the agent in pane` line in place of the close, the
create, and the workspace id, and starts the agent in that same pane.

`create` makes the campaign's directory, then says one line before it
writes the two files; it never commits them:

```
herdr-estate: home/quern: creating campaign.md and log.md in /home/me/estates/home/campaigns/quern (untracked)
```

The picker's `+ create` runs that, then opens the campaign as a new row,
with the lines above. If the start fails, nothing is deleted: the
campaign stays as a new row, and one more line says so.

When a dispatch from the picker fails, the popup stays open on the error
until a key press. Keys typed during the wait are discarded first, so
they can't dismiss it unread.

`sort`, which herdr runs on every new workspace, says one line: how
many workspaces it moved into label order, or why it stopped. As
herdr's hook it never fails, so a move herdr refuses is only logged:

```
herdr-estate: sort: moved 1 workspace into label order
```

The picker's Sort spaces row shows that line in its detail line and
stays open; it writes nothing to stderr, only to `events.log`.

`--dry-run`, and any refusal made before something changes, print no
progress lines. A close that can't be confirmed is refused after its
`closing` line, and nothing is created. A `paths` entry that isn't
there gets one line once `prepare` has passed, just before any close;
a dry run lists it in its plan instead.

## events.log

Every run also appends to `events.log` in the state dir: a first line
with its argv and how it was launched (`herdr action <id>`, `herdr pane
<id>`, or `shell`), then the same progress lines, timestamped, then any
error. herdr keeps each plugin action's own output in `herdr plugin log
list`; the picker popup's output is gone once it closes. `events.log`
covers every launch path, one timestamped line per step.

The state dir, and what else it holds, is drawn in the README's [where
things live](../README.md#configure) figure. The log is created readable
by you only, and a failed herdr call's message masks every estate `env`
value the way a `--dry-run` plan does, on stderr and in the log alike.
The log only grows; delete it whenever.
