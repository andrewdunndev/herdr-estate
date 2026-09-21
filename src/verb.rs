//! The two verbs an adapter builds argv for. `list` is a third top-level
//! command but never reaches an adapter, so it has no place here.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Resume,
    Fresh,
}
