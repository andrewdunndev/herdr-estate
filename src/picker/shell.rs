//! Crossterm events into core events, ratatui drawing, and the terminal
//! guard. Dispatch of the row Enter selects lives in `crate::dispatch`,
//! not here — this module only drives the event loop and hands the
//! chosen row off once the terminal is already restored.

use std::time::Duration;

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::dispatch;
use crate::errors::EstateError;
use crate::picker::core::{self, AppState, Event as CoreEvent, Outcome};
use crate::picker::rows;

/// Restores the terminal on drop — normal return, an early `?`, or a
/// panic unwinding through the scope that holds it — so raw mode and the
/// alternate screen are never left behind on any exit path. Constructed
/// only after `ratatui::try_init` has already succeeded, so a guard is
/// never live over a terminal `run_interactive` failed to enter (that
/// path restores directly — see `run_interactive`). The restore
/// function is a field, not a direct call, so a test can substitute one
/// and observe that `Drop` actually ran it.
struct RestoreGuard {
    restore: fn(),
}

impl RestoreGuard {
    fn new() -> Self {
        RestoreGuard {
            restore: ratatui::restore,
        }
    }
}

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        (self.restore)();
    }
}

/// Build the row list, then either print one static frame
/// (`render_once`) or run the interactive picker and dispatch whatever
/// it selected. A `build_rows` failure (herdr unreachable, `workspace
/// list` malformed) is returned before any terminal is touched: the
/// picker never opens a half-true list.
pub fn run(
    estates: &toml::Table,
    render_once: bool,
    width: usize,
    height: usize,
) -> Result<i32, EstateError> {
    let built_rows = rows::build_rows(estates)?;

    if render_once {
        let state = AppState::new(built_rows);
        for line in core::view(&state, width, height) {
            println!("{line}");
        }
        return Ok(0);
    }

    match run_interactive(built_rows)? {
        Outcome::Cancelled => Ok(0),
        Outcome::Selected(row) => {
            dispatch::open_row(estates, &row)?;
            Ok(0)
        }
    }
}

fn run_interactive(built_rows: Vec<core::Row>) -> Result<Outcome, EstateError> {
    // `try_init` enables raw mode before entering the alternate screen;
    // if the second step fails, raw mode is already on with nothing yet
    // constructed to undo it on drop. Restore directly on this path
    // rather than leaving the caller's terminal raw.
    let mut terminal = match ratatui::try_init() {
        Ok(t) => t,
        Err(e) => {
            ratatui::restore();
            return Err(EstateError::Protocol(format!(
                "could not start the terminal: {e}"
            )));
        }
    };
    let _guard = RestoreGuard::new();
    event_loop(&mut terminal, AppState::new(built_rows))
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    mut state: AppState,
) -> Result<Outcome, EstateError> {
    loop {
        terminal
            .draw(|frame| {
                let area = frame.area();
                let lines = core::view(&state, area.width as usize, area.height as usize);
                let text = ratatui::text::Text::from(
                    lines
                        .into_iter()
                        .map(ratatui::text::Line::from)
                        .collect::<Vec<_>>(),
                );
                frame.render_widget(ratatui::widgets::Paragraph::new(text), area);
            })
            .map_err(|e| EstateError::Protocol(format!("could not draw the picker: {e}")))?;

        // Poll so a resize or quiet terminal can't block here forever;
        // a real interactive session blocks on the next key as normal.
        let has_event = event::poll(Duration::from_millis(250))
            .map_err(|e| EstateError::Protocol(format!("could not poll for input: {e}")))?;
        if !has_event {
            continue;
        }

        match event::read()
            .map_err(|e| EstateError::Protocol(format!("could not read input: {e}")))?
        {
            TermEvent::Resize(_, _) => continue,
            TermEvent::Key(key) => {
                if let Some(core_event) = translate_key(key) {
                    state = core::update(state, core_event);
                }
            }
            _ => {}
        }

        if let Some(outcome) = state.done.clone() {
            return Ok(outcome);
        }
    }
}

/// `None` for a key press this picker gives no meaning to (ignored), or
/// for a release/repeat event on a platform that reports them.
fn translate_key(key: KeyEvent) -> Option<CoreEvent> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Some(CoreEvent::Esc),
            KeyCode::Char('n') => Some(CoreEvent::Down),
            KeyCode::Char('p') => Some(CoreEvent::Up),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char(c) => Some(CoreEvent::Char(c)),
        KeyCode::Backspace => Some(CoreEvent::Backspace),
        KeyCode::Up => Some(CoreEvent::Up),
        KeyCode::Down => Some(CoreEvent::Down),
        KeyCode::Enter => Some(CoreEvent::Enter),
        KeyCode::Esc => Some(CoreEvent::Esc),
        _ => None,
    }
}

#[cfg(test)]
mod restore_guard_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static CALLED: AtomicBool = AtomicBool::new(false);

    fn mark_called() {
        CALLED.store(true, Ordering::SeqCst);
    }

    #[test]
    fn drop_calls_the_restore_function() {
        // The doc comment promises raw mode and the alternate screen are
        // never left behind on any exit path — that promise rests on
        // `Drop::drop` actually running the restore function, which no
        // test observed before this one substituted a fake for the real
        // `ratatui::restore`.
        CALLED.store(false, Ordering::SeqCst);
        {
            let _guard = RestoreGuard {
                restore: mark_called,
            };
        }
        assert!(CALLED.load(Ordering::SeqCst));
    }
}

#[cfg(test)]
mod translate_key_tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn ctrl_c_is_esc() {
        assert!(matches!(
            translate_key(press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(CoreEvent::Esc)
        ));
    }

    #[test]
    fn ctrl_n_is_down() {
        assert!(matches!(
            translate_key(press(KeyCode::Char('n'), KeyModifiers::CONTROL)),
            Some(CoreEvent::Down)
        ));
    }

    #[test]
    fn ctrl_p_is_up() {
        assert!(matches!(
            translate_key(press(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            Some(CoreEvent::Up)
        ));
    }

    #[test]
    fn ctrl_with_an_unmapped_letter_is_ignored() {
        assert!(translate_key(press(KeyCode::Char('x'), KeyModifiers::CONTROL)).is_none());
    }

    #[test]
    fn a_plain_char_is_a_char_event() {
        assert!(matches!(
            translate_key(press(KeyCode::Char('a'), KeyModifiers::NONE)),
            Some(CoreEvent::Char('a'))
        ));
    }

    #[test]
    fn backspace_is_backspace() {
        assert!(matches!(
            translate_key(press(KeyCode::Backspace, KeyModifiers::NONE)),
            Some(CoreEvent::Backspace)
        ));
    }

    #[test]
    fn arrow_up_is_up() {
        assert!(matches!(
            translate_key(press(KeyCode::Up, KeyModifiers::NONE)),
            Some(CoreEvent::Up)
        ));
    }

    #[test]
    fn arrow_down_is_down() {
        assert!(matches!(
            translate_key(press(KeyCode::Down, KeyModifiers::NONE)),
            Some(CoreEvent::Down)
        ));
    }

    #[test]
    fn enter_is_enter() {
        assert!(matches!(
            translate_key(press(KeyCode::Enter, KeyModifiers::NONE)),
            Some(CoreEvent::Enter)
        ));
    }

    #[test]
    fn esc_is_esc() {
        assert!(matches!(
            translate_key(press(KeyCode::Esc, KeyModifiers::NONE)),
            Some(CoreEvent::Esc)
        ));
    }

    #[test]
    fn an_unmapped_key_is_ignored() {
        assert!(translate_key(press(KeyCode::Tab, KeyModifiers::NONE)).is_none());
    }

    #[test]
    fn a_non_press_kind_is_ignored_even_for_a_mapped_key() {
        let mut key = press(KeyCode::Enter, KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert!(translate_key(key).is_none());
    }
}
