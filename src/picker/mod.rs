//! The popup campaign picker: `herdr-estate pick`.
//!
//! `core` is the pure state machine and layout (no terminal I/O, no
//! herdr calls, no ratatui); `rows` builds its input from the estate
//! table, the continuity logs and one `workspace list` call; `theme`
//! copies herdr's colours and status glyphs, chosen from herdr's own
//! config; `shell` drives `core` with real crossterm events and paints
//! it with ratatui in those colours. Dispatch of the row Enter selects
//! lives beside the verbs, in `crate::dispatch`, not here.

pub mod core;
pub mod rows;
pub mod shell;
pub mod theme;
