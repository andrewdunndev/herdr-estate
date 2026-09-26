use std::fmt;

#[derive(Debug)]
pub enum EstateError {
    Config(String),
    Usage(String),
    Guard(String),
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
