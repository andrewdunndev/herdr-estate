//! Crossterm events into core events, ratatui drawing, and the terminal
//! guard. Dispatch of the row Enter selects, or of a campaign to create,
//! lives in `crate::dispatch`, not here — this module only drives the
//! event loop, hands the outcome off once the terminal is already
//! restored, and holds a failed dispatch's error on screen until a key
//! press. Run as a herdr action, with no terminal to drive, it opens the
//! picker popup instead.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use ratatui::backend::IntoCrossterm;
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::style::{SetBackgroundColor, SetForegroundColor};
use ratatui::crossterm::terminal::{Clear, ClearType};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::campaign;
use crate::detach;
use crate::dispatch;
use crate::errors::EstateError;
use crate::herdrcli;
use crate::paths::{self, non_empty_env};
use crate::picker::core::{
    self, AppState, Event as CoreEvent, Indicators, Outcome, Tone, ViewLine,
};
use crate::picker::rows;
use crate::picker::theme::{self, Palette};
use crate::progress;

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
        RestoreGuard { restore }
    }
}

/// Bracketed paste off, then ratatui's own restore.
fn restore() {
    let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
}

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        (self.restore)();
    }
}

/// The plugin id and pane entrypoint `herdr-plugin.toml` declares for
/// the popup picker (a test holds this to the manifest).
pub const PLUGIN_ID: &str = "herdr-estate";
pub const PICKER_ENTRYPOINT: &str = "picker";

/// Whether this `pick` opens the picker popup instead of drawing. herdr
/// runs a plugin action with no terminal, so a key bound to the `pick`
/// action has nothing to draw in and used to fail with "could not start
/// the terminal". The popup entrypoint gets one. A process that is
/// itself the entrypoint never re-opens it, whatever else is set, so the
/// popup can't open itself in a loop.
pub fn opens_popup_instead_of_drawing(render_once: bool) -> bool {
    !render_once
        && non_empty_env("HERDR_PLUGIN_ACTION_ID").is_some()
        && non_empty_env("HERDR_PLUGIN_ENTRYPOINT_ID").is_none()
}

/// Open the picker entrypoint as a popup over the active pane, the call
/// `herdr plugin pane open --plugin herdr-estate --entrypoint picker`
/// makes by hand. That popup runs `pick` again, with a terminal.
/// `$HERDR_PLUGIN_ID` names the plugin when herdr set it, so an install
/// under another id still opens its own pane.
pub fn open_popup() -> Result<i32, EstateError> {
    let plugin_id = non_empty_env("HERDR_PLUGIN_ID").unwrap_or_else(|| PLUGIN_ID.to_string());
    progress::step("opening the picker popup");
    herdrcli::call_herdr(
        &[
            "plugin".to_string(),
            "pane".to_string(),
            "open".to_string(),
            "--plugin".to_string(),
            plugin_id,
            "--entrypoint".to_string(),
            PICKER_ENTRYPOINT.to_string(),
        ],
        true,
    )?;
    Ok(0)
}

/// Build the row list, then either print one static frame
/// (`render_once`) or run the interactive picker and dispatch whatever
/// it selected. A `build_rows` failure (herdr unreachable, `workspace
/// list` malformed) is returned before the picker opens: it never opens
/// a half-true list. Inside herdr the popup's screen is painted herdr's
/// panel first (`Panel`), so the wait for that list shows no ring.
pub fn run(
    estates: &toml::Table,
    render_once: bool,
    width: usize,
    height: usize,
) -> Result<i32, EstateError> {
    if render_once {
        // Plain text has no colours, but it shows the same glyphs the
        // popup would.
        let built_rows = rows::build_rows(estates)?;
        let (_, ind) = theme::load();
        let state = AppState::new(built_rows);
        for line in core::view(&state, width, height, ind).lines {
            println!("{}", line.plain());
        }
        return Ok(0);
    }

    // herdr's look, read once as the popup opens (a theme changed while
    // it is open shows the next time it opens).
    let look = theme::load();
    let panel = Panel::new(look.0);
    let built_rows = rows::build_rows(estates)?;

    let state = AppState::new(built_rows).with_estates(creatable_estates(estates));
    let outcome = run_interactive(state, look)?;
    // A failure is held on screen: the popup closes the moment this
    // process exits, and an error returned straight to `cli::run` would
    // print into a pane that is already gone.
    let popup_workspace = detach::popup_workspace_id();
    let hand_off = HandOff {
        popup_workspace: popup_workspace.as_deref(),
        run: detach::hand_off,
    };
    match dispatch_outcome(
        estates,
        outcome,
        panel.0.as_ref(),
        &hand_off,
        &mut std::io::stdout(),
    ) {
        Ok(Dispatched::Done) => Ok(0),
        Ok(Dispatched::FailedAndSaid) => Ok(hold(&mut CrosstermEvents, &CROSSTERM_RAW_MODE)),
        Err(e) => Ok(report_and_hold(
            &e,
            &mut CrosstermEvents,
            &CROSSTERM_RAW_MODE,
        )),
    }
}

/// How a dispatch that returned ended, for `run`.
#[derive(Debug, PartialEq, Eq)]
enum Dispatched {
    Done,
    /// The run `detach::hand_off` handed to herdr failed; it logged why,
    /// and its output is already on this screen: the popup only holds it.
    FailedAndSaid,
}

/// The workspace the popup was opened over, and what a row on it is
/// handed to: `detach::hand_off` in production; a test substitutes a
/// recorder, the same seam `RawMode` is.
struct HandOff<'a> {
    popup_workspace: Option<&'a str>,
    run: fn(detach::RowAction, &str, &str) -> Result<bool, EstateError>,
}

/// Dispatch what the picker chose on the restored terminal, so its
/// progress lines show in the popup while `agent start` waits. ratatui's
/// restore went back to the main screen, where those lines show, so
/// inside herdr (`panel`) it is painted onto `out` again first. A cancel
/// exits next, so its paint never shows. A row whose live workspace is
/// the popup's own goes to `hand_off.run` instead: closing that
/// workspace would end this process partway (`detach`).
fn dispatch_outcome(
    estates: &toml::Table,
    outcome: Outcome,
    panel: Option<&Palette>,
    hand_off: &HandOff,
    out: &mut impl Write,
) -> Result<Dispatched, EstateError> {
    if let Some(p) = panel {
        let _ = paint_panel(out, p);
    }
    if let (Some(workspace), Some((action, address))) = (
        hand_off.popup_workspace,
        detach::self_hosted(&outcome, hand_off.popup_workspace),
    ) {
        return Ok(if (hand_off.run)(action, address, workspace)? {
            Dispatched::Done
        } else {
            Dispatched::FailedAndSaid
        });
    }
    match outcome {
        Outcome::Cancelled => Ok(()),
        Outcome::Selected(row) => dispatch::open_row(estates, &row),
        Outcome::Fresh(row) => dispatch::fresh_row(estates, &row.address),
        Outcome::FreshRunning(row) => dispatch::fresh_row_running(estates, &row.address),
        Outcome::Create { address, brief } => dispatch::create_and_open(estates, &address, &brief),
    }
    .map(|()| Dispatched::Done)
}

/// Paint a whole screen herdr's panel: `panel_bg` behind `text`, every
/// cell cleared to it, the cursor home. herdr's pane emulator
/// (libghostty-vt) erases with the cursor's background (Screen.zig
/// 1995-1998, used at :1830), so the clear fills every cell, and the
/// plain lines written after it (progress.rs's) keep those colours.
fn paint_panel(out: &mut impl Write, p: &Palette) -> std::io::Result<()> {
    ratatui::crossterm::queue!(
        out,
        SetBackgroundColor(p.panel_bg.into_crossterm()),
        SetForegroundColor(p.text.into_crossterm()),
        Clear(ClearType::All),
        MoveTo(0, 0)
    )?;
    out.flush()
}

/// herdr's panel on the popup's own screen, outside ratatui's frames:
/// painted as the picker starts and again by `dispatch_outcome`, then
/// SGR 0 when it drops, on every exit path. Only inside herdr
/// (`$HERDR_PLUGIN_ID` set), whose popup frame rings any cell left the
/// terminal's own background; a shell run keeps its terminal's colours.
struct Panel(Option<Palette>);

impl Panel {
    fn new(p: Palette) -> Self {
        let panel = Panel(non_empty_env("HERDR_PLUGIN_ID").map(|_| p));
        if let Some(p) = &panel.0 {
            let _ = paint_panel(&mut std::io::stdout(), p);
        }
        panel
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        if self.0.is_some() {
            let mut out = std::io::stdout();
            let _ = write!(out, "\x1b[0m").and_then(|()| out.flush());
        }
    }
}

/// Raw mode on and off. Crossterm's in production; a test substitutes
/// recorders, the same seam `RestoreGuard::restore` is.
struct RawMode {
    enable: fn() -> std::io::Result<()>,
    disable: fn() -> std::io::Result<()>,
}

const CROSSTERM_RAW_MODE: RawMode = RawMode {
    enable: ratatui::crossterm::terminal::enable_raw_mode,
    disable: ratatui::crossterm::terminal::disable_raw_mode,
};

/// A failed dispatch, kept readable: report it (stderr and the log),
/// then hold the popup open until a key press, since the popup closes
/// the moment this process exits. Raw mode, so a single key ends it with
/// no Enter. Returns 1, the code `cli::run` gives the same error.
/// Best-effort: a terminal that refuses raw mode or a read that fails
/// skips the wait rather than hanging the popup.
fn report_and_hold(err: &EstateError, events: &mut impl EventSource, raw: &RawMode) -> i32 {
    progress::report_error(err);
    hold(events, raw)
}

/// `report_and_hold`'s hold alone, for a failure already reported.
fn hold(events: &mut impl EventSource, raw: &RawMode) -> i32 {
    let _ = writeln!(std::io::stderr(), "press any key to close");
    let raw_on = (raw.enable)().is_ok();
    drain_pending(events);
    wait_for_press(events);
    if raw_on {
        let _ = (raw.disable)();
    }
    1
}

/// Throw away input typed before the error showed. Keys pressed while
/// `agent start` blocked sit in the tty's queue (the terminal was
/// restored to cooked mode for dispatch, and entering raw mode does not
/// flush it), and the first of them would end the hold before the error
/// could be read. Runs after raw mode is on, so a partial line typed in
/// cooked mode is readable too.
fn drain_pending(events: &mut impl EventSource) {
    while matches!(events.poll(Duration::ZERO), Ok(true)) {
        if events.read().is_err() {
            break;
        }
    }
}

/// Read until a key press: a resize, a key release or repeat, or any
/// other event is not the operator answering, so it keeps waiting. A
/// read error ends the wait.
fn wait_for_press(events: &mut impl EventSource) {
    loop {
        match events.read() {
            Ok(TermEvent::Key(key)) if key.kind == KeyEventKind::Press => return,
            Ok(_) => continue,
            Err(_) => return,
        }
    }
}

/// The estates a campaign can be created in, those whose campaigns
/// directory exists, each with that directory for the brief step, under
/// the root as estates.toml writes it (normally `~/...`), so the step's
/// "(untracked; not committed)" fits an 80-column popup.
fn creatable_estates(estates: &toml::Table) -> BTreeMap<String, String> {
    estates
        .iter()
        .filter_map(|(e, row)| {
            let dir = campaign::campaigns_dir(estates, e)
                .ok()
                .filter(|d| d.is_dir())?;
            let root = row.get("root")?.as_str()?;
            let shown = dir
                .strip_prefix(paths::expand(root))
                .map_or(dir.clone(), |sub| Path::new(root).join(sub));
            Some((e.clone(), shown.display().to_string()))
        })
        .collect()
}

fn run_interactive(state: AppState, look: (Palette, Indicators)) -> Result<Outcome, EstateError> {
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
    // A paste arrives whole, so its newlines can't press Enter.
    ratatui::crossterm::execute!(std::io::stdout(), EnableBracketedPaste)
        .map_err(|e| EstateError::Protocol(format!("could not start the terminal: {e}")))?;
    event_loop(&mut terminal, state, &mut CrosstermEvents, look)
}

/// Where `event_loop` gets its input from — real crossterm in
/// production, a scripted sequence in a test. Abstracting this (rather
/// than `event_loop` calling `crossterm::event::poll`/`read` directly)
/// is what lets a test drive the loop's resize-continue branch, its
/// no-event-continue branch and its key-event arm without a real
/// terminal: nothing could reach any of them before this seam existed.
trait EventSource {
    fn poll(&mut self, timeout: Duration) -> std::io::Result<bool>;
    fn read(&mut self) -> std::io::Result<TermEvent>;
}

struct CrosstermEvents;

impl EventSource for CrosstermEvents {
    fn poll(&mut self, timeout: Duration) -> std::io::Result<bool> {
        event::poll(timeout)
    }

    fn read(&mut self) -> std::io::Result<TermEvent> {
        event::read()
    }
}

/// The style a segment's tone is painted in: herdr's token for it, on
/// `panel_bg`. `Fill` is blank panel, so its foreground never shows.
fn tone_style(tone: Tone, p: &Palette) -> Style {
    let fg = match tone {
        Tone::Fill | Tone::Text => p.text,
        Tone::Subtext => p.subtext0,
        Tone::Muted => p.overlay0,
        Tone::Rule => p.surface1,
        Tone::Status(status) => p.status(status),
    };
    Style::new().fg(fg).bg(p.panel_bg)
}

/// The selection bar, herdr's selected-row style (overlays.rs:817-821):
/// contrast text on accent, bold. The glyph and meta take it too.
fn bar_style(p: &Palette) -> Style {
    Style::new()
        .fg(p.contrast())
        .bg(p.accent)
        .add_modifier(Modifier::BOLD)
}

/// One `ViewLine` as a styled ratatui line. On the selected line every
/// segment but the gutter takes the bar, so the bar spans columns
/// 1..=W−1 and the gutter stays panel.
fn styled_line(line: &ViewLine, p: &Palette) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .segs
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            let style = if line.selected && i > 0 {
                bar_style(p)
            } else {
                tone_style(seg.tone, p)
            };
            Span::styled(seg.text.clone(), style)
        })
        .collect();
    Line::from(spans)
}

/// Draw and read keys until `core::update` sets an outcome. The palette
/// and indicator style are passed in rather than loaded here, so a test
/// paints with a known palette and never reads the operator's herdr
/// config.
fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    mut state: AppState,
    events: &mut impl EventSource,
    (palette, ind): (Palette, Indicators),
) -> Result<Outcome, EstateError> {
    loop {
        terminal
            .draw(|frame| {
                let area = frame.area();
                // Every cell is panel_bg first, as herdr's `panel()`
                // fills its modals (overlays.rs:228-233), so the picker
                // and herdr's frame around it read as one surface.
                frame
                    .buffer_mut()
                    .set_style(area, Style::new().bg(palette.panel_bg).fg(palette.text));
                let view = core::view(&state, area.width as usize, area.height as usize, ind);
                let lines: Vec<Line> = view
                    .lines
                    .iter()
                    .map(|line| styled_line(line, &palette))
                    .collect();
                frame.render_widget(Paragraph::new(lines), area);
                // The picker is fullscreen, so the frame's area starts at
                // the origin and the view's cursor is already in terminal
                // coordinates. herdr forwards a popup's cursor to the
                // real terminal (client/shell.rs:270-277).
                if let Some(cursor) = view.cursor {
                    frame.set_cursor_position(cursor);
                }
            })
            .map_err(|e| EstateError::Protocol(format!("could not draw the picker: {e}")))?;

        // Poll so a resize or quiet terminal can't block here forever;
        // a real interactive session blocks on the next key as normal.
        let has_event = events
            .poll(Duration::from_millis(250))
            .map_err(|e| EstateError::Protocol(format!("could not poll for input: {e}")))?;
        if !has_event {
            continue;
        }

        match events
            .read()
            .map_err(|e| EstateError::Protocol(format!("could not read input: {e}")))?
        {
            TermEvent::Resize(_, _) => continue,
            TermEvent::Key(key) => {
                if let Some(core_event) = translate_key(key) {
                    state = core::update(state, core_event);
                }
            }
            // Typed in, first line only and never as Enter, so a paste
            // can't open a row or create a campaign. Terminals often
            // paste a newline as CR.
            TermEvent::Paste(text) => {
                let first = text.split(['\r', '\n']).next().unwrap_or("");
                for c in first.chars().filter(|c| !c.is_control()) {
                    state = core::update(state, CoreEvent::Char(c));
                }
            }
            _ => {}
        }

        if let Some(outcome) = state.done.clone() {
            return Ok(outcome);
        }
    }
}

/// `None` only for a release/repeat event on a platform that reports
/// them: those never reach `core::update`, so Ctrl-f's arm survives
/// them untouched, the same as it always has. Every genuine press
/// reaches `update` as `Some`, mapped to its own event or, for one this
/// picker gives no other meaning to, `CoreEvent::Other` -- so the arm
/// still disarms on it, the same as any mapped key.
fn translate_key(key: KeyEvent) -> Option<CoreEvent> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    // A chord, not text: Alt+f must not type "f", nor a Cmd chord herdr
    // passes through reach the filter or, with ctrl held, act as a key.
    // Still routed through as `Other` so the arm disarms on it like any
    // other key.
    if matches!(key.code, KeyCode::Char(_))
        && (key.modifiers.contains(KeyModifiers::ALT)
            || key.modifiers.contains(KeyModifiers::SUPER))
    {
        return Some(CoreEvent::Other);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(match key.code {
            KeyCode::Char('c') => CoreEvent::Quit,
            KeyCode::Char('n') => CoreEvent::Down,
            KeyCode::Char('p') => CoreEvent::Up,
            KeyCode::Char('f') => CoreEvent::Fresh,
            _ => CoreEvent::Other,
        });
    }
    Some(match key.code {
        KeyCode::Char(c) => CoreEvent::Char(c),
        KeyCode::Backspace => CoreEvent::Backspace,
        KeyCode::Up => CoreEvent::Up,
        KeyCode::Down => CoreEvent::Down,
        KeyCode::Enter => CoreEvent::Enter,
        KeyCode::Esc => CoreEvent::Esc,
        _ => CoreEvent::Other,
    })
}

#[cfg(test)]
mod event_loop_tests {
    use super::*;
    use crate::picker::core::{Row, RowState};
    use ratatui::backend::TestBackend;
    use std::collections::VecDeque;

    /// A scripted `EventSource`: `NoEvent` makes one `poll` return
    /// `Ok(false)` (the quiet-terminal branch); `Event` makes `poll`
    /// return `Ok(true)` and the next `read` return it. A script that
    /// runs out without the loop having reached `state.done` panics
    /// rather than spinning forever.
    enum Scripted {
        NoEvent,
        Event(TermEvent),
    }

    struct ScriptedEvents(VecDeque<Scripted>);

    impl ScriptedEvents {
        fn new(script: Vec<Scripted>) -> Self {
            ScriptedEvents(script.into())
        }
    }

    impl EventSource for ScriptedEvents {
        fn poll(&mut self, _timeout: Duration) -> std::io::Result<bool> {
            match self.0.front() {
                Some(Scripted::Event(_)) => Ok(true),
                Some(Scripted::NoEvent) => {
                    self.0.pop_front();
                    Ok(false)
                }
                None => panic!("event_loop polled past the end of the script"),
            }
        }

        fn read(&mut self) -> std::io::Result<TermEvent> {
            match self.0.pop_front() {
                Some(Scripted::Event(e)) => Ok(e),
                other => panic!("event_loop read with no scripted event: {other:?}"),
            }
        }
    }

    impl std::fmt::Debug for Scripted {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Scripted::NoEvent => write!(f, "NoEvent"),
                Scripted::Event(e) => write!(f, "Event({e:?})"),
            }
        }
    }

    fn key_press(code: KeyCode) -> TermEvent {
        TermEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl_key_press(c: char) -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn one_row() -> Vec<Row> {
        vec![Row {
            address: "home/garden".to_string(),
            state: RowState::New,
            brief: String::new(),
        }]
    }

    fn one_idle_running_row() -> Vec<Row> {
        vec![Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w1".to_string(),
                agent_status: "idle".to_string(),
            },
            brief: String::new(),
        }]
    }

    fn test_terminal() -> ratatui::Terminal<TestBackend> {
        ratatui::Terminal::new(TestBackend::new(40, 10)).unwrap()
    }

    /// herdr's default look, injected so no test reads a real config.
    fn catppuccin() -> (Palette, Indicators) {
        (Palette::catppuccin(), Indicators::Dots)
    }

    #[test]
    fn a_quiet_poll_continues_the_loop_instead_of_reading() {
        // If the `!has_event` guard were deleted, this would call
        // `read()` with nothing scripted for it and panic.
        let mut terminal = test_terminal();
        let mut events = ScriptedEvents::new(vec![
            Scripted::NoEvent,
            Scripted::NoEvent,
            Scripted::Event(key_press(KeyCode::Esc)),
        ]);
        let outcome = event_loop(
            &mut terminal,
            AppState::new(one_row()),
            &mut events,
            catppuccin(),
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Cancelled);
    }

    #[test]
    fn a_resize_event_continues_the_loop_instead_of_acting_on_it() {
        // If the resize arm's `continue` were deleted (falling through
        // to the `_ => {}` arm instead), the loop would still continue
        // — but it would then also miss `state.done` being unset and
        // loop with no event consumed. This pins the observable
        // behaviour: a resize changes nothing and the next scripted key
        // still decides the outcome.
        let mut terminal = test_terminal();
        let mut events = ScriptedEvents::new(vec![
            Scripted::Event(TermEvent::Resize(80, 24)),
            Scripted::Event(key_press(KeyCode::Esc)),
        ]);
        let outcome = event_loop(
            &mut terminal,
            AppState::new(one_row()),
            &mut events,
            catppuccin(),
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Cancelled);
    }

    #[test]
    fn an_unmapped_key_between_two_ctrl_f_presses_disarms_instead_of_firing() {
        // Ctrl-f arms an idle row; Tab is unmapped but must still reach
        // core::update and disarm it (translate_key routes it as
        // `Other` rather than dropping it as `None`, which would leave
        // the arm untouched). The next Ctrl-f then only re-arms, so Esc
        // cancels the picker rather than the second Ctrl-f firing
        // FreshRunning on an arm two key presses stale.
        let mut terminal = test_terminal();
        let mut events = ScriptedEvents::new(vec![
            Scripted::Event(ctrl_key_press('f')),
            Scripted::Event(key_press(KeyCode::Tab)),
            Scripted::Event(ctrl_key_press('f')),
            Scripted::Event(key_press(KeyCode::Esc)),
        ]);
        let outcome = event_loop(
            &mut terminal,
            AppState::new(one_idle_running_row()),
            &mut events,
            catppuccin(),
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Cancelled);
    }

    /// `home`, an estate a campaign can be created in.
    fn home() -> BTreeMap<String, String> {
        BTreeMap::from([("home".to_string(), "/e/home/campaigns".to_string())])
    }

    fn created(address: &str, brief: &str) -> Outcome {
        Outcome::Create {
            address: address.to_string(),
            brief: brief.to_string(),
        }
    }

    #[test]
    fn an_address_then_a_brief_creates_the_campaign() {
        let mut script: Vec<Scripted> = "home/q"
            .chars()
            .map(|c| Scripted::Event(key_press(KeyCode::Char(c))))
            .collect();
        for code in [KeyCode::Enter, KeyCode::Char('x'), KeyCode::Enter] {
            script.push(Scripted::Event(key_press(code)));
        }
        let state = AppState::new(one_row()).with_estates(home());
        let outcome = event_loop(
            &mut test_terminal(),
            state,
            &mut ScriptedEvents::new(script),
            catppuccin(),
        )
        .unwrap();
        assert_eq!(outcome, created("home/q", "x"));
    }

    #[test]
    fn a_paste_types_its_first_line_and_never_presses_enter() {
        // Were a paste's newlines Enter presses, the first would create
        // home/q with the brief "x" before the keys after it ran; were
        // its tab typed, the address would carry it. A CR ends the first
        // line as an LF does, and the second paste's LF ends its own.
        let script = vec![
            Scripted::Event(TermEvent::Paste("home/\tq\rx\r".to_string())),
            Scripted::Event(key_press(KeyCode::Enter)),
            Scripted::Event(TermEvent::Paste("y\nz\n".to_string())),
            Scripted::Event(key_press(KeyCode::Enter)),
        ];
        let state = AppState::new(one_row()).with_estates(home());
        let outcome = event_loop(
            &mut test_terminal(),
            state,
            &mut ScriptedEvents::new(script),
            catppuccin(),
        )
        .unwrap();
        assert_eq!(outcome, created("home/q", "y"));
    }

    #[test]
    fn only_an_estate_with_a_campaigns_directory_offers_create() {
        let d = crate::test_support::TempDir::new();
        let _env = crate::test_support::EnvGuard::set(&[("HOME", d.path().to_str().unwrap())]);
        let mut estates = toml::Table::new();
        for name in ["home", "bare", "gone"] {
            let mut row = toml::Table::new();
            row.insert("root".to_string(), toml::Value::String(format!("~/{name}")));
            estates.insert(name.to_string(), toml::Value::Table(row));
        }
        std::fs::create_dir_all(d.path().join("home/campaigns")).unwrap();
        std::fs::create_dir(d.path().join("bare")).unwrap();
        // Under the root as written, not expanded.
        assert_eq!(
            creatable_estates(&estates),
            BTreeMap::from([("home".to_string(), "~/home/campaigns".to_string())])
        );
    }

    #[test]
    fn a_key_event_reaches_core_update_and_can_select_a_row() {
        let mut terminal = test_terminal();
        let mut events = ScriptedEvents::new(vec![Scripted::Event(key_press(KeyCode::Enter))]);
        let outcome = event_loop(
            &mut terminal,
            AppState::new(one_row()),
            &mut events,
            catppuccin(),
        )
        .unwrap();
        assert_eq!(
            outcome,
            Outcome::Selected(Row {
                address: "home/garden".to_string(),
                state: RowState::New,
                brief: String::new(),
            })
        );
    }

    #[test]
    fn the_frame_is_herdrs_panel_with_a_gutter_a_selection_bar_and_a_cursor() {
        use ratatui::style::Color;
        // 40x10, catppuccin, one New row: the frame drawn before the Esc
        // is the one left in the backend.
        let mut terminal = test_terminal();
        let mut events = ScriptedEvents::new(vec![Scripted::Event(key_press(KeyCode::Esc))]);
        let outcome = event_loop(
            &mut terminal,
            AppState::new(one_row()),
            &mut events,
            catppuccin(),
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Cancelled);

        let panel = Color::Rgb(0x18, 0x18, 0x25);
        let accent = Color::Rgb(0x89, 0xb4, 0xfa);
        let buf = terminal.backend().buffer().clone();
        // The gutter is panel on every row, and so is every cell off the
        // selection bar: nothing shows the terminal's own background.
        for y in 0..10u16 {
            assert_eq!(buf[(0, y)].bg, panel, "gutter, row {y}");
            for x in 0..40u16 {
                if y != 2 || x == 0 {
                    assert_eq!(buf[(x, y)].bg, panel, "({x},{y})");
                }
            }
        }
        // The one row is selected: the bar spans columns 1..=39, bold,
        // in contrast text, and leaves the gutter out.
        for x in 1..=39u16 {
            let cell = &buf[(x, 2)];
            assert_eq!(cell.bg, accent, "bar at ({x},2)");
            assert_eq!(cell.fg, panel, "bar text at ({x},2)");
            assert!(cell.modifier.contains(Modifier::BOLD), "bold at ({x},2)");
        }
        assert_ne!(buf[(0, 2)].bg, accent);
        assert!(!buf[(0, 2)].modifier.contains(Modifier::BOLD));
        // Text starts at x=2 behind a blank x=1: the filter line, the
        // detail line and the footer. The address starts at x=4.
        for y in [0u16, 8, 9] {
            assert_eq!(buf[(1, y)].symbol(), " ", "row {y}");
            assert_ne!(buf[(2, y)].symbol(), " ", "row {y}");
        }
        assert_eq!(buf[(2, 0)].symbol(), "/");
        assert_eq!(buf[(4, 2)].symbol(), "h");
        assert_eq!(buf[(1, 1)].symbol(), "─");
        assert_eq!(buf[(1, 1)].fg, Color::Rgb(0x45, 0x47, 0x5a));
        // The cursor sits on the placeholder's first cell.
        terminal.backend_mut().assert_cursor_position((4, 0));
    }

    #[test]
    fn each_tone_paints_its_herdr_token_on_panel() {
        let p = Palette::catppuccin();
        let on_panel = |fg| Style::new().fg(fg).bg(p.panel_bg);
        assert_eq!(tone_style(Tone::Fill, &p), on_panel(p.text));
        assert_eq!(tone_style(Tone::Text, &p), on_panel(p.text));
        assert_eq!(tone_style(Tone::Subtext, &p), on_panel(p.subtext0));
        assert_eq!(tone_style(Tone::Muted, &p), on_panel(p.overlay0));
        assert_eq!(tone_style(Tone::Rule, &p), on_panel(p.surface1));
        assert_eq!(
            tone_style(Tone::Status(core::Status::Working), &p),
            on_panel(p.yellow)
        );
        assert_eq!(
            bar_style(&p),
            Style::new()
                .fg(p.contrast())
                .bg(p.accent)
                .add_modifier(Modifier::BOLD)
        );
    }

    #[test]
    fn the_popup_paints_in_the_look_herdrs_config_names() {
        use ratatui::style::Color;
        // The operator's theme, vesper, with symbols. `run` hands the
        // look it loads to the loop `run_interactive` runs once the
        // terminal is up, so this is the popup reading herdr's config.
        let d = crate::test_support::TempDir::new();
        let cfg = d.path().join("config.toml");
        std::fs::write(
            &cfg,
            "[theme]\nname = \"vesper\"\n[ui]\nstatus_indicators = \"symbols\"\n",
        )
        .unwrap();
        let _g =
            crate::test_support::EnvGuard::set(&[("HERDR_CONFIG_PATH", cfg.to_str().unwrap())]);
        let rows = vec![Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w1".to_string(),
                agent_status: "working".to_string(),
            },
            brief: String::new(),
        }];
        let mut terminal = test_terminal();
        let mut events = ScriptedEvents::new(vec![Scripted::Event(key_press(KeyCode::Esc))]);
        assert_eq!(
            event_loop(
                &mut terminal,
                AppState::new(rows),
                &mut events,
                theme::load()
            )
            .unwrap(),
            Outcome::Cancelled
        );
        let buf = terminal.backend().buffer();
        // herdr's symbol for working, not its default dot...
        assert_eq!(buf[(2, 2)].symbol(), "◐");
        // ...on vesper's accent bar (#ffc799), under its rule (#282828).
        assert_eq!(buf[(2, 2)].bg, Color::Rgb(255, 199, 153));
        assert_eq!(buf[(1, 1)].fg, Color::Rgb(40, 40, 40));
    }

    #[test]
    fn a_pick_dispatches_on_a_screen_painted_herdrs_panel_again() {
        // Inside herdr the main screen the dispatch's progress shows on
        // is painted vesper's panel first: #1a1a1a behind #ffffff,
        // cleared to, the cursor home. A shell run (no panel) leaves it
        // alone. Either way the live row's workspace is focused. The hold
        // for a failure is `run`'s, so a failed dispatch here fails fast
        // rather than waiting on this terminal for a key.
        let vesper = "[theme]\nname = \"vesper\"\n".parse().unwrap();
        let vesper = theme::resolve(Some(&vesper)).0;
        let painted = "\x1b[48;2;26;26;26m\x1b[38;2;255;255;255m\x1b[2J\x1b[1;1H";
        for (panel, want) in [(Some(&vesper), painted), (None, "")] {
            let d = crate::test_support::TempDir::new();
            let fake = crate::test_support::FakeHerdr::new();
            let _g = crate::test_support::EnvGuard::set(&[
                ("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap()),
                ("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap()),
            ]);
            fake.set_response(
                0,
                r#"{"result": {"workspaces": [
                    {"workspace_id": "w9", "label": "home/garden", "agent_status": "working"}
                ]}}"#,
                0,
            );
            let row = Row {
                address: "home/garden".to_string(),
                state: RowState::Live {
                    workspace_id: "w9".to_string(),
                    agent_status: "working".to_string(),
                },
                brief: String::new(),
            };
            let mut out = Vec::new();
            dispatch_outcome(
                &toml::Table::new(),
                Outcome::Selected(row),
                panel,
                &in_process(),
                &mut out,
            )
            .unwrap();
            assert_eq!(String::from_utf8(out).unwrap(), want, "{panel:?}");
            assert_eq!(fake.argv(1), vec!["workspace", "focus", "w9"]);
        }
    }

    fn must_not_hand_off(
        action: detach::RowAction,
        address: &str,
        _workspace: &str,
    ) -> Result<bool, EstateError> {
        panic!("handed off {action:?} {address}, a row that dispatches in this process");
    }

    /// A shell run's hand-off: no popup workspace, so every row
    /// dispatches in this process.
    fn in_process() -> HandOff<'static> {
        HandOff {
            popup_workspace: None,
            run: must_not_hand_off,
        }
    }

    thread_local! {
        static HANDED_OFF: std::cell::RefCell<Vec<(detach::RowAction, String, String)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn record_hand_off(
        action: detach::RowAction,
        address: &str,
        workspace: &str,
    ) -> Result<bool, EstateError> {
        HANDED_OFF.with(|h| {
            h.borrow_mut()
                .push((action, address.to_string(), workspace.to_string()))
        });
        Ok(true)
    }

    fn record_failed_hand_off(
        action: detach::RowAction,
        address: &str,
        workspace: &str,
    ) -> Result<bool, EstateError> {
        record_hand_off(action, address, workspace)?;
        Ok(false)
    }

    fn idle_row_in(workspace_id: &str) -> Row {
        Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: workspace_id.to_string(),
                agent_status: "idle".to_string(),
            },
            brief: String::new(),
        }
    }

    #[test]
    fn a_row_on_the_popups_own_workspace_is_handed_off_and_never_closed_from_here() {
        // The bug this guards: closing the popup's own workspace ends
        // the popup, so this process must make no herdr call of its own
        // for that row, not even the re-read, and pass the whole
        // dispatch to the hand-off with the row's action and address.
        let d = crate::test_support::TempDir::new();
        let fake = crate::test_support::FakeHerdr::new();
        let _g = crate::test_support::EnvGuard::set(&[
            ("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap()),
            ("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap()),
        ]);
        HANDED_OFF.with(|h| h.borrow_mut().clear());
        let hand_off = HandOff {
            popup_workspace: Some("w2"),
            run: record_hand_off,
        };
        let done = dispatch_outcome(
            &toml::Table::new(),
            Outcome::FreshRunning(idle_row_in("w2")),
            None,
            &hand_off,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(done, Dispatched::Done);
        assert_eq!(fake.call_count(), 0);
        let handed = HANDED_OFF.with(|h| h.borrow().clone());
        assert_eq!(
            handed,
            vec![(
                detach::RowAction::Stop,
                "home/garden".to_string(),
                "w2".to_string()
            )]
        );
    }

    #[test]
    fn a_failed_hand_off_is_held_without_reporting_it_a_second_time() {
        let d = crate::test_support::TempDir::new();
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().to_str().unwrap(),
        )]);
        let hand_off = HandOff {
            popup_workspace: Some("w2"),
            run: record_failed_hand_off,
        };
        let failed = dispatch_outcome(
            &toml::Table::new(),
            Outcome::Selected(idle_row_in("w2")),
            None,
            &hand_off,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(failed, Dispatched::FailedAndSaid);
        assert!(progress::logged_messages().is_empty());
    }

    #[test]
    fn a_row_in_another_workspace_dispatches_here_even_inside_a_popup() {
        // Same shape as `fresh_running_dispatches_to_stop_and_fresh_not_open_row`
        // below: with no estate configured the in-process stop path
        // fails resolving the campaign, which a hand-off never would.
        let d = crate::test_support::TempDir::new();
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().to_str().unwrap(),
        )]);
        let hand_off = HandOff {
            popup_workspace: Some("w2"),
            run: must_not_hand_off,
        };
        let err = dispatch_outcome(
            &toml::Table::new(),
            Outcome::FreshRunning(idle_row_in("w9")),
            None,
            &hand_off,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(err, EstateError::Config(msg) if msg.contains("no estate")));
    }

    #[test]
    fn fresh_running_dispatches_to_stop_and_fresh_not_open_row() {
        // The routing itself, not the guard it runs through (`verbs`
        // owns and tests that): `Outcome::FreshRunning` must reach
        // `dispatch::fresh_row_running`, never `open_row`'s plain focus.
        // With no `home` estate configured, `fresh_with_ids_stop_running`
        // fails resolving the campaign before it ever calls herdr; had
        // this gone to `open_row` instead, its Live-row branch calls
        // `workspace list` first regardless, so a herdr call of any kind
        // here would mean the wrong path ran.
        let d = crate::test_support::TempDir::new();
        let fake = crate::test_support::FakeHerdr::new();
        let _g = crate::test_support::EnvGuard::set(&[
            ("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap()),
            ("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap()),
        ]);
        let row = Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w1".to_string(),
                agent_status: "idle".to_string(),
            },
            brief: String::new(),
        };
        let err = dispatch_outcome(
            &toml::Table::new(),
            Outcome::FreshRunning(row),
            None,
            &in_process(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(err, EstateError::Config(msg) if msg.contains("no estate")));
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn fresh_running_reaches_the_stop_and_fresh_verb_not_the_ordinary_refusing_one() {
        // The test above shows FreshRunning does not reach `open_row`,
        // but `fresh_row` and `fresh_row_running` both resolve the
        // campaign first and fail identically with no estate
        // configured -- that alone cannot tell the two apart. With a
        // real estate and an idle, owned workspace they diverge
        // sharply: `fresh_row`'s `check_live_workspace` refuses an idle
        // workspace outright ("already open ... with a running
        // agent"), while `fresh_row_running` closes and recreates it.
        // A mutation routing `FreshRunning` to `fresh_row` instead
        // would turn this close call into that refusal.
        let tmp = crate::test_support::TempDir::new();
        let root = tmp.path().join("home");
        let cdir = root.join("campaigns").join("garden");
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(cdir.join("campaign.md"), "---\n---\nThe garden.\n").unwrap();
        let fake = crate::test_support::FakeHerdr::new();
        let user_home = tmp.path().join("userhome");
        std::fs::create_dir_all(&user_home).unwrap();
        let _g = crate::test_support::EnvGuard::set(&[
            ("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap()),
            (
                "HERDR_PLUGIN_STATE_DIR",
                tmp.path().join("state").to_str().unwrap(),
            ),
            ("HOME", user_home.to_str().unwrap()),
        ]);
        crate::ownership::record("home/garden", "w1").unwrap();
        fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle", "pane_count": 1}
            ]}}"#,
            0,
        );
        fake.set_response(
            1,
            r#"{"result": {"workspace": {"label": "home/garden", "agent_status": "idle", "pane_count": 1}}}"#,
            0,
        );
        fake.set_response(2, r#"{"result": {}}"#, 0); // close
        fake.set_response(
            3,
            r#"{"result": {
                "workspace": {"workspace_id": "w-new"},
                "root_pane": {"pane_id": "p1"}
            }}"#,
            0,
        ); // create
        fake.set_response(4, r#"{"result": {}}"#, 0); // start
        fake.set_response(5, r#"{"result": {}}"#, 0); // focus

        let mut home_table = toml::Table::new();
        home_table.insert(
            "root".to_string(),
            toml::Value::String(root.to_string_lossy().to_string()),
        );
        let mut estates = toml::Table::new();
        estates.insert("home".to_string(), toml::Value::Table(home_table));

        let row = Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w1".to_string(),
                agent_status: "idle".to_string(),
            },
            brief: String::new(),
        };
        dispatch_outcome(
            &estates,
            Outcome::FreshRunning(row),
            None,
            &in_process(),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(fake.argv(2), vec!["workspace", "close", "w1"]);
        assert_eq!(fake.argv(5), vec!["workspace", "focus", "w-new"]);
    }

    #[test]
    fn every_cluster_is_painted_where_the_core_measured_it() {
        use ratatui::buffer::CellWidth;
        use ratatui::layout::Position;
        // Addresses whose clusters take more or fewer cells than their
        // chars do one by one: a joined emoji (2 cells, 4 by char), an
        // emoji with its selector (2, 1 by char), a flag, a skin tone,
        // an accent spelled as macOS spells file names, a halfwidth
        // sound mark and CJK. ratatui measures each line afresh as it
        // paints it, so where it puts the right-aligned text, the end of
        // the bar and the end of the typed query shows whether the core
        // counted the same cells. Each address is typed into the filter
        // too, so the query goes through the same measure. The state is
        // typed before the loop starts, so the backend holds a single
        // frame: over several, ratatui skips a cell a wide char covers,
        // and TestBackend (unlike a terminal) keeps what was there.
        let p = Palette::catppuccin();
        let addresses = [
            "home/👩\u{200D}🔬lab",
            "home/❤\u{FE0F}garden",
            "home/🇬🇧uk",
            "home/👍🏽ok",
            "home/e\u{301}cole",
            "home/ｶ\u{FF9E}me",
            "home/日本",
        ];
        for address in addresses {
            for width in 12u16..=40 {
                let mut state = AppState::new(vec![Row {
                    address: address.to_string(),
                    state: RowState::New,
                    brief: String::new(),
                }]);
                for c in address.chars() {
                    state = core::update(state, CoreEvent::Char(c));
                }
                let mut terminal = ratatui::Terminal::new(TestBackend::new(width, 7)).unwrap();
                let mut events =
                    ScriptedEvents::new(vec![Scripted::Event(key_press(KeyCode::Esc))]);
                event_loop(&mut terminal, state, &mut events, (p, Indicators::Dots)).unwrap();
                let cursor = terminal.get_cursor_position().unwrap();
                let buf = terminal.backend().buffer();
                let at = |x: u16, y: u16| buf[(x, y)].symbol();
                let run = |y: u16, from: u16, to: u16| -> String {
                    (from..=to).map(|x| at(x, y)).collect()
                };
                let ctx = format!("{address:?} at width {width}");
                // The selected row's bar reaches the last column, whose
                // cell is blank...
                assert_eq!(buf[(width - 1, 2)].bg, p.accent, "{ctx}");
                assert_eq!(at(width - 1, 2), " ", "{ctx}");
                // ...and right-aligned text ends one cell short of it.
                if width >= 17 {
                    assert_eq!(run(2, width - 4, width - 2), "new", "{ctx}");
                    assert_eq!(run(0, width - 11, width - 2), "1 campaign", "{ctx}");
                }
                // The cursor sits right after the query as ratatui
                // painted it, stepping over each wide cluster's second
                // cell.
                let mut x = 4;
                while at(x, 0) != " " {
                    x += at(x, 0).cell_width().max(1);
                }
                assert_eq!(cursor, Position::new(x, 0), "{ctx}");
            }
        }
    }

    // --- wait_for_press: the hold after a failed dispatch ---

    #[test]
    fn the_error_hold_waits_through_a_resize_and_a_release_and_ends_on_the_first_press() {
        let mut release = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        let mut events = ScriptedEvents::new(vec![
            Scripted::Event(TermEvent::Resize(80, 24)),
            Scripted::Event(TermEvent::Key(release)),
            Scripted::Event(key_press(KeyCode::Esc)),
            Scripted::Event(key_press(KeyCode::Enter)),
        ]);
        wait_for_press(&mut events);
        // Stopped at the Esc press: the Enter after it is never read.
        assert_eq!(events.0.len(), 1);
    }

    #[test]
    fn a_failed_dispatch_is_logged_and_held_past_keys_typed_during_the_wait() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ON: AtomicUsize = AtomicUsize::new(0);
        static OFF: AtomicUsize = AtomicUsize::new(0);
        fn on() -> std::io::Result<()> {
            ON.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn off() -> std::io::Result<()> {
            // Raw mode goes off only after it went on.
            assert_eq!(ON.load(Ordering::SeqCst), 1);
            OFF.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        let d = crate::test_support::TempDir::new();
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().to_str().unwrap(),
        )]);
        // An Enter typed while `agent start` blocked is already queued
        // when the hold begins; the drain throws it away (the NoEvent
        // is the queue running dry), and only the Esc after it — the
        // operator answering the error — ends the hold.
        let mut events = ScriptedEvents::new(vec![
            Scripted::Event(key_press(KeyCode::Enter)),
            Scripted::NoEvent,
            Scripted::Event(key_press(KeyCode::Esc)),
        ]);
        let err = EstateError::Guard("home/garden: refused".to_string());
        let raw = RawMode {
            enable: on,
            disable: off,
        };
        assert_eq!(report_and_hold(&err, &mut events, &raw), 1);
        assert!(events.0.is_empty());
        assert_eq!(ON.load(Ordering::SeqCst), 1);
        assert_eq!(OFF.load(Ordering::SeqCst), 1);
        assert_eq!(
            crate::progress::logged_messages(),
            vec!["error: home/garden: refused"]
        );
    }

    #[test]
    fn a_terminal_that_refuses_raw_mode_still_holds_and_is_never_switched_back() {
        fn refuse() -> std::io::Result<()> {
            Err(std::io::Error::other("no tty"))
        }
        fn must_not_run() -> std::io::Result<()> {
            panic!("raw mode was never on, so it must not be turned off")
        }
        let d = crate::test_support::TempDir::new();
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().to_str().unwrap(),
        )]);
        let mut events = ScriptedEvents::new(vec![
            Scripted::NoEvent,
            Scripted::Event(key_press(KeyCode::Char('q'))),
        ]);
        let raw = RawMode {
            enable: refuse,
            disable: must_not_run,
        };
        let err = EstateError::Protocol("herdr agent start failed".to_string());
        assert_eq!(report_and_hold(&err, &mut events, &raw), 1);
        assert!(events.0.is_empty());
    }

    #[test]
    fn the_error_hold_ends_on_a_read_error_instead_of_hanging() {
        struct FailingRead;
        impl EventSource for FailingRead {
            fn poll(&mut self, _timeout: Duration) -> std::io::Result<bool> {
                unreachable!("wait_for_press only reads")
            }
            fn read(&mut self) -> std::io::Result<TermEvent> {
                Err(std::io::Error::other("tty gone"))
            }
        }
        wait_for_press(&mut FailingRead);
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
    fn ctrl_c_is_quit() {
        assert!(matches!(
            translate_key(press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(CoreEvent::Quit)
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
    fn ctrl_f_is_fresh() {
        assert!(matches!(
            translate_key(press(KeyCode::Char('f'), KeyModifiers::CONTROL)),
            Some(CoreEvent::Fresh)
        ));
    }

    #[test]
    fn a_char_with_alt_or_super_types_nothing_but_still_disarms() {
        // Even with ctrl held: Cmd+Ctrl+f is not Ctrl-f. It reaches
        // `update` as `Other` rather than `None`, so it still disarms
        // Ctrl-f's second press like any other key, even though it
        // types nothing into the filter.
        let cmd_ctrl = KeyModifiers::SUPER | KeyModifiers::CONTROL;
        for modifier in [KeyModifiers::ALT, KeyModifiers::SUPER, cmd_ctrl] {
            assert!(matches!(
                translate_key(press(KeyCode::Char('f'), modifier)),
                Some(CoreEvent::Other)
            ));
        }
    }

    #[test]
    fn ctrl_with_an_unmapped_letter_disarms_only() {
        assert!(matches!(
            translate_key(press(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            Some(CoreEvent::Other)
        ));
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
    fn an_unmapped_key_disarms_only() {
        // Tab does nothing of its own, but it must still reach `update`
        // as `Other` rather than being dropped as `None`: dropped, it
        // would never run the disarm and Ctrl-f's arm would survive it.
        assert!(matches!(
            translate_key(press(KeyCode::Tab, KeyModifiers::NONE)),
            Some(CoreEvent::Other)
        ));
    }

    #[test]
    fn a_non_press_kind_is_ignored_even_for_a_mapped_key() {
        let mut key = press(KeyCode::Enter, KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert!(translate_key(key).is_none());
    }
}

#[cfg(test)]
mod popup_tests {
    use super::*;
    use crate::test_support::{EnvGuard, FakeHerdr, TempDir};

    const LAUNCH_VARS: [&str; 3] = [
        "HERDR_PLUGIN_ACTION_ID",
        "HERDR_PLUGIN_ENTRYPOINT_ID",
        "HERDR_PLUGIN_ID",
    ];

    /// Set exactly `sets` among herdr's launch variables, unsetting the
    /// rest, under one guard.
    fn launched_with(sets: &[(&str, &str)]) -> EnvGuard {
        let unsets: Vec<&str> = LAUNCH_VARS
            .iter()
            .copied()
            .filter(|k| !sets.iter().any(|(s, _)| s == k))
            .collect();
        EnvGuard::new(sets, &unsets)
    }

    #[test]
    fn a_pick_run_as_a_herdr_action_opens_the_popup() {
        let _g = launched_with(&[("HERDR_PLUGIN_ACTION_ID", "pick")]);
        assert!(opens_popup_instead_of_drawing(false));
    }

    #[test]
    fn the_popup_entrypoint_itself_draws_and_never_reopens() {
        let _g = launched_with(&[
            ("HERDR_PLUGIN_ACTION_ID", "pick"),
            ("HERDR_PLUGIN_ENTRYPOINT_ID", "picker"),
        ]);
        assert!(!opens_popup_instead_of_drawing(false));
    }

    #[test]
    fn a_shell_run_draws() {
        let _g = launched_with(&[]);
        assert!(!opens_popup_instead_of_drawing(false));
    }

    #[test]
    fn render_once_never_opens_a_popup_even_as_an_action() {
        let _g = launched_with(&[("HERDR_PLUGIN_ACTION_ID", "pick")]);
        assert!(!opens_popup_instead_of_drawing(true));
    }

    fn popup_fixture(plugin_id: Option<&str>) -> (TempDir, FakeHerdr, EnvGuard) {
        let d = TempDir::new();
        let fake = FakeHerdr::new();
        let state = d.path().join("state");
        let mut sets = vec![
            (
                "HERDR_BIN_PATH",
                fake.bin_path().to_str().unwrap().to_string(),
            ),
            (
                "HERDR_PLUGIN_STATE_DIR",
                state.to_str().unwrap().to_string(),
            ),
        ];
        if let Some(id) = plugin_id {
            sets.push(("HERDR_PLUGIN_ID", id.to_string()));
        }
        let sets: Vec<(&str, &str)> = sets.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let unsets: &[&str] = if plugin_id.is_some() {
            &[]
        } else {
            &["HERDR_PLUGIN_ID"]
        };
        let g = EnvGuard::new(&sets, unsets);
        (d, fake, g)
    }

    #[test]
    fn open_popup_asks_herdr_for_the_picker_pane_and_says_so() {
        let (_d, fake, _g) = popup_fixture(None);
        fake.set_response(0, r#"{"result": {}}"#, 0);
        assert_eq!(open_popup().unwrap(), 0);
        assert_eq!(fake.call_count(), 1);
        assert_eq!(
            fake.argv(0),
            vec![
                "plugin",
                "pane",
                "open",
                "--plugin",
                "herdr-estate",
                "--entrypoint",
                "picker"
            ]
        );
        assert_eq!(
            crate::progress::logged_messages(),
            vec!["opening the picker popup"]
        );
    }

    #[test]
    fn open_popup_uses_the_plugin_id_herdr_set() {
        let (_d, fake, _g) = popup_fixture(Some("estate-dev"));
        fake.set_response(0, r#"{"result": {}}"#, 0);
        open_popup().unwrap();
        assert_eq!(fake.argv(0)[4], "estate-dev");
    }

    #[test]
    fn open_popup_fails_when_herdr_refuses_the_pane() {
        let (_d, fake, _g) = popup_fixture(None);
        fake.set_response(0, "", 1);
        assert!(matches!(open_popup(), Err(EstateError::Protocol(_))));
    }

    #[test]
    fn the_plugin_id_and_entrypoint_match_the_manifest() {
        // A rename in herdr-plugin.toml that missed these constants
        // would leave the action opening a pane herdr doesn't know.
        let manifest: toml::Table =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml"))
                .unwrap()
                .parse()
                .unwrap();
        assert_eq!(manifest["id"].as_str(), Some(PLUGIN_ID));
        let panes = manifest["panes"].as_array().unwrap();
        let picker = panes
            .iter()
            .find(|p| p["id"].as_str() == Some(PICKER_ENTRYPOINT))
            .expect("a [[panes]] entry with the picker's id");
        assert_eq!(picker["placement"].as_str(), Some("popup"));
        // A share of the window, not fixed cells, so the popup follows
        // the window when it is resized.
        for dim in ["width", "height"] {
            let v = picker[dim].as_str().unwrap_or("");
            let pct: u32 = v
                .strip_suffix('%')
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            assert!(
                (1..100).contains(&pct),
                "popup {dim} {v:?} is not a percentage under 100%"
            );
        }
        let actions = manifest["actions"].as_array().unwrap();
        assert!(actions.iter().any(|a| a["id"].as_str() == Some("pick")));
    }
}
