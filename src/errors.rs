//! The error type this package returns on purpose, as opposed to a herdr
//! failure at the herdr CLI boundary (a separate error kind defined where
//! that boundary is crossed).

use std::fmt;

/// Every error this package returns deliberately, one hand-written enum
/// carrying only a message: the variant names the class of refusal; the
/// message is printed as-is.
#[derive(Debug)]
pub enum EstateError {
    /// The estate table, or an estate/campaign it names, can't be read.
    Config(String),
    /// The command line or the environment doesn't give enough to act on.
    Usage(String),
    /// A double-open or label-match guard refused to proceed.
    Guard(String),
    /// A herdr response was missing a field this code depends on, or
    /// wasn't shaped the way the herdr CLI boundary is documented to
    /// shape it.
    Protocol(String),
}

impl fmt::Display for EstateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            EstateError::Config(msg)
            | EstateError::Usage(msg)
            | EstateError::Guard(msg)
            | EstateError::Protocol(msg) => msg,
        };
        write!(f, "{msg}")
    }
}

impl std::error::Error for EstateError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_the_message_only() {
        let e = EstateError::Config("no estate table".to_string());
        assert_eq!(e.to_string(), "no estate table");
    }

    #[test]
    fn every_variant_displays_its_message() {
        assert_eq!(EstateError::Usage("u".into()).to_string(), "u");
        assert_eq!(EstateError::Guard("g".into()).to_string(), "g");
        assert_eq!(EstateError::Protocol("p".into()).to_string(), "p");
    }
}
