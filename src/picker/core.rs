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

/// Render `state` as plain text lines: a filter line, then one line per
/// filtered row (or a "no match" line), each truncated — never wrapped —
/// to `width` chars, the whole frame truncated to `height` lines.
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

    for row in filtered {
        if lines.len() >= height {
            break;
        }
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
}
