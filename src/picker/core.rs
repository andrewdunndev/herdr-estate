//! The picker's pure core: state, events, sorting and rendering. No
//! terminal I/O and no herdr calls live here — `rows` builds the input,
//! `shell` drives this with real key events and draws the result.

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

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        s.chars().take(width).collect()
    }
}

fn state_label(state: &RowState) -> String {
    match state {
        RowState::Live {
            agent_status,
            workspace_id: _,
        } => format!("live ({agent_status})"),
        RowState::Dormant { last } => format!("dormant  last {last}"),
        RowState::New => "new".to_string(),
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

/// Render `state` as plain text lines: a filter line, then one line per
/// filtered row (or a "no match" line), each truncated — never wrapped —
/// to `width` chars, the whole frame truncated to `height` lines. When
/// more rows are filtered in than fit, the window scrolls so the
/// selected row is always among the lines drawn.
pub fn view(state: &AppState, width: usize, height: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if height == 0 {
        return lines;
    }
    lines.push(truncate(&format!("> {}", state.filter), width));

    let filtered = state.filtered();
    if filtered.is_empty() {
        if lines.len() < height {
            lines.push(truncate("no match", width));
        }
        return lines;
    }

    let window = height - 1;
    if window == 0 {
        return lines;
    }
    let selected_idx = current_index(&filtered, &state.selected_address).unwrap_or(0);
    let offset = scroll_offset(selected_idx, filtered.len(), window);

    for row in filtered.into_iter().skip(offset).take(window) {
        let marker = if state.selected_address.as_deref() == Some(row.address.as_str()) {
            ">"
        } else {
            " "
        };
        let text = format!("{marker} {:<30} {}", row.address, state_label(&row.state));
        lines.push(truncate(&text, width));
    }
    lines
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

    #[test]
    fn view_renders_a_literal_frame_for_one_row_of_each_state() {
        // Hand-transcribed expected text, not view()'s own output over
        // equivalent rows: pins the marker, the 30-wide address padding
        // and the exact label text (including the dormant date and the
        // live agent status) to literal strings, so a rendering
        // regression that drops or reshapes any of them fails here
        // instead of only failing a golden test that compares view()
        // against itself.
        let rows = vec![
            live("home/garden", "working"),
            dormant("home/kitchen", "2026-09-10"),
            new_row("home/attic"),
        ];
        let state = AppState::new(rows);
        let lines = view(&state, 60, 10);
        assert_eq!(
            lines,
            vec![
                "> ".to_string(),
                "> home/garden                    live (working)".to_string(),
                "  home/kitchen                   dormant  last 2026-09-10".to_string(),
                "  home/attic                     new".to_string(),
            ]
        );
    }

    #[test]
    fn view_truncates_every_line_at_narrow_widths() {
        let state = AppState::new(sample_rows());
        let lines = view(&state, 8, 20);
        for line in &lines {
            assert!(line.chars().count() <= 8, "line too wide: {line:?}");
        }
    }

    #[test]
    fn view_reports_no_match() {
        let mut state = AppState::new(sample_rows());
        for c in ['q', 'q', 'q'] {
            state = update(state, Event::Char(c));
        }
        let lines = view(&state, 40, 20);
        assert!(lines.iter().any(|l| l == "no match"));
    }

    #[test]
    fn view_truncates_a_row_whose_address_is_longer_than_the_width() {
        let rows = vec![new_row(
            "an-estate-with-a-very-long-name/and-an-equally-long-campaign-name",
        )];
        let state = AppState::new(rows);
        let lines = view(&state, 12, 20);
        assert!(lines.iter().all(|l| l.chars().count() <= 12));
        // The row line still starts with the selection marker and the
        // start of the address, truncated rather than dropped.
        assert!(lines[1].starts_with('>'));
    }

    #[test]
    fn view_at_zero_height_is_empty() {
        let state = AppState::new(sample_rows());
        assert_eq!(view(&state, 40, 0), Vec::<String>::new());
    }

    #[test]
    fn view_at_zero_width_never_panics() {
        let state = AppState::new(sample_rows());
        let lines = view(&state, 0, 5);
        assert!(lines.iter().all(|l| l.is_empty()));
    }

    #[test]
    fn empty_rows_show_no_match_immediately() {
        let state = AppState::new(Vec::new());
        let lines = view(&state, 40, 20);
        assert!(lines.iter().any(|l| l == "no match"));
    }

    #[test]
    fn no_match_at_height_one_returns_only_the_filter_line() {
        // The filter line alone already fills a 1-line frame; the
        // "no match" line must not be pushed past it.
        let mut state = AppState::new(sample_rows());
        for c in ['z', 'z', 'z'] {
            state = update(state, Event::Char(c));
        }
        let lines = view(&state, 40, 1);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with('>'));
    }

    fn many_rows(n: usize) -> Vec<Row> {
        (0..n).map(|i| new_row(&format!("home/c{i:02}"))).collect()
    }

    #[test]
    fn view_drops_rows_past_height_with_no_scroll_regression() {
        // Baseline: with the selection at the top, a short frame simply
        // shows the first `height - 1` rows (unchanged from before the
        // scroll window existed).
        let state = AppState::new(many_rows(25));
        let lines = view(&state, 40, 6);
        assert_eq!(lines.len(), 6);
        assert!(lines[1].contains("home/c00"));
        assert!(lines[5].contains("home/c04"));
    }

    #[test]
    fn view_scrolls_the_window_so_a_selection_past_the_page_is_still_drawn() {
        let mut state = AppState::new(many_rows(25));
        for _ in 0..24 {
            state = update(state, Event::Down);
        }
        // Selection is now the last row, home/c24 — with no scrolling
        // this would sit past a 6-line frame (5-row window) and never
        // be drawn.
        let lines = view(&state, 40, 6);
        assert_eq!(lines.len(), 6);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with('>') && l.contains("home/c24")),
            "{lines:?}"
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
        let lines = view(&state, 40, 6);
        // Selection is back at the very first row: the window must have
        // scrolled back up rather than staying pinned to the bottom.
        assert!(
            lines[1].starts_with('>') && lines[1].contains("home/c00"),
            "{lines:?}"
        );
    }
}
