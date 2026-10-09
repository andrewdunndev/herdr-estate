use serde::Deserialize;

use crate::errors::EstateError;
use crate::herdrcli;
use crate::lock;
use crate::paths::non_empty_env;
use crate::progress;

pub const HOME_LABEL: &str = "~";
pub const HOOK_EVENT: &str = "workspace.created";
const MESSAGE_CAP: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Slot {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
}

pub trait Board {
    fn list(&mut self) -> Result<Vec<Slot>, EstateError>;
    fn place(&mut self, workspace_id: &str, insert_index: usize) -> Result<Vec<Slot>, EstateError>;
}

pub fn label_order(slots: &[Slot]) -> Vec<Slot> {
    let mut order = slots.to_vec();
    order.sort_by_cached_key(|s| (s.label != HOME_LABEL, s.label.to_lowercase()));
    order
}

pub fn sort_board(board: &mut impl Board) -> Result<usize, EstateError> {
    let mut current = board.list()?;
    let limit = current.len();
    let mut moves = 0;
    loop {
        let order = label_order(&current);
        let Some(index) = current
            .iter()
            .zip(&order)
            .position(|(have, want)| have.workspace_id != want.workspace_id)
        else {
            return Ok(moves);
        };
        if moves >= limit {
            return Err(EstateError::Protocol(format!(
                "still out of label order after {moves} moves"
            )));
        }
        let id = order[index].workspace_id.clone();
        current = board
            .place(&id, index)
            .map_err(|e| EstateError::Protocol(format!("{e}; {} moved before it", count(moves))))?;
        moves += 1;
        if current.get(index).map(|s| s.workspace_id.as_str()) != Some(id.as_str()) {
            return Err(EstateError::Protocol(format!(
                "workspace.move left {id} out of place at {index}"
            )));
        }
    }
}

fn count(moves: usize) -> String {
    match moves {
        1 => "1 workspace".to_string(),
        n => format!("{n} workspaces"),
    }
}

struct Socket;

impl Board for Socket {
    fn list(&mut self) -> Result<Vec<Slot>, EstateError> {
        slots(
            "workspace.list",
            herdrcli::call_api("workspace.list", serde_json::json!({}))?,
        )
    }

    fn place(&mut self, workspace_id: &str, insert_index: usize) -> Result<Vec<Slot>, EstateError> {
        slots(
            "workspace.move",
            herdrcli::call_api(
                "workspace.move",
                serde_json::json!({"workspace_id": workspace_id, "insert_index": insert_index}),
            )?,
        )
    }
}

fn slots(method: &str, result: serde_json::Value) -> Result<Vec<Slot>, EstateError> {
    #[derive(Deserialize)]
    struct Listing {
        workspaces: Vec<Slot>,
    }
    serde_json::from_value::<Listing>(result)
        .map(|l| l.workspaces)
        .map_err(|_| {
            EstateError::Protocol(format!(
                "{method}: herdr's reply carried no 'workspaces' list"
            ))
        })
}

pub fn run() -> i32 {
    run_on(&mut Socket)
}

pub fn in_picker() -> String {
    in_picker_on(&mut Socket)
}

fn in_picker_on(board: &mut impl Board) -> String {
    match sort_locked(board) {
        Ok(line) => {
            progress::append(&format!("sort: {line}"));
            line
        }
        Err(line) => {
            progress::append(&format!("error: sort: {line}"));
            format!("sort stopped: {line}")
        }
    }
}

fn run_on(board: &mut impl Board) -> i32 {
    let hook = non_empty_env("HERDR_PLUGIN_EVENT");
    if let Some(event) = hook.as_deref().filter(|e| *e != HOOK_EVENT) {
        progress::append(&format!("sort: not run for {event}, only {HOOK_EVENT}"));
        return 0;
    }
    match sort_locked(board) {
        Ok(line) => {
            progress::step(&format!("sort: {line}"));
            0
        }
        Err(line) => {
            progress::report_error(&format!("sort: {line}"));
            if hook.is_some() {
                0
            } else {
                1
            }
        }
    }
}

fn sort_locked(board: &mut impl Board) -> Result<String, String> {
    let sorted = lock::hold_sort()
        .map_err(|e| EstateError::Guard(e.to_string()))
        .and_then(|_held| sort_board(board));
    match sorted {
        Ok(0) => Ok("the workspaces are already in label order".to_string()),
        Ok(moves) => Ok(format!("moved {} into label order", count(moves))),
        Err(e) => Err(e.to_string().chars().take(MESSAGE_CAP).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{fake_api_seq, EnvGuard, TempDir};

    fn slot(id: &str, label: &str) -> Slot {
        Slot {
            workspace_id: id.to_string(),
            label: label.to_string(),
        }
    }

    fn labels(slots: &[Slot]) -> Vec<&str> {
        slots.iter().map(|s| s.label.as_str()).collect()
    }

    struct Herdr {
        workspaces: Vec<Slot>,
        listed: usize,
        moves: Vec<(String, usize)>,
        refuse_move: Option<usize>,
        ignore_moves: bool,
    }

    impl Herdr {
        fn new(workspaces: Vec<Slot>) -> Self {
            Herdr {
                workspaces,
                listed: 0,
                moves: Vec::new(),
                refuse_move: None,
                ignore_moves: false,
            }
        }
    }

    impl Board for Herdr {
        fn list(&mut self) -> Result<Vec<Slot>, EstateError> {
            self.listed += 1;
            Ok(self.workspaces.clone())
        }

        fn place(&mut self, id: &str, insert: usize) -> Result<Vec<Slot>, EstateError> {
            if self.refuse_move == Some(self.moves.len()) {
                return Err(EstateError::Protocol(
                    "workspace.move: unknown variant `workspace.move`".to_string(),
                ));
            }
            self.moves.push((id.to_string(), insert));
            if !self.ignore_moves {
                let source = self
                    .workspaces
                    .iter()
                    .position(|s| s.workspace_id == id)
                    .unwrap();
                let target = if source < insert { insert - 1 } else { insert };
                let moved = self.workspaces.remove(source);
                self.workspaces.insert(target, moved);
            }
            Ok(self.workspaces.clone())
        }
    }

    fn isolated(extra: &[(&str, &str)]) -> (TempDir, EnvGuard) {
        let d = TempDir::new();
        let state = d.path().to_str().unwrap().to_string();
        let mut sets = vec![("HERDR_PLUGIN_STATE_DIR", state.as_str())];
        sets.extend_from_slice(extra);
        let unsets: Vec<&str> = ["HERDR_PLUGIN_EVENT", "HERDR_SOCKET_PATH"]
            .into_iter()
            .filter(|k| !extra.iter().any(|(set, _)| set == k))
            .collect();
        let g = EnvGuard::new(&sets, &unsets);
        (d, g)
    }

    #[test]
    fn home_is_pinned_first_and_the_rest_sort_ignoring_case() {
        let board = [
            slot("w2", "tanuki/den"),
            slot("w3", "Zeta"),
            slot("w1", "~"),
            slot("w4", "alpha"),
            slot("w5", "Beta"),
            slot("w6", "~/notes"),
        ];
        assert_eq!(
            labels(&label_order(&board)),
            vec!["~", "alpha", "Beta", "tanuki/den", "Zeta", "~/notes"]
        );
    }

    #[test]
    fn labels_equal_but_for_case_keep_their_current_order() {
        let board = [slot("w1", "~"), slot("w2", "Den"), slot("w3", "den")];
        assert_eq!(label_order(&board), board.to_vec());
        let flipped = [slot("w1", "~"), slot("w3", "den"), slot("w2", "Den")];
        assert_eq!(label_order(&flipped), flipped.to_vec());
    }

    #[test]
    fn a_board_already_in_order_is_never_moved() {
        let mut herdr = Herdr::new(vec![
            slot("w1", "~"),
            slot("w2", "Alpha"),
            slot("w3", "beta"),
        ]);
        assert_eq!(sort_board(&mut herdr).unwrap(), 0);
        assert!(herdr.moves.is_empty());
    }

    #[test]
    fn a_new_workspace_moves_once_to_just_after_home() {
        let mut herdr = Herdr::new(vec![
            slot("w1", "~"),
            slot("w2", "tanuki/den"),
            slot("w3", "tanuki/harness"),
            slot("w9", "aaa-sort-test"),
        ]);
        assert_eq!(sort_board(&mut herdr).unwrap(), 1);
        assert_eq!(herdr.moves, vec![("w9".to_string(), 1)]);
        assert_eq!(
            labels(&herdr.workspaces),
            vec!["~", "aaa-sort-test", "tanuki/den", "tanuki/harness"]
        );
    }

    #[test]
    fn a_shuffled_board_ends_in_label_order_and_a_second_run_moves_nothing() {
        let mut herdr = Herdr::new(vec![
            slot("w5", "echo"),
            slot("w3", "Charlie"),
            slot("w1", "~"),
            slot("w4", "delta"),
            slot("w2", "Bravo"),
            slot("w6", "alpha"),
        ]);
        let moves = sort_board(&mut herdr).unwrap();
        assert!(moves <= 5, "{moves}");
        assert_eq!(
            labels(&herdr.workspaces),
            vec!["~", "alpha", "Bravo", "Charlie", "delta", "echo"]
        );
        herdr.moves.clear();
        assert_eq!(sort_board(&mut herdr).unwrap(), 0);
        assert!(herdr.moves.is_empty());
    }

    #[test]
    fn a_refused_move_stops_the_sort_and_names_what_moved_before_it() {
        let mut herdr = Herdr::new(vec![
            slot("w3", "charlie"),
            slot("w2", "bravo"),
            slot("w1", "~"),
        ]);
        herdr.refuse_move = Some(1);
        let err = sort_board(&mut herdr).unwrap_err().to_string();
        assert!(err.contains("unknown variant `workspace.move`"), "{err}");
        assert!(err.ends_with("1 workspace moved before it"), "{err}");
        assert_eq!(herdr.moves.len(), 1);
    }

    #[test]
    fn a_move_herdr_does_not_apply_stops_instead_of_looping() {
        let mut herdr = Herdr::new(vec![slot("w2", "bravo"), slot("w1", "~")]);
        herdr.ignore_moves = true;
        let err = sort_board(&mut herdr).unwrap_err().to_string();
        assert!(err.contains("left w1 out of place at 0"), "{err}");
        assert_eq!(herdr.moves.len(), 1);
    }

    #[test]
    fn a_hook_for_any_event_but_workspace_created_reads_nothing() {
        for event in ["workspace.moved", "workspace.reordered", "startup"] {
            let (_d, _g) = isolated(&[("HERDR_PLUGIN_EVENT", event)]);
            let mut herdr = Herdr::new(vec![slot("w2", "bravo"), slot("w1", "~")]);
            assert_eq!(run_on(&mut herdr), 0);
            assert_eq!(herdr.listed, 0);
            assert!(herdr.moves.is_empty());
            assert_eq!(
                progress::logged_messages(),
                vec![format!("sort: not run for {event}, only workspace.created")]
            );
        }
    }

    #[test]
    fn a_refused_move_in_the_hook_is_logged_and_exits_zero() {
        let (_d, _g) = isolated(&[("HERDR_PLUGIN_EVENT", HOOK_EVENT)]);
        let mut herdr = Herdr::new(vec![slot("w2", "bravo"), slot("w1", "~")]);
        herdr.refuse_move = Some(0);
        assert_eq!(run_on(&mut herdr), 0);
        let log = progress::logged_messages();
        assert_eq!(log.len(), 1, "{log:?}");
        assert!(log[0].starts_with("error: sort: workspace.move: unknown variant"));
    }

    #[test]
    fn a_refused_move_from_the_shell_exits_one() {
        let (_d, _g) = isolated(&[]);
        let mut herdr = Herdr::new(vec![slot("w2", "bravo"), slot("w1", "~")]);
        herdr.refuse_move = Some(0);
        assert_eq!(run_on(&mut herdr), 1);
    }

    #[test]
    fn a_long_herdr_error_is_capped_in_the_log() {
        let long = format!(
            "{{\"id\":\"x\",\"error\":{{\"message\":\"{}\"}}}}",
            "m".repeat(2000)
        );
        let (socket, served) = fake_api_seq(vec![long]);
        let (_d, _g) = isolated(&[("HERDR_SOCKET_PATH", socket.to_str().unwrap())]);
        assert_eq!(run(), 1);
        served.join().unwrap();
        let _ = std::fs::remove_file(&socket);
        let log = progress::logged_messages();
        assert!(log[0].len() < MESSAGE_CAP + 20, "{}", log[0].len());
    }

    #[test]
    fn over_the_socket_the_sort_lists_then_moves_and_creates_nothing() {
        let listing = |ids: &[(&str, &str)]| {
            serde_json::json!({"id": "x", "result": {"type": "workspace_list", "workspaces":
                ids.iter().map(|(id, label)| serde_json::json!({"workspace_id": id, "number": 1, "label": label})).collect::<Vec<_>>()
            }})
            .to_string()
        };
        let (socket, served) = fake_api_seq(vec![
            listing(&[("w1", "~"), ("w2", "tanuki/den"), ("w9", "aaa")]),
            listing(&[("w1", "~"), ("w9", "aaa"), ("w2", "tanuki/den")]),
        ]);
        let (_d, _g) = isolated(&[
            ("HERDR_SOCKET_PATH", socket.to_str().unwrap()),
            ("HERDR_PLUGIN_EVENT", HOOK_EVENT),
        ]);
        assert_eq!(run(), 0);
        let requests: Vec<serde_json::Value> = served
            .join()
            .unwrap()
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let _ = std::fs::remove_file(&socket);
        assert_eq!(requests[0]["method"], "workspace.list");
        assert_eq!(requests[1]["method"], "workspace.move");
        assert_eq!(
            requests[1]["params"],
            serde_json::json!({"workspace_id": "w9", "insert_index": 1})
        );
        assert_eq!(
            progress::logged_messages(),
            vec!["sort: moved 1 workspace into label order"]
        );
    }

    #[test]
    fn the_picker_gets_the_outcome_as_a_line_and_nothing_on_stderr_reaches_it() {
        let (_d, _g) = isolated(&[]);
        let mut herdr = Herdr::new(vec![slot("w2", "bravo"), slot("w1", "~")]);
        assert_eq!(
            in_picker_on(&mut herdr),
            "moved 1 workspace into label order"
        );
        assert_eq!(
            in_picker_on(&mut herdr),
            "the workspaces are already in label order"
        );
        assert_eq!(
            progress::logged_messages(),
            vec![
                "sort: moved 1 workspace into label order",
                "sort: the workspaces are already in label order",
            ]
        );
    }

    #[test]
    fn a_refused_move_in_the_picker_is_a_line_to_show_not_a_failure() {
        let (_d, _g) = isolated(&[]);
        let mut herdr = Herdr::new(vec![slot("w2", "bravo"), slot("w1", "~")]);
        herdr.refuse_move = Some(0);
        let line = in_picker_on(&mut herdr);
        assert!(
            line.starts_with("sort stopped: workspace.move: unknown variant"),
            "{line}"
        );
        assert!(progress::logged_messages()[0].starts_with("error: sort: workspace.move:"));
    }

    #[test]
    fn the_manifest_offers_sort_as_an_action() {
        let text = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml"));
        let manifest: toml::Table = toml::from_str(text).unwrap();
        let action = manifest["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"].as_str() == Some("sort"))
            .expect("no manifest action sort");
        assert_eq!(action["title"].as_str(), Some("Sort spaces"));
        let command: Vec<&str> = action["command"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(command, vec!["target/release/herdr-estate", "sort"]);
    }

    #[test]
    fn the_manifest_runs_sort_on_workspace_created_and_no_other_event() {
        let text = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml"));
        let manifest: toml::Table = toml::from_str(text).unwrap();
        let hooks = manifest["events"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["on"].as_str(), Some(HOOK_EVENT));
        let command: Vec<&str> = hooks[0]["command"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(command, vec!["target/release/herdr-estate", "sort"]);
    }
}
