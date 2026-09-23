//! The picker's pure core: state, events, sorting and layout. No
//! terminal I/O, no herdr calls and no ratatui live here — `rows` builds
//! the input, `shell` drives this with real key events and paints the
//! `View` it returns, mapping each segment's `Tone` to one of herdr's
//! colours. The layout copies herdr's pane navigator (v0.9.1
//! overlays.rs:706-957): a filter line with a count, a rule, the rows, a
//! detail line and a key-hint footer, behind a one-column gutter.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    /// A workspace currently carries this address. `agent_status` is
    /// already rendered to text (`herdrcli::AgentStatus::as_str`).
    Live {
        workspace_id: String,
        agent_status: String,
    },
    /// No live workspace, but `sessions.log` records a conversation.
    /// `last` is the date of its latest line, `YYYY-MM-DD`.
    Dormant { last: String },
    /// No live workspace and no recorded conversation.
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub address: String,
    pub state: RowState,
}

fn state_rank(state: &RowState) -> u8 {
    match state {
        RowState::Live { .. } => 0,
        RowState::Dormant { .. } => 1,
        RowState::New => 2,
    }
}

/// Live first, then dormant by most recent (`last` sorts lexicographically
/// the same as chronologically, since it is `YYYY-MM-DD`), then new.
/// Within a group, ties break on address so the order is deterministic.
pub fn sort_rows(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by(|a, b| {
        state_rank(&a.state)
            .cmp(&state_rank(&b.state))
            .then_with(|| match (&a.state, &b.state) {
                (RowState::Dormant { last: la }, RowState::Dormant { last: lb }) => {
                    lb.cmp(la).then_with(|| a.address.cmp(&b.address))
                }
                _ => a.address.cmp(&b.address),
            })
    });
    rows
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Selected(Row),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct AppState {
    /// Every row, already sorted; `filtered()` narrows this by
    /// `self.filter` on every read rather than storing a second list.
    rows: Vec<Row>,
    pub filter: String,
    /// Tracked by address, not index, so it survives a filter change as
    /// long as the selected row still matches.
    selected_address: Option<String>,
    pub done: Option<Outcome>,
}

#[derive(Debug, Clone, Copy)]
pub enum Event {
    Char(char),
    Backspace,
    Up,
    Down,
    Enter,
    Esc,
}

impl AppState {
    pub fn new(rows: Vec<Row>) -> Self {
        let rows = sort_rows(rows);
        let selected_address = rows.first().map(|r| r.address.clone());
        AppState {
            rows,
            filter: String::new(),
            selected_address,
            done: None,
        }
    }

    pub fn filtered(&self) -> Vec<&Row> {
        self.rows
            .iter()
            .filter(|r| r.address.contains(&self.filter))
            .collect()
    }
}

fn current_index(filtered: &[&Row], selected: &Option<String>) -> Option<usize> {
    let addr = selected.as_deref()?;
    filtered.iter().position(|r| r.address == addr)
}

/// After the filter text changes, keep the current selection if it still
/// matches; otherwise fall back to the first row of the new filtered list
/// (or none, when nothing matches).
fn recompute_selection(state: &mut AppState) {
    let filtered = state.filtered();
    let still_valid = state
        .selected_address
        .as_deref()
        .is_some_and(|addr| filtered.iter().any(|r| r.address == addr));
    if still_valid {
        return;
    }
    state.selected_address = filtered.first().map(|r| r.address.clone());
}

pub fn update(mut state: AppState, event: Event) -> AppState {
    match event {
        Event::Char(c) => {
            state.filter.push(c);
            recompute_selection(&mut state);
        }
        Event::Backspace => {
            state.filter.pop();
            recompute_selection(&mut state);
        }
        Event::Up => {
            let filtered = state.filtered();
            if let Some(idx) = current_index(&filtered, &state.selected_address) {
                if idx > 0 {
                    state.selected_address = Some(filtered[idx - 1].address.clone());
                }
            }
        }
        Event::Down => {
            let filtered = state.filtered();
            if let Some(idx) = current_index(&filtered, &state.selected_address) {
                if idx + 1 < filtered.len() {
                    state.selected_address = Some(filtered[idx + 1].address.clone());
                }
            }
        }
        Event::Enter => {
            let filtered = state.filtered();
            // Enter on an empty filtered list (or a selection that
            // somehow names no row) does nothing: no `Outcome` is set,
            // and the picker stays open.
            if let Some(idx) = current_index(&filtered, &state.selected_address) {
                state.done = Some(Outcome::Selected(filtered[idx].clone()));
            }
        }
        Event::Esc => {
            state.done = Some(Outcome::Cancelled);
        }
    }
    state
}

/// Which glyph set marks a live row's status: herdr's
/// `[ui].status_indicators`, which `theme::resolve` reads from herdr's
/// own config. `Dots` is herdr's default (config/model.rs:113-117).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indicators {
    Dots,
    Symbols,
}

/// A live row's agent status, in the five kinds herdr draws a status
/// icon for (client/shell.rs:177-236). The row keeps the wire string;
/// it is classified only here, where the glyph and colour are chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Working,
    Blocked,
    Done,
    Idle,
    Unknown,
}

impl Status {
    /// Classify `agent_status` as `herdrcli::AgentStatus::as_str`
    /// renders it. "blocked" arrives as `Other("blocked")` and renders
    /// as "blocked"; anything unrecognized is `Unknown`, which herdr
    /// draws muted.
    pub fn from_wire(s: &str) -> Status {
        match s {
            "working" => Status::Working,
            "blocked" => Status::Blocked,
            "done" => Status::Done,
            "idle" => Status::Idle,
            _ => Status::Unknown,
        }
    }

    /// herdr's `status_icon`. Every glyph is one cell wide, so the
    /// addresses after it line up whatever the status.
    pub fn glyph(self, ind: Indicators) -> &'static str {
        match (ind, self) {
            (Indicators::Dots, Status::Working | Status::Blocked | Status::Done) => "●",
            (Indicators::Dots, Status::Idle) => "○",
            (Indicators::Dots, Status::Unknown) => "·",
            (Indicators::Symbols, Status::Working) => "◐",
            (Indicators::Symbols, Status::Blocked) => "×",
            (Indicators::Symbols, Status::Done) => "✓",
            (Indicators::Symbols, Status::Idle) => "○",
            (Indicators::Symbols, Status::Unknown) => "·",
        }
    }

    /// herdr's `status_text`: the word a live row's meta shows.
    pub fn word(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Blocked => "blocked",
            Status::Done => "done",
            Status::Idle => "idle",
            Status::Unknown => "unknown",
        }
    }
}

/// What a segment of a `ViewLine` is, for the shell to colour. It names
/// herdr's colour token by role rather than by value, so this core
/// stays free of ratatui. On a selected line every segment after the
/// gutter takes the selection bar's style instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Blank panel: the gutter, padding and the spaces between parts.
    Fill,
    /// The "/" prompt, the query and a live row's address (`text`).
    Text,
    /// A dormant or new row's address (`subtext0`).
    Subtext,
    /// The placeholder, the count, meta, the detail line, the footer
    /// and the empty states (`overlay0`).
    Muted,
    /// The ─ rule under the filter line (`surface1`).
    Rule,
    /// A live row's status glyph, in that status's colour.
    Status(Status),
}

/// A run of text drawn in one tone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub tone: Tone,
}

/// One terminal row of the picker. Its `segs` add up to exactly the
/// frame's width in cells, and `segs[0]` is the one-cell gutter in
/// `Tone::Fill` (a zero-width frame has no segments at all). `selected`
/// marks the row the selection bar covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewLine {
    pub segs: Vec<Seg>,
    pub selected: bool,
}

impl ViewLine {
    /// The line as plain text, for `--render-once` and for tests: the
    /// segments joined, '>' in the gutter of the selected line (plain
    /// text has no bar to show it with), trailing spaces trimmed.
    pub fn plain(&self) -> String {
        let mut text: String = self.segs.iter().map(|seg| seg.text.as_str()).collect();
        if self.selected && text.starts_with(' ') {
            text.replace_range(..1, ">");
        }
        text.trim_end().to_string()
    }
}

/// One frame: exactly as many lines as its height, and where the
/// terminal cursor goes as (column, row), if anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub lines: Vec<ViewLine>,
    pub cursor: Option<(u16, u16)>,
}

/// The column the query, the placeholder and every row's address start
/// at: after the gutter, a leading space and "/ " (or a status glyph
/// and its space), so what is typed lines up with what it filters.
const TEXT_COL: usize = 4;

/// Shown muted in the empty filter, with the cursor on its first cell.
const PLACEHOLDER: &str = "filter campaigns";

/// The footer's key hints, verb then key: herdr's navigator search
/// footer (overlays.rs:937) with "search" as "filter" and "back" as
/// "close". ctrl+c also closes, unlisted, as herdr's own footers leave
/// out alternate keys.
const FOOTER: [&str; 4] = ["filter type", "move ↑↓/ctrl+n/p", "open enter", "close esc"];

/// With fewer cells than this left for an address, a row drops its meta
/// to give the address that room back [ours; the navigator clips meta,
/// overlays.rs:909-917].
const MIN_ADDRESS_CELLS: usize = 8;

/// `s` split into the clusters a terminal draws as one unit: UAX #29
/// extended grapheme clusters, from unicode-segmentation, the splitter
/// ratatui itself paints by (the same locked version). Text is only ever
/// cut between these clusters and measured one at a time, so each line is
/// exactly as wide where ratatui paints it as the core counted, and an
/// emoji is never cut from its variation selector or its joined parts.
fn clusters(s: &str) -> Vec<&str> {
    s.graphemes(true).collect()
}

/// The cells one cluster takes, as ratatui measures it (ratatui-core's
/// `CellWidth` for `str`): unicode-width's width of the whole cluster,
/// plus one for each halfwidth katakana sound mark (U+FF9E, U+FF9F),
/// which unicode-width counts as nothing but terminals draw in a cell
/// of its own. A control char takes none, since ratatui drops it,
/// though unicode-width would give it one.
fn cluster_cells(cluster: &str) -> usize {
    if cluster.starts_with(char::is_control) {
        return 0;
    }
    cluster.width()
        + cluster
            .chars()
            .filter(|c| matches!(c, '\u{FF9E}' | '\u{FF9F}'))
            .count()
}

/// The cells `s` takes on a terminal, one cluster at a time, as ratatui
/// paints it: what every line's "exactly the width" is counted in.
fn cells(s: &str) -> usize {
    clusters(s).into_iter().map(cluster_cells).sum()
}

/// The longest prefix of `s` that fits in `max` cells: a hard clip,
/// always between clusters. A wide cluster that would straddle the edge
/// is left out whole, and an emoji is never cut from its variation
/// selector or its joined parts.
fn clip(s: &str, max: usize) -> &str {
    let mut used = 0;
    let mut end = 0;
    for cluster in clusters(s) {
        used += cluster_cells(cluster);
        if used > max {
            break;
        }
        end += cluster.len();
    }
    &s[..end]
}

/// `s` cut to `max` cells, ending in "…" when anything was cut [ours:
/// herdr's `truncate_end` (ui/text.rs:7-20), which its sidebar and pane
/// titles use; its modals hard-clip].
fn ellipsize(s: &str, max: usize) -> String {
    if cells(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    format!("{}…", clip(s, max - 1))
}

/// The end of `query` that an editor `width` cells wide shows with the
/// cursor after it: the longest suffix narrower than `width`, as
/// herdr's text editor scrolls (text_editor.rs:240-264). What was typed
/// last stays in view, the cursor cell stays inside the editor, and the
/// cut falls between clusters, as `clip`'s does.
fn tail_before_cursor(query: &str, width: usize) -> &str {
    let mut start = query.len();
    let mut used = 0;
    for cluster in clusters(query).into_iter().rev() {
        used += cluster_cells(cluster);
        if used >= width {
            break;
        }
        start -= cluster.len();
    }
    &query[start..]
}

/// The cells left for text that starts at `TEXT_COL` with `right`
/// right-aligned after it: less the four cells before it, one blank
/// cell between the two, and the trailing cell. The filter line's
/// editor beside the count and a row's address beside its meta share
/// this geometry.
fn room_beside(width: usize, right: &str) -> usize {
    width.saturating_sub(TEXT_COL + 2 + cells(right))
}

/// The cells left for text that starts at `TEXT_COL` with nothing
/// after it: less the four cells before it and the trailing cell.
fn room_alone(width: usize) -> usize {
    width.saturating_sub(TEXT_COL + 1)
}

/// The cells between the leading space and the trailing cell, columns
/// 2..=W−2, where the detail line, the footer and the empty states go.
fn text_room(width: usize) -> usize {
    width.saturating_sub(3)
}

/// One line under construction, left to right. Every push is clipped so
/// the line can never pass `width` cells, and `finish` pads it with
/// `Fill` to exactly `width`: the layout functions place the text, and
/// this keeps the contract that each line is the frame's full width.
struct LineBuilder {
    width: usize,
    used: usize,
    segs: Vec<Seg>,
}

impl LineBuilder {
    /// A line whose first cell is the gutter: column 0, always blank
    /// panel and never selected. herdr always keeps one blank panel
    /// column inside the popup's right border and none inside its left
    /// (popup_size.rs:75-79); the gutter mirrors it, so text sits clear
    /// of both borders.
    fn new(width: usize) -> Self {
        let mut line = LineBuilder {
            width,
            used: 0,
            segs: Vec::new(),
        };
        line.push(" ", Tone::Fill);
        line
    }

    /// Append `text` in `tone`, clipped to the cells left. Text that
    /// clips to nothing is not pushed, so no segment is ever empty.
    fn push(&mut self, text: &str, tone: Tone) {
        let text = clip(text, self.width - self.used);
        if text.is_empty() {
            return;
        }
        self.used += cells(text);
        self.segs.push(Seg {
            text: text.to_string(),
            tone,
        });
    }

    /// Pad with `Fill` up to column `col`.
    fn pad_to(&mut self, col: usize) {
        self.push(&" ".repeat(col.saturating_sub(self.used)), Tone::Fill);
    }

    /// Append `text` right-aligned, with one trailing cell after it
    /// [ours; herdr's `put_right_text` is flush to the border, render.rs
    /// 342-352]. Right-aligned text then sits two cells in from herdr's
    /// border, as left text does behind the gutter and leading space.
    fn push_right(&mut self, text: &str, tone: Tone) {
        self.pad_to(self.width.saturating_sub(1 + cells(text)));
        self.push(text, tone);
    }

    /// The finished line, padded with `Fill` to exactly `width` cells
    /// however much text went in, so no layout function has to count
    /// its own padding. `selected` puts the selection bar over it.
    fn finish(mut self, selected: bool) -> ViewLine {
        self.pad_to(self.width);
        ViewLine {
            segs: self.segs,
            selected,
        }
    }
}

/// Which of the frame's fixed lines fit a given height [ours; herdr
/// computes body = inner.h − 4, overlays.rs:788]. The filter line
/// always shows; the rule needs 3 rows, the footer 5 and the detail
/// line 7, and the list gets whatever is left.
struct Tiers {
    rule: bool,
    footer: bool,
    detail: bool,
    list: usize,
}

impl Tiers {
    /// The lines a frame `height` rows tall has room for. Each fixed
    /// line drops out below its row count (detail first, then footer,
    /// then rule), so a short popup still shows the filter and as many
    /// rows as it can.
    fn for_height(height: usize) -> Tiers {
        let rule = height >= 3;
        let footer = height >= 5;
        let detail = height >= 7;
        let fixed = 1 + usize::from(rule) + usize::from(footer) + usize::from(detail);
        Tiers {
            rule,
            footer,
            detail,
            list: height.saturating_sub(fixed),
        }
    }
}

/// The first row index to draw so a `window`-tall page still contains
/// `selected_idx` out of `total` rows. No separate "page" concept —
/// just the smallest shift that keeps the selection on screen, clamped
/// so the window never scrolls past the last page.
fn scroll_offset(selected_idx: usize, total: usize, window: usize) -> usize {
    if window == 0 || total <= window {
        return 0;
    }
    let max_offset = total - window;
    selected_idx.saturating_sub(window - 1).min(max_offset)
}

/// Lay `state` out as one `width` x `height` frame in herdr's navigator
/// layout: the filter line, a rule, the list, a detail line for the
/// selected row and a key-hint footer, as far as the height tiers allow
/// (`Tiers::for_height`). Every line is exactly `width` cells, and
/// nothing wraps. When more rows match than the list holds, it scrolls
/// so the selected row is always drawn. No vertical padding: herdr's
/// modals put their first line right under the border.
pub fn view(state: &AppState, width: usize, height: usize, ind: Indicators) -> View {
    if height == 0 {
        return View {
            lines: Vec::new(),
            cursor: None,
        };
    }
    let tiers = Tiers::for_height(height);
    let filtered = state.filtered();
    let selected_idx = current_index(&filtered, &state.selected_address);

    let (filter, cursor) = filter_line(&state.filter, filtered.len(), state.rows.len(), width);
    let mut lines = vec![filter];
    if tiers.rule {
        lines.push(rule_line(width));
    }
    lines.extend(list_lines(
        state,
        &filtered,
        selected_idx,
        width,
        tiers.list,
        ind,
    ));
    if tiers.detail {
        lines.push(detail_line(selected_idx.map(|i| filtered[i]), width));
    }
    if tiers.footer {
        lines.push(footer_line(width));
    }
    View { lines, cursor }
}

/// Row 0: " / " then the query (or the muted placeholder), and the
/// count right-aligned, "N campaigns" or "N/M campaigns" while a filter
/// narrows the list (herdr's open-worktree count,
/// worktree_overlays.rs:150-154). Only a lone campaign reads singular,
/// "1 campaign" [ours]; an N/M count is always plural, as herdr's
/// counts are, so one campaign filtered away reads "0/1 campaigns".
/// Returns where the cursor goes: after the query, or on the
/// placeholder's first cell; `None` in a frame too narrow to hold
/// `TEXT_COL`.
fn filter_line(
    query: &str,
    shown: usize,
    total: usize,
    width: usize,
) -> (ViewLine, Option<(u16, u16)>) {
    let count = if shown != total {
        format!("{shown}/{total} campaigns")
    } else if total == 1 {
        "1 campaign".to_string()
    } else {
        format!("{total} campaigns")
    };
    // The editor ends one blank cell before the count, as herdr's
    // `inner − 4 − count` does (overlays.rs:759-766), so the query and
    // cursor never run into it. Only a count that leaves the editor no
    // cell at all is dropped, and the editor then runs to the trailing
    // cell.
    let beside_count = room_beside(width, &count);
    let (editor, count) = if beside_count > 0 {
        (beside_count, Some(count))
    } else {
        (room_alone(width), None)
    };
    let mut line = LineBuilder::new(width);
    line.push(" ", Tone::Fill);
    line.push("/", Tone::Text);
    line.push(" ", Tone::Fill);
    let cursor_col = if query.is_empty() {
        line.push(clip(PLACEHOLDER, editor), Tone::Muted);
        TEXT_COL
    } else {
        let visible = tail_before_cursor(query, editor);
        line.push(visible, Tone::Text);
        TEXT_COL + cells(visible)
    };
    if let Some(count) = count {
        line.push_right(&count, Tone::Muted);
    }
    let cursor = (width > TEXT_COL).then(|| (u16::try_from(cursor_col).unwrap_or(u16::MAX), 0));
    (line.finish(false), cursor)
}

/// Row 1: "─" across everything but the gutter, in `surface1`
/// (overlays.rs:780-787).
fn rule_line(width: usize) -> ViewLine {
    let mut line = LineBuilder::new(width);
    line.push(&"─".repeat(width.saturating_sub(1)), Tone::Rule);
    line.finish(false)
}

/// The list: exactly `height` lines. The window of matching rows that
/// holds the selection, or an empty state on the first line, then blank
/// lines. With no campaigns at all it says " no campaigns" [ours],
/// whatever is typed, as the count above it says "0 campaigns".
/// Otherwise an empty list means a typed filter hides every campaign
/// (an empty filter matches them all), and it says " no matching
/// campaigns" (herdr's " no matching worktrees",
/// worktree_overlays.rs:248).
fn list_lines(
    state: &AppState,
    filtered: &[&Row],
    selected_idx: Option<usize>,
    width: usize,
    height: usize,
    ind: Indicators,
) -> Vec<ViewLine> {
    let mut lines: Vec<ViewLine> = if filtered.is_empty() {
        let message = if state.rows.is_empty() {
            "no campaigns"
        } else {
            "no matching campaigns"
        };
        vec![note_line(message, width)]
    } else {
        let offset = scroll_offset(selected_idx.unwrap_or(0), filtered.len(), height);
        filtered
            .iter()
            .skip(offset)
            .take(height)
            .map(|row| {
                let selected = state.selected_address.as_deref() == Some(row.address.as_str());
                row_line(row, width, ind, selected)
            })
            .collect()
    };
    lines.truncate(height);
    lines.resize_with(height, || LineBuilder::new(width).finish(false));
    lines
}

/// A list row, " {glyph} {address}" with the meta right-aligned
/// (adapted from the navigator's " {tree} {status} {label}",
/// overlays.rs:861-865). The glyph is at column 2 and the address at
/// `TEXT_COL`, under the query. A dormant or new row has no status and
/// leaves the glyph cell blank, so every address starts in the same
/// column. The address shrinks to fit, ending in "…", and takes the
/// meta's room when fewer than `MIN_ADDRESS_CELLS` would be left.
fn row_line(row: &Row, width: usize, ind: Indicators, selected: bool) -> ViewLine {
    let (glyph, glyph_tone, address_tone, meta) = match &row.state {
        RowState::Live { agent_status, .. } => {
            let status = Status::from_wire(agent_status);
            (
                status.glyph(ind),
                Tone::Status(status),
                Tone::Text,
                status.word().to_string(),
            )
        }
        RowState::Dormant { last } => (" ", Tone::Fill, Tone::Subtext, format!("last {last}")),
        RowState::New => (" ", Tone::Fill, Tone::Subtext, "new".to_string()),
    };
    let beside_meta = room_beside(width, &meta);
    let (room, meta) = if beside_meta >= MIN_ADDRESS_CELLS {
        (beside_meta, Some(meta))
    } else {
        (room_alone(width), None)
    };
    let mut line = LineBuilder::new(width);
    line.push(" ", Tone::Fill);
    line.push(glyph, glyph_tone);
    line.push(" ", Tone::Fill);
    line.push(&ellipsize(&row.address, room), address_tone);
    if let Some(meta) = meta {
        line.push_right(&meta, Tone::Muted);
    }
    line.finish(selected)
}

/// Row H−2: " {address} · {action}" for the selected row, the
/// navigator's detail line (overlays.rs:920-930). The action says what
/// Enter will do, in `dispatch::open_row`'s terms. Hard-clipped with no
/// "…", as herdr clips its own detail line (`put_text`, render.rs:
/// 367-372): the ellipsis is only for a row's address. Blank when
/// nothing is selected.
fn detail_line(row: Option<&Row>, width: usize) -> ViewLine {
    let Some(row) = row else {
        return LineBuilder::new(width).finish(false);
    };
    let action = match &row.state {
        RowState::Live { workspace_id, .. } => format!("focus {workspace_id}"),
        RowState::Dormant { last } => format!("resume from {last}"),
        RowState::New => "start fresh".to_string(),
    };
    note_line(&format!("{} · {action}", row.address), width)
}

/// Row H−1: the key hints. When the footer is too narrow for all of
/// them, whole hints drop from the end [ours; herdr just clips].
fn footer_line(width: usize) -> ViewLine {
    let room = text_room(width);
    let mut text = String::new();
    for hint in FOOTER {
        let next = if text.is_empty() {
            hint.to_string()
        } else {
            format!("{text} · {hint}")
        };
        if cells(&next) > room {
            break;
        }
        text = next;
    }
    note_line(&text, width)
}

/// A muted line of text after the leading space, hard-clipped to the
/// text columns.
fn note_line(text: &str, width: usize) -> ViewLine {
    let mut line = LineBuilder::new(width);
    line.push(" ", Tone::Fill);
    line.push(clip(text, text_room(width)), Tone::Muted);
    line.finish(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(address: &str, state: RowState) -> Row {
        Row {
            address: address.to_string(),
            state,
        }
    }

    fn live(address: &str, status: &str) -> Row {
        row(
            address,
            RowState::Live {
                workspace_id: format!("w-{address}"),
                agent_status: status.to_string(),
            },
        )
    }

    fn dormant(address: &str, last: &str) -> Row {
        row(
            address,
            RowState::Dormant {
                last: last.to_string(),
            },
        )
    }

    fn new_row(address: &str) -> Row {
        row(address, RowState::New)
    }

    fn sample_rows() -> Vec<Row> {
        vec![
            dormant("home/kitchen", "2026-09-10"),
            new_row("home/attic"),
            live("home/garden", "working"),
            dormant("home/taxes", "2026-09-18"),
            live("lab/bench", "idle"),
            new_row("home/cellar"),
        ]
    }

    #[test]
    fn sort_order_puts_live_first_then_dormant_by_recency_then_new() {
        let sorted = sort_rows(sample_rows());
        let addrs: Vec<&str> = sorted.iter().map(|r| r.address.as_str()).collect();
        assert_eq!(
            addrs,
            vec![
                "home/garden",  // live
                "lab/bench",    // live
                "home/taxes",   // dormant, 2026-09-18 (more recent)
                "home/kitchen", // dormant, 2026-09-10
                "home/attic",   // new
                "home/cellar",  // new
            ]
        );
    }

    #[test]
    fn sort_is_stable_across_repeated_calls() {
        let once = sort_rows(sample_rows());
        let twice = sort_rows(once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn filtering_narrows_by_address_substring() {
        let state = AppState::new(sample_rows());
        let state = update(state, Event::Char('b'));
        let state = update(state, Event::Char('e'));
        let state = update(state, Event::Char('n'));
        let addrs: Vec<_> = state.filtered().iter().map(|r| r.address.clone()).collect();
        assert_eq!(addrs, vec!["lab/bench".to_string()]);
    }

    #[test]
    fn movement_clamps_at_the_top() {
        let state = AppState::new(sample_rows()); // selected = home/garden (rank 0)
        let state = update(state, Event::Up);
        assert_eq!(state.selected_address.as_deref(), Some("home/garden"));
    }

    #[test]
    fn movement_clamps_at_the_bottom() {
        let mut state = AppState::new(sample_rows());
        for _ in 0..10 {
            state = update(state, Event::Down);
        }
        assert_eq!(state.selected_address.as_deref(), Some("home/cellar"));
    }

    #[test]
    fn up_from_below_the_top_reselects_the_previous_row() {
        // sample_rows sorts to: home/garden, lab/bench, home/taxes, ...
        // Down twice then Up once must land back on the middle row, not
        // clamp to the top (the only Up behaviour the existing tests
        // exercised) or stay put.
        let mut state = AppState::new(sample_rows());
        state = update(state, Event::Down); // lab/bench
        state = update(state, Event::Down); // home/taxes
        state = update(state, Event::Up); // back to lab/bench
        assert_eq!(state.selected_address.as_deref(), Some("lab/bench"));
    }

    #[test]
    fn enter_on_empty_filter_result_does_nothing() {
        let state = AppState::new(sample_rows());
        let state = update(state, Event::Char('z'));
        let state = update(state, Event::Char('z'));
        assert!(state.filtered().is_empty());
        let state = update(state, Event::Enter);
        assert_eq!(state.done, None);
    }

    #[test]
    fn enter_selects_the_current_row() {
        let mut state = AppState::new(sample_rows());
        state = update(state, Event::Down); // lab/bench
        state = update(state, Event::Enter);
        assert_eq!(
            state.done,
            Some(Outcome::Selected(live("lab/bench", "idle")))
        );
    }

    #[test]
    fn esc_cancels_regardless_of_filter_or_selection() {
        let state = AppState::new(sample_rows());
        let state = update(state, Event::Char('x'));
        let state = update(state, Event::Esc);
        assert_eq!(state.done, Some(Outcome::Cancelled));
    }

    #[test]
    fn selection_survives_a_filter_change_and_restores_on_widen() {
        let mut state = AppState::new(sample_rows());
        state = update(state, Event::Down); // lab/bench
        assert_eq!(state.selected_address.as_deref(), Some("lab/bench"));
        state = update(state, Event::Char('b'));
        assert_eq!(state.selected_address.as_deref(), Some("lab/bench"));
        state = update(state, Event::Backspace);
        assert_eq!(state.selected_address.as_deref(), Some("lab/bench"));
    }

    #[test]
    fn selection_falls_back_to_first_match_when_it_no_longer_matches() {
        let mut state = AppState::new(sample_rows());
        state = update(state, Event::Down); // lab/bench
                                            // "home" excludes lab/bench (unlike a bare "h", which "bench"
                                            // still contains) and narrows to the home/* rows only.
        for c in ['h', 'o', 'm', 'e'] {
            state = update(state, Event::Char(c));
        }
        assert_eq!(state.selected_address.as_deref(), Some("home/garden"));
    }

    fn typed(mut state: AppState, text: &str) -> AppState {
        for c in text.chars() {
            state = update(state, Event::Char(c));
        }
        state
    }

    fn plain(view: &View) -> Vec<String> {
        view.lines.iter().map(ViewLine::plain).collect()
    }

    fn line_cells(line: &ViewLine) -> usize {
        line.segs.iter().map(|seg| cells(&seg.text)).sum()
    }

    /// The column the first segment starting with `text` begins at.
    fn column_of(line: &ViewLine, text: &str) -> Option<usize> {
        let mut col = 0;
        for seg in &line.segs {
            if seg.text.starts_with(text) {
                return Some(col);
            }
            col += cells(&seg.text);
        }
        None
    }

    /// The tone of the segment whose text is exactly `text`.
    fn tone_of(line: &ViewLine, text: &str) -> Option<Tone> {
        line.segs
            .iter()
            .find(|seg| seg.text == text)
            .map(|seg| seg.tone)
    }

    #[test]
    fn view_renders_a_literal_frame_for_one_row_of_each_state() {
        // Hand-transcribed expected text, not view()'s own output over
        // equivalent rows: pins the gutter, the " / " prompt, the count,
        // the glyph and address columns, the right-aligned meta with its
        // trailing cell, the detail wording and the footer to literal
        // strings, so a layout regression fails here instead of only
        // failing a golden test that compares view() against itself.
        // The padding widths are counted by hand: 60 cells, less the
        // text on each side.
        let rows = vec![
            live("home/garden", "working"),
            dormant("home/kitchen", "2026-09-10"),
            new_row("home/attic"),
        ];
        let state = AppState::new(rows);
        let v = view(&state, 60, 10, Indicators::Symbols);
        let pad = |n: usize| " ".repeat(n);
        assert_eq!(
            plain(&v),
            vec![
                format!("  / filter campaigns{}3 campaigns", pad(28)),
                format!(" {}", "─".repeat(59)),
                format!("> ◐ home/garden{}working", pad(37)),
                format!("    home/kitchen{}last 2026-09-10", pad(28)),
                format!("    home/attic{}new", pad(42)),
                String::new(),
                String::new(),
                String::new(),
                "  home/garden · focus w-home/garden".to_string(),
                "  filter type · move ↑↓/ctrl+n/p · open enter · close esc".to_string(),
            ]
        );
        assert_eq!(v.cursor, Some((4, 0)));
        assert_eq!(v.lines.iter().filter(|l| l.selected).count(), 1);
        assert!(v.lines[2].selected);

        assert_eq!(tone_of(&v.lines[0], "/"), Some(Tone::Text));
        assert_eq!(tone_of(&v.lines[0], "filter campaigns"), Some(Tone::Muted));
        assert_eq!(tone_of(&v.lines[0], "3 campaigns"), Some(Tone::Muted));
        assert_eq!(v.lines[1].segs[1].tone, Tone::Rule);
        assert_eq!(
            tone_of(&v.lines[2], "◐"),
            Some(Tone::Status(Status::Working))
        );
        assert_eq!(tone_of(&v.lines[2], "home/garden"), Some(Tone::Text));
        assert_eq!(tone_of(&v.lines[2], "working"), Some(Tone::Muted));
        assert_eq!(tone_of(&v.lines[3], "home/kitchen"), Some(Tone::Subtext));
        assert_eq!(tone_of(&v.lines[3], "last 2026-09-10"), Some(Tone::Muted));
        assert_eq!(tone_of(&v.lines[4], "home/attic"), Some(Tone::Subtext));
        assert_eq!(v.lines[8].segs[2].tone, Tone::Muted);
        assert_eq!(v.lines[9].segs[2].tone, Tone::Muted);
    }

    #[test]
    fn view_truncates_every_line_at_narrow_widths() {
        let state = AppState::new(sample_rows());
        let v = view(&state, 8, 20, Indicators::Dots);
        for line in plain(&v) {
            assert!(line.chars().count() <= 8, "line too wide: {line:?}");
        }
    }

    #[test]
    fn view_reports_no_match() {
        let state = typed(AppState::new(sample_rows()), "qqq");
        let v = view(&state, 40, 20, Indicators::Dots);
        assert_eq!(v.lines[2].plain(), "  no matching campaigns");
        assert_eq!(v.lines[2].segs[2].tone, Tone::Muted);
    }

    #[test]
    fn view_truncates_a_row_whose_address_is_longer_than_the_width() {
        let rows = vec![new_row(
            "an-estate-with-a-very-long-name/and-an-equally-long-campaign-name",
        )];
        let state = AppState::new(rows);
        let v = view(&state, 12, 20, Indicators::Dots);
        assert!(v.lines.iter().all(|l| line_cells(l) == 12));
        // The row is still the selected one and still starts with the
        // address, cut with "…" rather than dropped.
        assert!(v.lines[2].selected);
        assert_eq!(v.lines[2].plain(), ">   an-est…");
    }

    #[test]
    fn view_at_zero_height_is_empty() {
        let state = AppState::new(sample_rows());
        assert_eq!(
            view(&state, 40, 0, Indicators::Dots),
            View {
                lines: Vec::new(),
                cursor: None,
            }
        );
    }

    #[test]
    fn view_at_zero_width_never_panics() {
        let state = AppState::new(sample_rows());
        let v = view(&state, 0, 5, Indicators::Dots);
        assert_eq!(v.lines.len(), 5);
        assert!(v.lines.iter().all(|l| l.segs.is_empty()));
        assert!(plain(&v).iter().all(|l| l.is_empty()));
        assert_eq!(v.cursor, None);
    }

    #[test]
    fn no_campaigns_at_all_says_so_whatever_is_typed() {
        let state = AppState::new(Vec::new());
        let v = view(&state, 40, 20, Indicators::Dots);
        assert_eq!(v.lines[2].plain(), "  no campaigns");
        // A filter typed into an empty list changes nothing: there are
        // no campaigns for it to match, as the count still says.
        let v = view(&typed(state, "x"), 40, 20, Indicators::Dots);
        assert_eq!(v.lines[2].plain(), "  no campaigns");
        assert!(v.lines[0].plain().ends_with(" 0 campaigns"));
    }

    #[test]
    fn no_match_at_height_one_returns_only_the_filter_line() {
        // The filter line alone already fills a 1-line frame; the
        // empty state must not be pushed past it.
        let state = typed(AppState::new(sample_rows()), "zzz");
        let v = view(&state, 40, 1, Indicators::Dots);
        assert_eq!(v.lines.len(), 1);
        assert!(!v.lines[0].selected);
        assert!(v.lines[0].plain().starts_with("  / zzz"), "{:?}", plain(&v));
    }

    fn many_rows(n: usize) -> Vec<Row> {
        (0..n).map(|i| new_row(&format!("home/c{i:02}"))).collect()
    }

    #[test]
    fn view_drops_rows_past_height_with_no_scroll_regression() {
        // Baseline: with the selection at the top, a 6-line frame is the
        // filter line, the rule, the first 3 rows and the footer.
        let state = AppState::new(many_rows(25));
        let v = view(&state, 40, 6, Indicators::Dots);
        assert_eq!(v.lines.len(), 6);
        assert_eq!(v.lines[1].segs[1].tone, Tone::Rule);
        assert!(v.lines[2].selected);
        assert!(v.lines[2].plain().contains("home/c00"));
        assert!(v.lines[4].plain().contains("home/c02"));
        assert!(v.lines[5].plain().starts_with("  filter type"));
    }

    #[test]
    fn view_scrolls_the_window_so_a_selection_past_the_page_is_still_drawn() {
        let mut state = AppState::new(many_rows(25));
        for _ in 0..24 {
            state = update(state, Event::Down);
        }
        // Selection is now the last row, home/c24 — with no scrolling
        // this would sit past a 6-line frame (3-row list) and never be
        // drawn.
        let v = view(&state, 40, 6, Indicators::Dots);
        assert_eq!(v.lines.len(), 6);
        assert!(
            v.lines
                .iter()
                .any(|l| l.selected && l.plain().contains("home/c24")),
            "{:?}",
            plain(&v)
        );
    }

    #[test]
    fn view_scrolls_back_to_the_top_page_when_selection_returns_there() {
        let mut state = AppState::new(many_rows(25));
        for _ in 0..24 {
            state = update(state, Event::Down);
        }
        for _ in 0..24 {
            state = update(state, Event::Up);
        }
        let v = view(&state, 40, 6, Indicators::Dots);
        // Selection is back at the very first row: the window must have
        // scrolled back up rather than staying pinned to the bottom.
        assert!(
            v.lines[2].selected && v.lines[2].plain().contains("home/c00"),
            "{:?}",
            plain(&v)
        );
    }

    #[test]
    fn every_line_is_exactly_the_width_in_cells() {
        // Wide (two-cell) characters, emoji sequences whose cells differ
        // from their chars', an accent, a long address, every status and
        // every kind of line, at every width up to past the longest
        // line: no line is ever a cell short or a cell over, the gutter
        // is always first, and no segment is empty. This counts in the
        // core's own measure; shell.rs paints the same lines through
        // ratatui and checks that its measure agrees.
        let mut rows = sample_rows();
        rows.push(new_row("home/日本語のキャンペーン"));
        rows.push(new_row("home/👩\u{200D}🔬lab"));
        rows.push(dormant("home/❤\u{FE0F}garden", "2026-09-01"));
        rows.push(live("home/🇬🇧e\u{301}cole", "done"));
        rows.push(live(
            "an-estate-with-a-very-long-name/and-an-equally-long-campaign-name",
            "blocked",
        ));
        let gutter = Seg {
            text: " ".to_string(),
            tone: Tone::Fill,
        };
        for ind in [Indicators::Dots, Indicators::Symbols] {
            for query in ["", "h", "日本", "zzz", "❤\u{FE0F}", "👩\u{200D}🔬l"] {
                let state = typed(AppState::new(rows.clone()), query);
                for width in 0..=90 {
                    for height in [1, 2, 3, 5, 7, 12] {
                        let v = view(&state, width, height, ind);
                        assert_eq!(v.lines.len(), height);
                        for line in &v.lines {
                            assert_eq!(
                                line_cells(line),
                                width,
                                "{width}x{height} {query:?}: {:?}",
                                line.plain()
                            );
                            if width > 0 {
                                assert_eq!(line.segs[0], gutter);
                            }
                            assert!(line.segs.iter().all(|seg| !seg.text.is_empty()));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_wide_address_is_cut_between_characters_and_padded_to_the_meta() {
        // Room for 11 cells beside "new" at width 20: "home/日本" is 9,
        // the next character is two cells and would leave no room for
        // the "…", so the cut lands at 10 cells and one blank cell pads
        // out the room before the gap and the meta.
        let state = AppState::new(vec![new_row("home/日本語のキャンペーン")]);
        let v = view(&state, 20, 3, Indicators::Dots);
        assert_eq!(v.lines[2].plain(), ">   home/日本…  new");
        assert_eq!(line_cells(&v.lines[2]), 20);
    }

    #[test]
    fn text_splits_into_the_clusters_a_terminal_draws_as_one() {
        let table: &[(&str, &[&str])] = &[
            ("", &[]),
            // An accent as a macOS file name spells it (NFD) stays on
            // its letter.
            ("e\u{301}cole", &["e\u{301}", "c", "o", "l", "e"]),
            // An emoji keeps its variation selector and its keycap.
            ("❤\u{FE0F}x", &["❤\u{FE0F}", "x"]),
            ("1\u{FE0F}\u{20E3}", &["1\u{FE0F}\u{20E3}"]),
            // A joiner makes two emoji one, skin tones and a
            // text-default base included...
            ("👩\u{200D}🔬", &["👩\u{200D}🔬"]),
            ("⛹🏽\u{200D}♀\u{FE0F}", &["⛹🏽\u{200D}♀\u{FE0F}"]),
            // ...but never joins a letter, CJK, a lone skin tone or a
            // flag letter, on either side.
            ("a\u{200D}👩", &["a\u{200D}", "👩"]),
            ("👩\u{200D}a", &["👩\u{200D}", "a"]),
            ("日\u{200D}👩", &["日\u{200D}", "👩"]),
            ("🏽\u{200D}👩", &["🏽\u{200D}", "👩"]),
            ("🇬\u{200D}👩", &["🇬\u{200D}", "👩"]),
            // A skin tone joins the emoji before it.
            ("👍🏽", &["👍🏽"]),
            // Regional indicators pair into flags, two at a time, and
            // only with each other.
            ("🇬🇧🇯🇵🇺", &["🇬🇧", "🇯🇵", "🇺"]),
            ("a🇬", &["a", "🇬"]),
            // A control char stands alone, even before a skin tone or
            // a mark that would otherwise join it.
            ("\t🏽a\t\u{301}", &["\t", "🏽", "a", "\t", "\u{301}"]),
            // A Khmer coeng joins two consonants, and nothing else.
            ("ក\u{17D2}ក", &["ក\u{17D2}ក"]),
            ("\u{17D2}ក", &["\u{17D2}", "ក"]),
            ("ក\u{17D2}a", &["ក\u{17D2}", "a"]),
            ("a\u{17D2}ក", &["a\u{17D2}", "ក"]),
            // Kirat Rai vowel signs join each other, and nothing else.
            ("\u{16D63}\u{16D67}a", &["\u{16D63}\u{16D67}", "a"]),
            ("a\u{16D67}", &["a", "\u{16D67}"]),
            // CJK is one char a cluster; Hangul jamo make one syllable.
            ("日本", &["日", "本"]),
            ("\u{1100}\u{1161}\u{11A8}", &["\u{1100}\u{1161}\u{11A8}"]),
        ];
        for (text, want) in table {
            assert_eq!(clusters(text), *want, "{text:?}");
        }
    }

    #[test]
    fn a_cluster_takes_the_cells_ratatui_paints_it_in() {
        // Each expected width is what ratatui 0.30 paints the text in
        // (unicode-segmentation graphemes, measured by ratatui-core's
        // `CellWidth`), read off a scratch probe, not this module.
        let table = [
            ("", 0),
            ("home/", 5),
            ("e\u{301}", 1),
            ("❤", 1),
            ("❤\u{FE0F}", 2),
            ("1\u{FE0F}\u{20E3}", 2),
            ("👩\u{200D}🔬", 2),
            ("⛹🏽\u{200D}♀\u{FE0F}", 2),
            ("a\u{200D}👩", 3),
            ("🏽\u{200D}👩", 4),
            ("🇬", 1),
            ("🇬🇧", 2),
            ("👍🏽", 2),
            ("日本", 4),
            // A halfwidth sound mark takes a cell of its own.
            ("ｶ\u{FF9E}", 2),
            ("ក\u{17D2}ក", 1),
            ("\u{16D63}\u{16D67}", 1),
            // ratatui drops a control char; unicode-width would give it
            // a cell.
            ("\t", 0),
            ("\t🏽", 2),
            ("home/❤\u{FE0F}garden", 13),
            ("home/👩\u{200D}🔬lab", 10),
        ];
        for (text, want) in table {
            assert_eq!(cells(text), want, "{text:?}");
        }
    }

    #[test]
    fn a_cut_never_falls_inside_a_cluster() {
        // A per-char cut would keep "❤" without its selector here,
        // one cell of a two-cell emoji.
        let heart = "home/❤\u{FE0F}garden";
        assert_eq!(clip(heart, 6), "home/");
        assert_eq!(clip(heart, 7), "home/❤\u{FE0F}");
        assert_eq!(ellipsize(heart, 7), "home/…");
        assert_eq!(ellipsize(heart, 8), "home/❤\u{FE0F}…");
        // And "👩" without the rest of its sequence here.
        let scientist = "home/👩\u{200D}🔬lab";
        assert_eq!(clip(scientist, 6), "home/");
        assert_eq!(clip(scientist, 7), "home/👩\u{200D}🔬");
        // The query's visible tail starts between clusters too: a
        // per-char scroll would start it at the selector or the joiner.
        assert_eq!(tail_before_cursor("x❤\u{FE0F}ab", 4), "ab");
        assert_eq!(tail_before_cursor("x❤\u{FE0F}ab", 5), "❤\u{FE0F}ab");
        assert_eq!(tail_before_cursor("x👩\u{200D}🔬", 3), "👩\u{200D}🔬");
        assert_eq!(tail_before_cursor("x👩\u{200D}🔬", 2), "");
    }

    #[test]
    fn the_query_column_lines_up_with_the_address_column() {
        let state = typed(AppState::new(sample_rows()), "gar");
        let v = view(&state, 40, 10, Indicators::Symbols);
        assert_eq!(column_of(&v.lines[0], "/"), Some(2));
        assert_eq!(column_of(&v.lines[0], "gar"), Some(4));
        assert_eq!(tone_of(&v.lines[0], "gar"), Some(Tone::Text));
        assert_eq!(column_of(&v.lines[2], "◐"), Some(2));
        assert_eq!(column_of(&v.lines[2], "home/garden"), Some(4));
        assert_eq!(v.cursor, Some((7, 0)));
    }

    #[test]
    fn a_long_query_stops_before_the_count() {
        // "0/6 campaigns" is 13 cells and ends one cell short of the
        // edge, at column 38, so it starts at 26. The editor is columns
        // 4..=24, one blank cell short of the count: the last 20 cells
        // of the query show, the cursor goes after them at 24, and 25
        // stays blank.
        let query = format!("{}wxyz", "q".repeat(30));
        let state = typed(AppState::new(sample_rows()), &query);
        let v = view(&state, 40, 10, Indicators::Dots);
        assert_eq!(
            v.lines[0].plain(),
            format!("  / {}wxyz  0/6 campaigns", "q".repeat(16))
        );
        assert_eq!(v.cursor, Some((24, 0)));
        assert_eq!(column_of(&v.lines[0], "0/6"), Some(26));
    }

    #[test]
    fn the_count_shows_only_while_it_leaves_the_editor_a_cell() {
        let state = AppState::new(sample_rows());
        // "6 campaigns" is 11 cells: at width 18 the editor keeps one
        // cell, enough for the cursor, then a blank cell, then the count.
        assert_eq!(
            view(&state, 18, 3, Indicators::Dots).lines[0].plain(),
            "  / f 6 campaigns"
        );
        // One narrower and the count goes; the placeholder runs to the
        // trailing cell instead.
        assert_eq!(
            view(&state, 17, 3, Indicators::Dots).lines[0].plain(),
            "  / filter campa"
        );
        assert_eq!(view(&state, 17, 3, Indicators::Dots).cursor, Some((4, 0)));
    }

    #[test]
    fn the_count_says_n_of_m_while_a_filter_narrows_and_one_campaign_is_singular() {
        let first = |state: &AppState| view(state, 40, 3, Indicators::Dots).lines[0].plain();
        let all = AppState::new(sample_rows());
        assert!(first(&all).ends_with(" 6 campaigns"), "{}", first(&all));
        let narrowed = typed(all, "bench");
        assert!(first(&narrowed).ends_with(" 1/6 campaigns"));
        let one = AppState::new(vec![new_row("home/attic")]);
        assert!(first(&one).ends_with(" 1 campaign"));
        // An N/M count stays plural, even out of one campaign.
        let one_hidden = typed(one, "zzz");
        assert!(
            first(&one_hidden).ends_with(" 0/1 campaigns"),
            "{}",
            first(&one_hidden)
        );
        let none = AppState::new(Vec::new());
        assert!(first(&none).ends_with(" 0 campaigns"));
    }

    #[test]
    fn the_footer_drops_whole_hints_from_the_end() {
        let state = AppState::new(sample_rows());
        let footer = |width| view(&state, width, 5, Indicators::Dots).lines[4].plain();
        // All four hints are 55 cells: they fit exactly at width 58,
        // after the gutter and the leading cell and before the trailing
        // cell.
        assert_eq!(
            footer(58),
            "  filter type · move ↑↓/ctrl+n/p · open enter · close esc"
        );
        assert_eq!(footer(57), "  filter type · move ↑↓/ctrl+n/p · open enter");
        assert_eq!(footer(14), "  filter type");
        assert_eq!(footer(13), "");
    }

    /// What a line of the frame is, read off its text and tones.
    #[derive(Debug, PartialEq)]
    enum Kind {
        Filter,
        Rule,
        Row,
        Blank,
        Detail,
        Footer,
    }

    fn kind(line: &ViewLine) -> Kind {
        let text = line.plain();
        if text.starts_with("  / ") {
            Kind::Filter
        } else if line.segs.get(1).is_some_and(|seg| seg.tone == Tone::Rule) {
            Kind::Rule
        } else if text.starts_with("  filter type") {
            Kind::Footer
        } else if text.contains(" · ") {
            Kind::Detail
        } else if text.is_empty() {
            Kind::Blank
        } else {
            Kind::Row
        }
    }

    #[test]
    fn each_height_tier_keeps_the_lines_it_has_room_for() {
        use Kind::{Blank, Detail, Filter, Footer, Row, Rule};
        let state = AppState::new(vec![
            live("home/garden", "idle"),
            dormant("home/kitchen", "2026-09-10"),
            new_row("home/attic"),
        ]);
        let expected: [&[Kind]; 9] = [
            &[],
            &[Filter],
            &[Filter, Row],
            &[Filter, Rule, Row],
            &[Filter, Rule, Row, Row],
            &[Filter, Rule, Row, Row, Footer],
            &[Filter, Rule, Row, Row, Row, Footer],
            &[Filter, Rule, Row, Row, Row, Detail, Footer],
            &[Filter, Rule, Row, Row, Row, Blank, Detail, Footer],
        ];
        for (height, want) in expected.iter().enumerate() {
            let got: Vec<Kind> = view(&state, 60, height, Indicators::Dots)
                .lines
                .iter()
                .map(kind)
                .collect();
            assert_eq!(&got, want, "height {height}");
        }
    }

    #[test]
    fn blocked_is_a_cross_under_symbols_and_a_dot_under_dots() {
        assert_eq!(Status::from_wire("blocked"), Status::Blocked);
        assert_eq!(Status::Blocked.glyph(Indicators::Symbols), "×");
        assert_eq!(Status::Blocked.glyph(Indicators::Dots), "●");
        let state = AppState::new(vec![live("home/garden", "blocked")]);
        let row = |ind| view(&state, 40, 3, ind).lines[2].plain();
        assert!(row(Indicators::Symbols).starts_with("> × home/garden"));
        assert!(row(Indicators::Dots).starts_with("> ● home/garden"));
        assert!(row(Indicators::Dots).ends_with(" blocked"));
    }

    #[test]
    fn every_status_has_herdrs_glyphs_and_word_in_one_cell() {
        let table = [
            ("working", Status::Working, "●", "◐"),
            ("blocked", Status::Blocked, "●", "×"),
            ("done", Status::Done, "●", "✓"),
            ("idle", Status::Idle, "○", "○"),
            ("unknown", Status::Unknown, "·", "·"),
        ];
        for (wire, status, dot, symbol) in table {
            assert_eq!(Status::from_wire(wire), status, "{wire}");
            assert_eq!(status.word(), wire);
            assert_eq!(status.glyph(Indicators::Dots), dot, "{wire}");
            assert_eq!(status.glyph(Indicators::Symbols), symbol, "{wire}");
            assert_eq!(cells(dot), 1, "{dot}");
            assert_eq!(cells(symbol), 1, "{symbol}");
        }
        for wire in ["", "Working", "asleep"] {
            assert_eq!(Status::from_wire(wire), Status::Unknown, "{wire:?}");
        }
        // A live row whose status this port doesn't know reads as
        // unknown, glyph and word alike.
        let state = AppState::new(vec![live("home/garden", "asleep")]);
        let v = view(&state, 40, 3, Indicators::Symbols);
        assert_eq!(
            tone_of(&v.lines[2], "·"),
            Some(Tone::Status(Status::Unknown))
        );
        assert!(v.lines[2].plain().ends_with(" unknown"));
    }

    #[test]
    fn the_detail_line_says_what_enter_does_with_the_selected_row() {
        let mut state = AppState::new(vec![
            live("home/garden", "idle"),
            dormant("home/kitchen", "2026-09-10"),
            new_row("home/attic"),
        ]);
        let detail = |state: &AppState| view(state, 60, 7, Indicators::Dots).lines[5].plain();
        assert_eq!(detail(&state), "  home/garden · focus w-home/garden");
        state = update(state, Event::Down);
        assert_eq!(detail(&state), "  home/kitchen · resume from 2026-09-10");
        state = update(state, Event::Down);
        assert_eq!(detail(&state), "  home/attic · start fresh");
        // Nothing matches, so nothing is selected: a blank line.
        state = typed(state, "zzz");
        assert_eq!(detail(&state), "");
        // Hard-clipped to the text columns, 17 cells at width 20, as
        // herdr clips its own detail line: no "…" on the action.
        let narrow = AppState::new(vec![new_row("home/attic")]);
        assert_eq!(
            view(&narrow, 20, 7, Indicators::Dots).lines[5].plain(),
            "  home/attic · star"
        );
    }

    #[test]
    fn the_address_ends_in_an_ellipsis_and_gives_up_the_meta_below_eight_cells() {
        let state = AppState::new(vec![new_row(
            "an-estate-with-a-very-long-name/and-an-equally-long-campaign-name",
        )]);
        let row = |width| view(&state, width, 3, Indicators::Dots).lines[2].plain();
        // Width 30 leaves 21 cells beside "new": 20 of the address, "…",
        // a blank cell, the meta and the trailing cell.
        assert_eq!(row(30), ">   an-estate-with-a-ver… new");
        // Width 17 leaves exactly eight, and the meta stays.
        assert_eq!(row(17), ">   an-esta… new");
        // One narrower would leave seven: the meta goes and the address
        // takes its room.
        assert_eq!(row(16), ">   an-estate-…");
        // An address exactly as wide as its room is shown whole.
        let fits = AppState::new(vec![new_row("home/abc")]);
        assert_eq!(
            view(&fits, 17, 3, Indicators::Dots).lines[2].plain(),
            ">   home/abc new"
        );
    }

    #[test]
    fn the_cursor_sits_after_the_query_and_needs_column_four() {
        let state = AppState::new(sample_rows());
        assert_eq!(view(&state, 4, 5, Indicators::Dots).cursor, None);
        assert_eq!(view(&state, 5, 5, Indicators::Dots).cursor, Some((4, 0)));
        assert_eq!(view(&state, 40, 0, Indicators::Dots).cursor, None);
        let state = typed(state, "ho");
        assert_eq!(view(&state, 40, 5, Indicators::Dots).cursor, Some((6, 0)));
        // No room for any of the query: the cursor holds column 4.
        assert_eq!(view(&state, 5, 5, Indicators::Dots).cursor, Some((4, 0)));
    }
}
