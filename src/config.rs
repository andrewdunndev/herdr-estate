//! The estate table: estates.toml, and the address it's keyed by.

use std::path::{Path, PathBuf};

use crate::errors::EstateError;
use crate::paths::home_dir;

pub const DEFAULT_CAMPAIGNS_DIR: &str = "campaigns";
pub const DEFAULT_AGENT_KIND: &str = "claude";

fn non_empty_env(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// `$HERDR_PLUGIN_CONFIG_DIR/estates.toml` when herdr sets that (running
/// as a plugin action) and the file is actually there; otherwise the
/// fallback a bare shell invocation uses.
pub fn config_path() -> PathBuf {
    if let Some(d) = non_empty_env("HERDR_PLUGIN_CONFIG_DIR") {
        let p = PathBuf::from(d).join("estates.toml");
        if p.is_file() {
            return p;
        }
    }
    home_dir()
        .join(".config")
        .join("herdr-estate")
        .join("estates.toml")
}

/// `{estate_name: estate_row}` from estates.toml's `[estate.*]` tables.
/// `path` overrides `config_path()` — used by tests and by a caller that
/// already resolved a path.
pub fn load_estates(path: Option<&Path>) -> Result<toml::Table, EstateError> {
    let p = match path {
        Some(p) => p.to_path_buf(),
        None => config_path(),
    };
    if !p.is_file() {
        return Err(EstateError::Config(format!(
            "no estate table at {}",
            p.display()
        )));
    }
    let text = std::fs::read_to_string(&p).map_err(|e| {
        EstateError::Config(format!(
            "could not read estate table at {}: {e}",
            p.display()
        ))
    })?;
    let data: toml::Table = toml::from_str(&text).map_err(|e| {
        EstateError::Config(format!("invalid estate table at {}: {e}", p.display()))
    })?;
    match data.get("estate").and_then(|v| v.as_table()) {
        Some(t) => Ok(t.clone()),
        None => Ok(toml::Table::new()),
    }
}

/// Split `estate/campaign` on the FIRST `/`; anything after further
/// slashes stays part of the campaign name.
pub fn parse_address(address: &str) -> Result<(String, String), EstateError> {
    match address.split_once('/') {
        Some((estate, campaign)) if !estate.is_empty() && !campaign.is_empty() => {
            Ok((estate.to_string(), campaign.to_string()))
        }
        _ => Err(EstateError::Config(format!(
            "address must be 'estate/campaign': {address:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, TempDir};

    const ESTATES_TOML: &str = r#"
[estate.home]
root = "~/estates/home"
campaigns = "campaigns"
agent = "claude"
prompts = ["prompts/base.md"]

[estate.home.env]
GREETING = "hi"
"#;

    #[test]
    fn parse_address_splits_on_first_slash() {
        assert_eq!(
            parse_address("home/garden").unwrap(),
            ("home".to_string(), "garden".to_string())
        );
    }

    #[test]
    fn parse_address_extra_slashes_go_to_campaign() {
        assert_eq!(
            parse_address("home/garden/sub").unwrap(),
            ("home".to_string(), "garden/sub".to_string())
        );
    }

    #[test]
    fn parse_address_missing_slash_rejected() {
        assert!(parse_address("home").is_err());
    }

    #[test]
    fn parse_address_empty_estate_rejected() {
        assert!(parse_address("/garden").is_err());
    }

    #[test]
    fn parse_address_empty_campaign_rejected() {
        assert!(parse_address("home/").is_err());
    }

    #[test]
    fn load_estates_loads_estate_table() {
        let d = TempDir::new();
        let p = d.path().join("estates.toml");
        std::fs::write(&p, ESTATES_TOML).unwrap();
        let estates = load_estates(Some(&p)).unwrap();
        let home = estates["home"].as_table().unwrap();
        assert_eq!(home["root"].as_str(), Some("~/estates/home"));
        let prompts: Vec<&str> = home["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(prompts, ["prompts/base.md"]);
        assert_eq!(
            home["env"].as_table().unwrap()["GREETING"].as_str(),
            Some("hi")
        );
    }

    #[test]
    fn load_estates_missing_file_raises() {
        let d = TempDir::new();
        assert!(load_estates(Some(&d.path().join("nope.toml"))).is_err());
    }

    #[test]
    fn config_path_uses_plugin_config_dir_when_file_exists() {
        let d = TempDir::new();
        let p = d.path().join("estates.toml");
        std::fs::write(&p, ESTATES_TOML).unwrap();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_CONFIG_DIR", d.path().to_str().unwrap())]);
        assert_eq!(config_path(), p);
    }

    #[test]
    fn config_path_falls_back_when_plugin_config_dir_file_missing() {
        let d = TempDir::new();
        let home = TempDir::new();
        let _g = EnvGuard::set(&[
            ("HERDR_PLUGIN_CONFIG_DIR", d.path().to_str().unwrap()),
            ("HOME", home.path().to_str().unwrap()),
        ]);
        assert_eq!(
            config_path(),
            home.path()
                .join(".config")
                .join("herdr-estate")
                .join("estates.toml")
        );
    }

    #[test]
    fn empty_env_value_counts_as_unset() {
        // Mirrors paths.rs's test of the same name for this module's own
        // copy of the same truthiness rule. Direct, not routed through
        // config_path()'s is_file() gate — that gate would swallow the
        // difference on a test machine with no ./estates.toml in the
        // working directory either way.
        let _g = EnvGuard::set(&[("HERDR_ESTATE_TEST_EMPTY", "")]);
        assert_eq!(non_empty_env("HERDR_ESTATE_TEST_EMPTY"), None);
    }

    #[test]
    fn config_path_falls_back_when_plugin_config_dir_unset() {
        let home = TempDir::new();
        let _g = EnvGuard::new(
            &[("HOME", home.path().to_str().unwrap())],
            &["HERDR_PLUGIN_CONFIG_DIR"],
        );
        assert_eq!(
            config_path(),
            home.path()
                .join(".config")
                .join("herdr-estate")
                .join("estates.toml")
        );
    }
}
