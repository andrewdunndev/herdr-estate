pub use crate::herdrcli::AgentStatus;
use std::collections::BTreeMap;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    Live {
        workspace_id: String,
        agent_status: Option<AgentStatus>,
    },
    Dormant {
        last: String,
    },
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub address: String,
    pub state: RowState,
    pub brief: String,
}

fn state_rank(state: &RowState) -> u8 {
    match state {
        RowState::Live { .. } => 0,
        RowState::Dormant { .. } => 1,
        RowState::New => 2,
    }
}

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
    Fresh(Row),
    FreshRunning(Row),
    Create { address: String, brief: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Selection {
    Row(String),
    Create,
    Sort,
}

pub const SORT_LABEL: &str = "Sort spaces";

const SORT_HINT: &str = "order herdr's workspaces by label, ~ first · enter";

#[derive(Debug, Clone)]
pub struct AppState {
    rows: Vec<Row>,
    pub filter: String,
    selected: Option<Selection>,
    estates: BTreeMap<String, String>,
    brief: Option<String>,
    brief_wanted: bool,
    armed: Option<String>,
    sort_note: Option<String>,
    pub sort_requested: bool,
    pub done: Option<Outcome>,
}

#[derive(Debug, Clone, Copy)]
struct Extras {
    creates: bool,
    sorts: bool,
}

impl Extras {
    fn items(self, rows: usize) -> usize {
        rows + usize::from(self.creates) + usize::from(self.sorts)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Event {
    Char(char),
    Backspace,
    Up,
    Down,
    Enter,
    Fresh,
    Esc,
    Quit,
    Other,
}

impl AppState {
    pub fn new(rows: Vec<Row>) -> Self {
        let rows = sort_rows(rows);
        let selected = rows.first().map(|r| Selection::Row(r.address.clone()));
        AppState {
            rows,
            filter: String::new(),
            selected,
            estates: BTreeMap::new(),
            brief: None,
            brief_wanted: false,
            armed: None,
            sort_note: None,
            sort_requested: false,
            done: None,
        }
    }

    pub fn with_estates(self, estates: BTreeMap<String, String>) -> Self {
        AppState { estates, ..self }
    }

    pub fn sorted(self, note: String) -> Self {
        AppState {
            sort_note: Some(note),
            sort_requested: false,
            ..self
        }
    }

    pub fn filtered(&self) -> Vec<&Row> {
        self.rows
            .iter()
            .filter(|r| r.address.contains(&self.filter))
            .collect()
    }

    fn create_refusal(&self) -> Option<String> {
        let (estate, name) = self.filter.split_once('/')?;
        if !self.estates.contains_key(estate) {
            return Some(format!("no estate '{estate}'"));
        }
        let known: Vec<&str> = self.rows.iter().map(|r| r.address.as_str()).collect();
        name::refusal(estate, name, &known)
    }

    fn creates(&self) -> bool {
        self.filter.contains('/') && self.create_refusal().is_none()
    }

    fn sorts(&self) -> bool {
        SORT_LABEL
            .to_lowercase()
            .contains(&self.filter.to_lowercase())
    }

    fn extras(&self) -> Extras {
        Extras {
            creates: self.creates(),
            sorts: self.sorts(),
        }
    }
}

fn current_index(filtered: &[&Row], extras: Extras, selected: &Option<Selection>) -> Option<usize> {
    match selected.as_ref()? {
        Selection::Row(addr) => filtered.iter().position(|r| &r.address == addr),
        Selection::Create => extras.creates.then_some(filtered.len()),
        Selection::Sort => extras
            .sorts
            .then_some(filtered.len() + usize::from(extras.creates)),
    }
}

fn selection_at(filtered: &[&Row], extras: Extras, i: usize) -> Selection {
    match filtered.get(i) {
        Some(r) => Selection::Row(r.address.clone()),
        None if extras.creates && i == filtered.len() => Selection::Create,
        None => Selection::Sort,
    }
}

fn running(row: &Row) -> bool {
    matches!(&row.state, RowState::Live { agent_status, .. }
        if agent_status.as_ref() != Some(&AgentStatus::Unknown))
}

fn arm_eligible(row: &Row) -> bool {
    matches!(&row.state, RowState::Live { agent_status, .. }
        if matches!(Status::of(agent_status.as_ref()), Status::Idle | Status::Done))
}

fn recompute_selection(state: &mut AppState) {
    let filtered = state.filtered();
    let extras = state.extras();
    let items = extras.items(filtered.len());
    if current_index(&filtered, extras, &state.selected).is_none_or(|i| i >= items) {
        state.selected = (items > 0).then(|| selection_at(&filtered, extras, 0));
    }
}

pub fn update(mut state: AppState, event: Event) -> AppState {
    if let Some(brief) = &mut state.brief {
        match event {
            Event::Char(c) if !c.is_control() && brief.chars().count() < name::BRIEF_MAX => {
                brief.push(c);
            }
            Event::Backspace => {
                brief.pop();
            }
            Event::Enter if brief.trim().is_empty() => state.brief_wanted = true,
            Event::Enter => {
                state.done = Some(Outcome::Create {
                    address: state.filter.clone(),
                    brief: brief.trim().to_string(),
                });
            }
            Event::Esc => {
                state.brief = None;
                state.brief_wanted = false;
            }
            Event::Quit => state.done = Some(Outcome::Cancelled),
            _ => {}
        }
        return state;
    }
    if !matches!(event, Event::Fresh) {
        state.armed = None;
    }
    match event {
        Event::Char(c) => {
            state.filter.push(c);
            recompute_selection(&mut state);
        }
        Event::Backspace => {
            state.filter.pop();
            recompute_selection(&mut state);
        }
        Event::Up | Event::Down => {
            let filtered = state.filtered();
            let extras = state.extras();
            let items = extras.items(filtered.len());
            let to = current_index(&filtered, extras, &state.selected).and_then(|i| match event {
                Event::Up => i.checked_sub(1),
                _ => Some(i + 1).filter(|&j| j < items),
            });
            if let Some(j) = to {
                state.selected = Some(selection_at(&filtered, extras, j));
            }
        }
        Event::Enter | Event::Fresh => {
            let filtered = state.filtered();
            let extras = state.extras();
            let current = current_index(&filtered, extras, &state.selected)
                .map(|i| (filtered.get(i), selection_at(&filtered, extras, i)));
            match current {
                Some((None, Selection::Sort)) => {
                    if matches!(event, Event::Enter) {
                        state.sort_requested = true;
                    }
                }
                Some((None, _)) => state.brief = Some(String::new()),
                Some((Some(row), _)) if matches!(event, Event::Enter) => {
                    state.done = Some(Outcome::Selected((*row).clone()));
                }
                Some((Some(row), _)) if !running(row) => {
                    state.done = Some(Outcome::Fresh((*row).clone()));
                }
                Some((Some(row), _)) if arm_eligible(row) => {
                    if state.armed.as_deref() == Some(row.address.as_str()) {
                        state.done = Some(Outcome::FreshRunning((*row).clone()));
                        state.armed = None;
                    } else {
                        state.armed = Some(row.address.clone());
                    }
                }
                _ => {
                    state.armed = None;
                }
            }
        }
        Event::Esc | Event::Quit => {
            state.done = Some(Outcome::Cancelled);
        }
        Event::Other => {}
    }
    state
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indicators {
    Dots,
    Symbols,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Working,
    Blocked,
    Done,
    Idle,
    Unknown,
}

impl Status {
    pub fn of(agent_status: Option<&AgentStatus>) -> Status {
        match agent_status {
            Some(AgentStatus::Working) => Status::Working,
            Some(AgentStatus::Other(s)) if s == "blocked" => Status::Blocked,
            Some(AgentStatus::Done) => Status::Done,
            Some(AgentStatus::Idle) => Status::Idle,
            Some(AgentStatus::Unknown | AgentStatus::Other(_)) | None => Status::Unknown,
        }
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Fill,
    Text,
    Subtext,
    Muted,
    Rule,
    Status(Status),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewLine {
    pub segs: Vec<Seg>,
    pub selected: bool,
}

impl ViewLine {
    pub fn plain(&self) -> String {
        let mut text: String = self.segs.iter().map(|seg| seg.text.as_str()).collect();
        if self.selected && text.starts_with(' ') {
            text.replace_range(..1, ">");
        }
        text.trim_end().to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub lines: Vec<ViewLine>,
    pub cursor: Option<(u16, u16)>,
}

const TEXT_COL: usize = 4;

const PLACEHOLDER: &str = "filter campaigns";

const FOOTER: [&str; 5] = [
    "filter type",
    "move ↑↓",
    "open enter",
    "fresh ctrl+f",
    "close esc",
];

const BRIEF_FOOTER: [&str; 3] = ["create and start enter", "back esc", "quit ctrl+c"];

const MIN_ADDRESS_CELLS: usize = 8;

const MAX_ADDRESS_CELLS: usize = 32;

const BRIEF_GAP: usize = 2;

fn clusters(s: &str) -> Vec<&str> {
    s.graphemes(true).collect()
}

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

fn cells(s: &str) -> usize {
    clusters(s).into_iter().map(cluster_cells).sum()
}

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

fn ellipsize(s: &str, max: usize) -> String {
    if cells(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    format!("{}…", clip(s, max - 1))
}

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

fn room_beside(width: usize, right: &str) -> usize {
    width.saturating_sub(TEXT_COL + 2 + cells(right))
}

fn room_alone(width: usize) -> usize {
    width.saturating_sub(TEXT_COL + 1)
}

fn text_room(width: usize) -> usize {
    width.saturating_sub(3)
}

struct LineBuilder {
    width: usize,
    used: usize,
    segs: Vec<Seg>,
}

impl LineBuilder {
    fn new(width: usize) -> Self {
        let mut line = LineBuilder {
            width,
            used: 0,
            segs: Vec::new(),
        };
        line.push(" ", Tone::Fill);
        line
    }

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

    fn pad_to(&mut self, col: usize) {
        self.push(&" ".repeat(col.saturating_sub(self.used)), Tone::Fill);
    }

    fn push_right(&mut self, text: &str, tone: Tone) {
        self.pad_to(self.width.saturating_sub(1 + cells(text)));
        self.push(text, tone);
    }

    fn finish(mut self, selected: bool) -> ViewLine {
        self.pad_to(self.width);
        ViewLine {
            segs: self.segs,
            selected,
        }
    }
}

struct Tiers {
    rule: bool,
    footer: bool,
    detail: bool,
    list: usize,
}

impl Tiers {
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

fn scroll_offset(selected_idx: usize, total: usize, window: usize) -> usize {
    if window == 0 || total <= window {
        return 0;
    }
    let max_offset = total - window;
    selected_idx.saturating_sub(window - 1).min(max_offset)
}

pub fn view(state: &AppState, width: usize, height: usize, ind: Indicators) -> View {
    if height == 0 {
        return View {
            lines: Vec::new(),
            cursor: None,
        };
    }
    if let Some(brief) = &state.brief {
        return brief_view(state, brief, width, height);
    }
    let tiers = Tiers::for_height(height);
    let filtered = state.filtered();
    let extras = state.extras();
    let selected_idx = current_index(&filtered, extras, &state.selected);

    let (filter, cursor) = filter_line(&state.filter, filtered.len(), state.rows.len(), width);
    let mut lines = vec![filter];
    if tiers.rule {
        lines.push(rule_line(width));
    }
    lines.extend(list_lines(
        state,
        &filtered,
        extras,
        selected_idx,
        width,
        tiers.list,
        ind,
    ));
    if tiers.detail {
        let sort_selected =
            selected_idx.is_some_and(|i| selection_at(&filtered, extras, i) == Selection::Sort);
        lines.push(if sort_selected {
            note_line(state.sort_note.as_deref().unwrap_or(SORT_HINT), width)
        } else {
            detail_line(
                selected_idx.and_then(|i| filtered.get(i).copied()),
                state.armed.as_deref(),
                width,
            )
        });
    }
    if tiers.footer {
        lines.push(footer_line(&FOOTER, width));
    }
    View { lines, cursor }
}

fn brief_view(state: &AppState, brief: &str, width: usize, height: usize) -> View {
    let prompt = format!("brief for {}> ", state.filter);
    let mut input = LineBuilder::new(width);
    input.push(" ", Tone::Fill);
    input.push(&prompt, Tone::Muted);
    let start = 2 + cells(&prompt);
    let visible = tail_before_cursor(brief, width.saturating_sub(start + 1));
    input.push(visible, Tone::Text);
    let cursor =
        (start + 1 < width).then(|| (u16::try_from(start + cells(visible)).unwrap_or(u16::MAX), 0));
    let (estate, name) = state.filter.split_once('/').unwrap_or_default();
    let dir = state.estates.get(estate).map_or("", String::as_str);
    let place = format!("in {dir}/{name} (untracked; not committed)");
    let hint = if state.brief_wanted && brief.trim().is_empty() {
        note_line("type a one-line brief first", width)
    } else {
        footer_line(&BRIEF_FOOTER, width)
    };
    let mut lines = vec![input.finish(false), note_line(&place, width), hint];
    lines.resize_with(height, || LineBuilder::new(width).finish(false));
    View { lines, cursor }
}

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

fn rule_line(width: usize) -> ViewLine {
    let mut line = LineBuilder::new(width);
    line.push(&"─".repeat(width.saturating_sub(1)), Tone::Rule);
    line.finish(false)
}

fn list_lines(
    state: &AppState,
    filtered: &[&Row],
    extras: Extras,
    selected_idx: Option<usize>,
    width: usize,
    height: usize,
    ind: Indicators,
) -> Vec<ViewLine> {
    let items = extras.items(filtered.len());
    let mut lines: Vec<ViewLine> = Vec::new();
    if filtered.is_empty() && !extras.creates {
        let message = match state.create_refusal() {
            Some(reason) => reason,
            None if state.rows.is_empty() => "no campaigns".to_string(),
            None => "no matching campaigns".to_string(),
        };
        lines.push(note_line(&message, width));
    }
    let window = height.saturating_sub(lines.len());
    let offset = scroll_offset(selected_idx.unwrap_or(0), items, window);
    let column = filtered
        .iter()
        .map(|row| cells(&row.address))
        .max()
        .unwrap_or(0)
        .min(MAX_ADDRESS_CELLS);
    lines.extend((offset..items).take(window).map(|i| {
        match (filtered.get(i), selection_at(filtered, extras, i)) {
            (Some(row), _) => row_line(row, column, width, ind, selected_idx == Some(i)),
            (None, Selection::Sort) => sort_line(width, selected_idx == Some(i)),
            (None, _) => create_line(&state.filter, width, selected_idx == Some(i)),
        }
    }));
    lines.truncate(height);
    lines.resize_with(height, || LineBuilder::new(width).finish(false));
    lines
}

fn row_line(row: &Row, column: usize, width: usize, ind: Indicators, selected: bool) -> ViewLine {
    let (glyph, glyph_tone, address_tone, meta) = match &row.state {
        RowState::Live { agent_status, .. } => {
            let status = Status::of(agent_status.as_ref());
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
    let column = column.min(room);
    let mut line = LineBuilder::new(width);
    line.push(" ", Tone::Fill);
    line.push(glyph, glyph_tone);
    line.push(" ", Tone::Fill);
    line.push(&ellipsize(&row.address, column), address_tone);
    if let Some(meta) = meta {
        let brief_room = room.saturating_sub(column + BRIEF_GAP);
        if brief_room > 1 {
            line.pad_to(TEXT_COL + column + BRIEF_GAP);
            line.push(&ellipsize(&row.brief, brief_room), Tone::Subtext);
        }
        line.push_right(&meta, Tone::Muted);
    }
    line.finish(selected)
}

fn create_line(address: &str, width: usize, selected: bool) -> ViewLine {
    let mut line = LineBuilder::new(width);
    line.push(" ", Tone::Fill);
    line.push(&format!("+ create {address}"), Tone::Text);
    line.finish(selected)
}

fn sort_line(width: usize, selected: bool) -> ViewLine {
    let mut line = LineBuilder::new(width);
    line.pad_to(TEXT_COL);
    line.push(SORT_LABEL, Tone::Text);
    line.finish(selected)
}

fn detail_line(row: Option<&Row>, armed: Option<&str>, width: usize) -> ViewLine {
    let Some(row) = row else {
        return LineBuilder::new(width).finish(false);
    };
    let action = match &row.state {
        RowState::Live { workspace_id, .. } => format!("focus {workspace_id}"),
        RowState::Dormant { last } => format!("resume from {last}"),
        RowState::New => "start fresh".to_string(),
    };
    let fresh = fresh_tail(row, armed == Some(row.address.as_str()));
    note_line(&format!("{} · {action} · {fresh}", row.address), width)
}

fn fresh_tail(row: &Row, armed: bool) -> &'static str {
    if !running(row) {
        return "fresh ctrl+f";
    }
    let RowState::Live { agent_status, .. } = &row.state else {
        return "fresh ctrl+f";
    };
    match Status::of(agent_status.as_ref()) {
        Status::Idle | Status::Done if armed => "ctrl+f again: stop agent, start fresh",
        Status::Idle | Status::Done => "fresh ctrl+f",
        Status::Working => "agent working, wait or exit",
        Status::Blocked => "agent blocked, wait or exit",
        Status::Unknown => "agent busy, wait or exit",
    }
}

fn footer_line(hints: &[&str], width: usize) -> ViewLine {
    let room = text_room(width);
    let mut text = String::new();
    for hint in hints {
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
            brief: String::new(),
        }
    }

    fn with_brief(row: Row, brief: &str) -> Row {
        Row {
            brief: brief.to_string(),
            ..row
        }
    }

    fn wire_status(wire: &str) -> Status {
        Status::of(Some(&AgentStatus::from_wire(wire)))
    }

    fn live(address: &str, status: &str) -> Row {
        row(
            address,
            RowState::Live {
                workspace_id: format!("w-{address}"),
                agent_status: Some(AgentStatus::from_wire(status)),
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
                "home/garden",
                "lab/bench",
                "home/taxes",
                "home/kitchen",
                "home/attic",
                "home/cellar",
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
        let state = AppState::new(sample_rows());
        let state = update(state, Event::Up);
        assert_eq!(selected(&state), Some("home/garden"));
    }

    #[test]
    fn movement_clamps_at_sort_spaces_below_the_last_campaign() {
        let mut state = AppState::new(sample_rows());
        for _ in 0..10 {
            state = update(state, Event::Down);
        }
        assert_eq!(state.selected, Some(Selection::Sort));
        let state = update(state, Event::Up);
        assert_eq!(selected(&state), Some("home/cellar"));
    }

    #[test]
    fn sort_spaces_shows_while_the_filter_is_part_of_its_label_in_any_case() {
        let state = AppState::new(sample_rows());
        assert!(state.sorts());
        let state = "SpA".chars().fold(state, |s, c| update(s, Event::Char(c)));
        assert!(state.sorts());
        assert_eq!(state.selected, Some(Selection::Sort));
        let state = update(state, Event::Char('x'));
        assert!(!state.sorts());
        assert_eq!(state.selected, None);
    }

    #[test]
    fn enter_on_sort_spaces_asks_for_a_sort_and_leaves_the_picker_open() {
        let state = "sort".chars().fold(AppState::new(sample_rows()), |s, c| {
            update(s, Event::Char(c))
        });
        let state = update(state, Event::Enter);
        assert!(state.sort_requested);
        assert_eq!(state.done, None);
        let state = state.sorted("moved 2 workspaces into label order".to_string());
        assert!(!state.sort_requested);
        let v = view(&state, 60, 8, Indicators::Dots);
        assert!(v
            .lines
            .iter()
            .any(|l| l.selected && l.plain().contains(SORT_LABEL)));
        assert!(v
            .lines
            .iter()
            .any(|l| l.plain().trim() == "moved 2 workspaces into label order"));
    }

    #[test]
    fn ctrl_f_on_sort_spaces_does_nothing() {
        let state = "sort".chars().fold(AppState::new(sample_rows()), |s, c| {
            update(s, Event::Char(c))
        });
        let state = update(state, Event::Fresh);
        assert!(!state.sort_requested);
        assert_eq!(state.done, None);
        assert_eq!(state.brief, None);
    }

    #[test]
    fn a_filter_naming_no_campaign_still_says_so_above_sort_spaces() {
        let state = "spaces".chars().fold(AppState::new(sample_rows()), |s, c| {
            update(s, Event::Char(c))
        });
        let v = view(&state, 60, 8, Indicators::Dots);
        assert_eq!(v.lines[2].plain(), "  no matching campaigns");
        assert_eq!(v.lines[3].plain(), ">   Sort spaces");
        assert!(v.lines[3].selected);
    }

    #[test]
    fn up_from_below_the_top_reselects_the_previous_row() {
        let mut state = AppState::new(sample_rows());
        state = update(state, Event::Down);
        state = update(state, Event::Down);
        state = update(state, Event::Up);
        assert_eq!(selected(&state), Some("lab/bench"));
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
        state = update(state, Event::Down);
        state = update(state, Event::Enter);
        assert_eq!(
            state.done,
            Some(Outcome::Selected(live("lab/bench", "idle")))
        );
    }

    #[test]
    fn ctrl_f_freshens_the_selected_row_unless_its_agent_is_running() {
        for row in [
            new_row("home/attic"),
            dormant("home/kitchen", "2026-09-10"),
            live("home/garden", "unknown"),
        ] {
            let state = update(AppState::new(vec![row.clone()]), Event::Fresh);
            assert_eq!(state.done, Some(Outcome::Fresh(row)));
        }
        let running = AppState::new(vec![live("home/garden", "working")]);
        assert_eq!(update(running, Event::Fresh).done, None);
        let none = typed(AppState::new(sample_rows()), "zzz");
        assert_eq!(update(none, Event::Fresh).done, None);
    }

    #[test]
    fn a_live_row_with_no_status_is_running_and_never_arms() {
        let row = Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w9".to_string(),
                agent_status: None,
            },
            brief: String::new(),
        };
        assert!(running(&row));
        assert!(!arm_eligible(&row));
        let state = update(AppState::new(vec![row.clone()]), Event::Fresh);
        assert_eq!(state.done, None);
        assert_eq!(state.armed, None);
        assert_eq!(fresh_tail(&row, false), "agent busy, wait or exit");
    }

    #[test]
    fn ctrl_f_arms_idle_or_done_and_fires_on_the_second_press() {
        for status in ["idle", "done"] {
            let state = AppState::new(vec![live("home/garden", status)]);
            let state = update(state, Event::Fresh);
            assert_eq!(state.done, None);
            let state = update(state, Event::Fresh);
            assert_eq!(
                state.done,
                Some(Outcome::FreshRunning(live("home/garden", status)))
            );
        }
    }

    #[test]
    fn ctrl_f_refuses_working_blocked_and_any_other_running_status() {
        for status in ["working", "blocked", "starting"] {
            let state = AppState::new(vec![live("home/garden", status)]);
            let state = update(state, Event::Fresh);
            assert_eq!(state.done, None, "status {status}");
            let state = update(state, Event::Fresh);
            assert_eq!(state.done, None, "status {status}");
        }
    }

    #[test]
    fn ctrl_f_arm_disarms_on_a_selection_move_a_filter_change_or_another_key() {
        let rows = vec![live("home/garden", "idle"), live("home/kitchen", "idle")];
        let armed = |mut state: AppState| {
            state = update(state, Event::Fresh);
            assert_eq!(state.done, None);
            state
        };

        let state = armed(AppState::new(rows.clone()));
        let state = update(state, Event::Down);
        let state = update(state, Event::Fresh);
        assert_eq!(state.done, None);
        let state = update(state, Event::Fresh);
        assert_eq!(
            state.done,
            Some(Outcome::FreshRunning(live("home/kitchen", "idle")))
        );

        let state = armed(AppState::new(rows.clone()));
        let state = update(state, Event::Down);
        let state = update(state, Event::Up);
        let state = update(state, Event::Fresh);
        assert_eq!(state.done, None);
        let state = update(state, Event::Fresh);
        assert_eq!(
            state.done,
            Some(Outcome::FreshRunning(live("home/garden", "idle")))
        );

        let state = armed(AppState::new(rows.clone()));
        let state = update(state, Event::Char('x'));
        let state = update(state, Event::Backspace);
        let state = update(state, Event::Fresh);
        assert_eq!(state.done, None);

        let state = armed(AppState::new(rows.clone()));
        let state = update(state, Event::Enter);
        assert_eq!(state.armed, None);

        for other in [Event::Esc, Event::Quit, Event::Other] {
            let state = armed(AppState::new(rows.clone()));
            let state = update(state, other);
            assert_eq!(state.armed, None);
        }
    }

    #[test]
    fn ctrl_f_second_press_re_checks_status_and_refuses_a_flip_to_ineligible() {
        let mut state = AppState::new(vec![live("home/garden", "idle")]);
        state = update(state, Event::Fresh);
        assert_eq!(state.armed.as_deref(), Some("home/garden"));
        state.rows[0] = live("home/garden", "working");
        state = update(state, Event::Fresh);
        assert_eq!(state.done, None);
        assert_eq!(state.armed, None);
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
        state = update(state, Event::Down);
        assert_eq!(selected(&state), Some("lab/bench"));
        state = update(state, Event::Char('b'));
        assert_eq!(selected(&state), Some("lab/bench"));
        state = update(state, Event::Backspace);
        assert_eq!(selected(&state), Some("lab/bench"));
    }

    #[test]
    fn selection_falls_back_to_first_match_when_it_no_longer_matches() {
        let mut state = AppState::new(sample_rows());
        state = update(state, Event::Down);
        for c in ['h', 'o', 'm', 'e'] {
            state = update(state, Event::Char(c));
        }
        assert_eq!(selected(&state), Some("home/garden"));
    }

    fn typed(mut state: AppState, text: &str) -> AppState {
        for c in text.chars() {
            state = update(state, Event::Char(c));
        }
        state
    }

    fn selected(state: &AppState) -> Option<&str> {
        match &state.selected {
            Some(Selection::Row(address)) => Some(address),
            _ => None,
        }
    }

    fn creating(rows: Vec<Row>) -> AppState {
        AppState::new(rows).with_estates(BTreeMap::from([(
            "home".to_string(),
            "/e/home/campaigns".to_string(),
        )]))
    }

    fn briefing(brief: &str) -> AppState {
        typed(
            update(typed(creating(sample_rows()), "home/quern"), Event::Enter),
            brief,
        )
    }

    fn plain(view: &View) -> Vec<String> {
        view.lines.iter().map(ViewLine::plain).collect()
    }

    fn line_cells(line: &ViewLine) -> usize {
        line.segs.iter().map(|seg| cells(&seg.text)).sum()
    }

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

    fn tone_of(line: &ViewLine, text: &str) -> Option<Tone> {
        line.segs
            .iter()
            .find(|seg| seg.text == text)
            .map(|seg| seg.tone)
    }

    #[test]
    fn view_renders_a_literal_frame_for_one_row_of_each_state() {
        let rows = vec![
            with_brief(live("home/garden", "working"), "Grow the beds."),
            with_brief(
                dormant("home/kitchen", "2026-09-10"),
                "Stock the pantry and keep the knives sharp.",
            ),
            new_row("home/attic"),
        ];
        let state = AppState::new(rows);
        let v = view(&state, 64, 10, Indicators::Symbols);
        let pad = |n: usize| " ".repeat(n);
        assert_eq!(
            plain(&v),
            vec![
                format!("  / filter campaigns{}3 campaigns", pad(32)),
                format!(" {}", "─".repeat(63)),
                format!("> ◐ home/garden   Grow the beds.{}working", pad(24)),
                "    home/kitchen  Stock the pantry and keep th… last 2026-09-10".to_string(),
                format!("    home/attic{}new", pad(46)),
                "    Sort spaces".to_string(),
                String::new(),
                String::new(),
                "  home/garden · focus w-home/garden · agent working, wait or ex".to_string(),
                "  filter type · move ↑↓ · open enter · fresh ctrl+f · close esc".to_string(),
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
        assert_eq!(tone_of(&v.lines[2], "Grow the beds."), Some(Tone::Subtext));
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
        let v = view(&typed(state, "x"), 40, 20, Indicators::Dots);
        assert_eq!(v.lines[2].plain(), "  no campaigns");
        assert!(v.lines[0].plain().ends_with(" 0 campaigns"));
    }

    #[test]
    fn no_match_at_height_one_returns_only_the_filter_line() {
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
        assert!(
            v.lines[2].selected && v.lines[2].plain().contains("home/c00"),
            "{:?}",
            plain(&v)
        );
    }

    #[test]
    fn every_line_is_exactly_the_width_in_cells() {
        let mut rows = sample_rows();
        rows.push(new_row("home/日本語のキャンペーン"));
        rows.push(new_row("home/👩\u{200D}🔬lab"));
        rows.push(dormant("home/❤\u{FE0F}garden", "2026-09-01"));
        rows.push(live("home/🇬🇧e\u{301}cole", "done"));
        rows.push(live(
            "an-estate-with-a-very-long-name/and-an-equally-long-campaign-name",
            "blocked",
        ));
        rows.push(with_brief(
            new_row("home/brief"),
            "日本語の❤\u{FE0F}ブリーフ",
        ));
        let gutter = Seg {
            text: " ".to_string(),
            tone: Tone::Fill,
        };
        let queries = [
            "",
            "h",
            "日本",
            "zzz",
            "❤\u{FE0F}",
            "👩\u{200D}🔬l",
            "home/q",
            "home/Q",
        ];
        let mut states: Vec<AppState> = queries
            .iter()
            .map(|query| typed(creating(rows.clone()), query))
            .collect();
        states.push(typed(
            update(typed(creating(rows.clone()), "home/q"), Event::Enter),
            "日本語の❤\u{FE0F}ブリーフ",
        ));
        for ind in [Indicators::Dots, Indicators::Symbols] {
            for state in &states {
                for width in 0..=90 {
                    for height in [1, 2, 3, 5, 7, 12] {
                        let v = view(state, width, height, ind);
                        assert_eq!(v.lines.len(), height);
                        for line in &v.lines {
                            assert_eq!(
                                line_cells(line),
                                width,
                                "{width}x{height} {:?}: {:?}",
                                state.filter,
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
        let state = AppState::new(vec![new_row("home/日本語のキャンペーン")]);
        let v = view(&state, 20, 3, Indicators::Dots);
        assert_eq!(v.lines[2].plain(), ">   home/日本…  new");
        assert_eq!(line_cells(&v.lines[2]), 20);
    }

    #[test]
    fn text_splits_into_the_clusters_a_terminal_draws_as_one() {
        let table: &[(&str, &[&str])] = &[
            ("", &[]),
            ("e\u{301}cole", &["e\u{301}", "c", "o", "l", "e"]),
            ("❤\u{FE0F}x", &["❤\u{FE0F}", "x"]),
            ("1\u{FE0F}\u{20E3}", &["1\u{FE0F}\u{20E3}"]),
            ("👩\u{200D}🔬", &["👩\u{200D}🔬"]),
            ("⛹🏽\u{200D}♀\u{FE0F}", &["⛹🏽\u{200D}♀\u{FE0F}"]),
            ("a\u{200D}👩", &["a\u{200D}", "👩"]),
            ("👩\u{200D}a", &["👩\u{200D}", "a"]),
            ("日\u{200D}👩", &["日\u{200D}", "👩"]),
            ("🏽\u{200D}👩", &["🏽\u{200D}", "👩"]),
            ("🇬\u{200D}👩", &["🇬\u{200D}", "👩"]),
            ("👍🏽", &["👍🏽"]),
            ("🇬🇧🇯🇵🇺", &["🇬🇧", "🇯🇵", "🇺"]),
            ("a🇬", &["a", "🇬"]),
            ("\t🏽a\t\u{301}", &["\t", "🏽", "a", "\t", "\u{301}"]),
            ("ក\u{17D2}ក", &["ក\u{17D2}ក"]),
            ("\u{17D2}ក", &["\u{17D2}", "ក"]),
            ("ក\u{17D2}a", &["ក\u{17D2}", "a"]),
            ("a\u{17D2}ក", &["a\u{17D2}", "ក"]),
            ("\u{16D63}\u{16D67}a", &["\u{16D63}\u{16D67}", "a"]),
            ("a\u{16D67}", &["a", "\u{16D67}"]),
            ("日本", &["日", "本"]),
            ("\u{1100}\u{1161}\u{11A8}", &["\u{1100}\u{1161}\u{11A8}"]),
        ];
        for (text, want) in table {
            assert_eq!(clusters(text), *want, "{text:?}");
        }
    }

    #[test]
    fn a_cluster_takes_the_cells_ratatui_paints_it_in() {
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
            ("ｶ\u{FF9E}", 3),
            ("ក\u{17D2}ក", 1),
            ("\u{16D63}\u{16D67}", 1),
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
        let heart = "home/❤\u{FE0F}garden";
        assert_eq!(clip(heart, 6), "home/");
        assert_eq!(clip(heart, 7), "home/❤\u{FE0F}");
        assert_eq!(ellipsize(heart, 7), "home/…");
        assert_eq!(ellipsize(heart, 8), "home/❤\u{FE0F}…");
        let scientist = "home/👩\u{200D}🔬lab";
        assert_eq!(clip(scientist, 6), "home/");
        assert_eq!(clip(scientist, 7), "home/👩\u{200D}🔬");
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
        assert_eq!(
            view(&state, 18, 3, Indicators::Dots).lines[0].plain(),
            "  / f 6 campaigns"
        );
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
        assert_eq!(
            footer(64),
            "  filter type · move ↑↓ · open enter · fresh ctrl+f · close esc"
        );
        assert_eq!(
            footer(63),
            "  filter type · move ↑↓ · open enter · fresh ctrl+f"
        );
        assert_eq!(footer(14), "  filter type");
        assert_eq!(footer(13), "");
    }

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
        let expected: [&[Kind]; 10] = [
            &[],
            &[Filter],
            &[Filter, Row],
            &[Filter, Rule, Row],
            &[Filter, Rule, Row, Row],
            &[Filter, Rule, Row, Row, Footer],
            &[Filter, Rule, Row, Row, Row, Footer],
            &[Filter, Rule, Row, Row, Row, Detail, Footer],
            &[Filter, Rule, Row, Row, Row, Row, Detail, Footer],
            &[Filter, Rule, Row, Row, Row, Row, Blank, Detail, Footer],
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
        assert_eq!(wire_status("blocked"), Status::Blocked);
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
            assert_eq!(wire_status(wire), status, "{wire}");
            assert_eq!(status.word(), wire);
            assert_eq!(status.glyph(Indicators::Dots), dot, "{wire}");
            assert_eq!(status.glyph(Indicators::Symbols), symbol, "{wire}");
            assert_eq!(cells(dot), 1, "{dot}");
            assert_eq!(cells(symbol), 1, "{symbol}");
        }
        for wire in ["", "Working", "asleep"] {
            assert_eq!(wire_status(wire), Status::Unknown, "{wire:?}");
        }
        assert_eq!(Status::of(None), Status::Unknown);
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
            live("home/garden", "working"),
            dormant("home/kitchen", "2026-09-10"),
            new_row("home/attic"),
        ]);
        let detail = |state: &AppState| view(state, 68, 7, Indicators::Dots).lines[5].plain();
        assert_eq!(
            detail(&state),
            "  home/garden · focus w-home/garden · agent working, wait or exit"
        );
        state = update(state, Event::Down);
        assert_eq!(
            detail(&state),
            "  home/kitchen · resume from 2026-09-10 · fresh ctrl+f"
        );
        state = update(state, Event::Down);
        assert_eq!(detail(&state), "  home/attic · start fresh · fresh ctrl+f");
        state = typed(state, "zzz");
        assert_eq!(detail(&state), "");
        let narrow = AppState::new(vec![new_row("home/attic")]);
        assert_eq!(
            view(&narrow, 20, 7, Indicators::Dots).lines[5].plain(),
            "  home/attic · star"
        );
    }

    #[test]
    fn the_detail_line_names_the_ctrl_f_arm_state_on_idle_and_done() {
        let detail = |status: &str, armed_first| {
            let mut state = AppState::new(vec![live("home/garden", status)]);
            if armed_first {
                state = update(state, Event::Fresh);
            }
            view(&state, 80, 7, Indicators::Dots).lines[5].plain()
        };
        for status in ["idle", "done"] {
            assert_eq!(
                detail(status, false),
                "  home/garden · focus w-home/garden · fresh ctrl+f"
            );
            assert_eq!(
                detail(status, true),
                "  home/garden · focus w-home/garden · ctrl+f again: stop agent, start fresh"
            );
        }
        assert_eq!(
            detail("working", false),
            "  home/garden · focus w-home/garden · agent working, wait or exit"
        );
        assert_eq!(
            detail("blocked", false),
            "  home/garden · focus w-home/garden · agent blocked, wait or exit"
        );
        assert_eq!(
            detail("starting", false),
            "  home/garden · focus w-home/garden · agent busy, wait or exit"
        );
    }

    #[test]
    fn the_address_ends_in_an_ellipsis_and_gives_up_the_meta_below_eight_cells() {
        let state = AppState::new(vec![new_row(
            "an-estate-with-a-very-long-name/and-an-equally-long-campaign-name",
        )]);
        let row = |width| view(&state, width, 3, Indicators::Dots).lines[2].plain();
        assert_eq!(row(30), ">   an-estate-with-a-ver… new");
        assert_eq!(row(17), ">   an-esta… new");
        assert_eq!(row(16), ">   an-estate-…");
        let fits = AppState::new(vec![new_row("home/abc")]);
        assert_eq!(
            view(&fits, 17, 3, Indicators::Dots).lines[2].plain(),
            ">   home/abc new"
        );
    }

    #[test]
    fn as_a_row_narrows_the_brief_goes_first_then_the_meta() {
        let state = AppState::new(vec![with_brief(new_row("home/abc"), "Sweep it out.")]);
        let row = |width| view(&state, width, 3, Indicators::Dots).lines[2].plain();
        assert_eq!(
            row(40),
            format!(">   home/abc  Sweep it out.{}new", " ".repeat(9))
        );
        assert_eq!(row(28), ">   home/abc  Sweep it… new");
        assert_eq!(row(20), ">   home/abc    new");
        assert_eq!(row(16), ">   home/abc");
    }

    #[test]
    fn the_address_column_is_the_longest_match_up_to_32_cells() {
        let long = format!("home/{}", "x".repeat(40));
        let state = AppState::new(vec![
            with_brief(new_row("home/garden"), "Grow."),
            with_brief(new_row(&long), "Long."),
        ]);
        let v = view(&state, 80, 7, Indicators::Dots);
        assert_eq!(column_of(&v.lines[2], "Grow."), Some(38));
        assert_eq!(column_of(&v.lines[3], "Long."), Some(38));
        assert!(v.lines[3]
            .plain()
            .starts_with(&format!("    home/{}…  Long.", "x".repeat(26))));
        let v = view(&typed(state, "garden"), 80, 7, Indicators::Dots);
        assert_eq!(column_of(&v.lines[2], "Grow."), Some(17));
    }

    #[test]
    fn the_cursor_sits_after_the_query_and_needs_column_four() {
        let state = AppState::new(sample_rows());
        assert_eq!(view(&state, 4, 5, Indicators::Dots).cursor, None);
        assert_eq!(view(&state, 5, 5, Indicators::Dots).cursor, Some((4, 0)));
        assert_eq!(view(&state, 40, 0, Indicators::Dots).cursor, None);
        let state = typed(state, "ho");
        assert_eq!(view(&state, 40, 5, Indicators::Dots).cursor, Some((6, 0)));
        assert_eq!(view(&state, 5, 5, Indicators::Dots).cursor, Some((4, 0)));
    }

    #[test]
    fn create_follows_the_matches_for_an_address_in_a_creatable_estate() {
        let rows = vec![live("home/garden", "working"), new_row("home/attic")];
        let state = update(typed(creating(rows), "home/ga"), Event::Down);
        let v = view(&state, 64, 7, Indicators::Dots);
        let pad = |n: usize| " ".repeat(n);
        assert_eq!(
            plain(&v),
            vec![
                format!("  / home/ga{}1/2 campaigns", pad(39)),
                format!(" {}", "─".repeat(63)),
                format!("  ● home/garden{}working", pad(41)),
                "> + create home/ga".to_string(),
                String::new(),
                String::new(),
                "  filter type · move ↑↓ · open enter · fresh ctrl+f · close esc".to_string(),
            ]
        );
        assert!(v.lines[3].selected);
        assert_eq!(tone_of(&v.lines[3], "+ create home/ga"), Some(Tone::Text));
        let plain_state = typed(
            AppState::new(vec![live("home/garden", "working")]),
            "home/ga",
        );
        let v = view(&plain_state, 64, 7, Indicators::Dots);
        assert!(plain(&v).iter().all(|l| !l.contains("+ create")));
    }

    #[test]
    fn the_selection_moves_between_the_matches_and_create() {
        let mut state = typed(creating(sample_rows()), "home/ga");
        assert_eq!(selected(&state), Some("home/garden"));
        state = update(state, Event::Down);
        assert_eq!(state.selected, Some(Selection::Create));
        state = update(state, Event::Down);
        assert_eq!(state.selected, Some(Selection::Create));
        state = update(state, Event::Up);
        assert_eq!(selected(&state), Some("home/garden"));
        state = typed(update(state, Event::Down), "rd");
        assert_eq!(state.selected, Some(Selection::Create));
        state = typed(state, "en");
        assert_eq!(selected(&state), Some("home/garden"));
        assert!(!plain(&view(&state, 40, 7, Indicators::Dots))
            .iter()
            .any(|l| l.contains("+ create")));
        state = typed(state, "x");
        assert_eq!(state.selected, Some(Selection::Create));
        state = typed(state, "_");
        assert_eq!(state.selected, None);
    }

    #[test]
    fn enter_or_ctrl_f_on_create_asks_for_a_brief_and_an_empty_one_creates_nothing() {
        for key in [Event::Enter, Event::Fresh] {
            let state = update(typed(creating(sample_rows()), "home/quern"), key);
            assert_eq!(state.brief.as_deref(), Some(""));
            assert_eq!(state.done, None);
            let state = update(state, Event::Enter);
            assert_eq!(state.done, None);
            let state = update(typed(state, "  "), Event::Enter);
            assert_eq!(state.done, None);
            assert!(state.brief.is_some());
            assert_eq!(
                view(&state, 64, 3, Indicators::Dots).lines[2].plain(),
                "  type a one-line brief first"
            );
        }
    }

    #[test]
    fn the_brief_is_one_line_of_at_most_120_characters() {
        let mut state = briefing(" Grind\t it x");
        state = update(state, Event::Backspace);
        for event in [Event::Up, Event::Down, Event::Fresh] {
            state = update(state, event);
        }
        assert_eq!(state.done, None);
        assert_eq!(
            update(state, Event::Enter).done,
            Some(Outcome::Create {
                address: "home/quern".to_string(),
                brief: "Grind it".to_string(),
            })
        );
        let full = briefing(&"b".repeat(121));
        assert_eq!(full.brief.as_deref(), Some("b".repeat(120).as_str()));
    }

    #[test]
    fn esc_goes_back_with_the_filter_kept_and_ctrl_c_quits_from_either_step() {
        let briefing = briefing("draft");
        let back = update(briefing.clone(), Event::Esc);
        assert_eq!(back.brief, None);
        assert_eq!(back.done, None);
        assert_eq!(back.filter, "home/quern");
        assert_eq!(back.selected, Some(Selection::Create));
        assert_eq!(
            update(back.clone(), Event::Esc).done,
            Some(Outcome::Cancelled)
        );
        assert_eq!(update(briefing, Event::Quit).done, Some(Outcome::Cancelled));
        assert_eq!(update(back, Event::Quit).done, Some(Outcome::Cancelled));
    }

    #[test]
    fn the_brief_step_is_the_input_then_where_it_goes_then_the_keys() {
        let v = view(&briefing("Grind the grain."), 64, 5, Indicators::Dots);
        assert_eq!(
            plain(&v),
            vec![
                "  brief for home/quern> Grind the grain.",
                "  in /e/home/campaigns/quern (untracked; not committed)",
                "  create and start enter · back esc · quit ctrl+c",
                "",
                "",
            ]
        );
        assert_eq!(v.cursor, Some((40, 0)));
        assert_eq!(
            tone_of(&v.lines[0], "brief for home/quern> "),
            Some(Tone::Muted)
        );
        assert_eq!(tone_of(&v.lines[0], "Grind the grain."), Some(Tone::Text));
        let v = view(
            &briefing("abcdefghijklmnopqrstuvwxyz"),
            40,
            1,
            Indicators::Dots,
        );
        assert_eq!(plain(&v), vec!["  brief for home/quern> mnopqrstuvwxyz"]);
        assert_eq!(v.cursor, Some((38, 0)));
        let cursor = |width| view(&briefing("ab"), width, 1, Indicators::Dots).cursor;
        assert_eq!((cursor(26), cursor(25)), (Some((24, 0)), None));
        let hint = |state: &AppState| view(state, 64, 3, Indicators::Dots).lines[2].plain();
        let keys = "  create and start enter · back esc · quit ctrl+c";
        assert_eq!(hint(&briefing("")), keys);
        let nudged = update(briefing(""), Event::Enter);
        assert_eq!(hint(&nudged), "  type a one-line brief first");
        assert_eq!(hint(&typed(nudged, "x")), keys);
    }

    #[test]
    fn an_address_that_cannot_be_created_says_why_and_enter_does_nothing() {
        for (query, reason) in [
            ("home/Quern", "  can't create: a-z 0-9 and single -"),
            ("lab/quern", "  no estate 'lab'"),
        ] {
            let state = typed(creating(sample_rows()), query);
            assert_eq!(
                view(&state, 64, 7, Indicators::Dots).lines[2].plain(),
                reason
            );
            let state = update(state, Event::Enter);
            assert_eq!((state.brief, state.done), (None, None));
        }
        let state = typed(creating(sample_rows()), "quern");
        assert_eq!(
            view(&state, 64, 7, Indicators::Dots).lines[2].plain(),
            "  no matching campaigns"
        );
        assert_eq!(update(state, Event::Enter).brief, None);
    }
}
