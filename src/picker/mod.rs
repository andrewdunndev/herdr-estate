//! The popup campaign picker: `herdr-estate pick`.
//!
//! `core` is the pure state machine (no terminal I/O, no herdr calls);
//! `rows` builds its input from the estate table, the continuity logs
//! and one `workspace list` call; `shell` drives `core` with real
//! crossterm events and draws it with ratatui. Dispatch of the row Enter
//! selects lives beside the verbs, in `crate::dispatch`, not here.

pub mod core;
pub mod rows;
pub mod shell;
