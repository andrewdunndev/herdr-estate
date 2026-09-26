use std::path::{Path, PathBuf};

use crate::errors::EstateError;
use crate::paths::{home_dir, plugin_config_dir};

pub const DEFAULT_CAMPAIGNS_DIR: &str = "campaigns";
pub const DEFAULT_AGENT_KIND: &str = "claude";

pub fn config_path() -> PathBuf {
    let herdrs = plugin_config_dir().join("estates.toml");
    if herdrs.is_file() {
        return herdrs;
    }
    let legacy = legacy_config_path();
    if legacy.is_file() {
        return legacy;
    }
    herdrs
}

fn legacy_config_path() -> PathBuf {
    home_dir()
        .join(".config")
        .join("herdr-estate")
        .join("estates.toml")
}

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

    fn write_legacy(home: &Path) -> PathBuf {
        let p = home
            .join(".config")
            .join("herdr-estate")
            .join("estates.toml");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, ESTATES_TOML).unwrap();
        p
    }

    #[test]
    fn a_legacy_table_is_read_when_herdrs_config_dir_has_none() {
        let d = TempDir::new();
        let home = TempDir::new();
        let legacy = write_legacy(home.path());
        let _g = EnvGuard::set(&[
            ("HERDR_PLUGIN_CONFIG_DIR", d.path().to_str().unwrap()),
            ("HOME", home.path().to_str().unwrap()),
        ]);
        assert_eq!(config_path(), legacy);
    }

    #[test]
    fn a_shell_run_reads_the_table_in_herdrs_config_dir_first() {
        let home = TempDir::new();
        write_legacy(home.path());
        let herdrs = home
            .path()
            .join(".config/herdr/plugins/config/herdr-estate/estates.toml");
        std::fs::create_dir_all(herdrs.parent().unwrap()).unwrap();
        std::fs::write(&herdrs, ESTATES_TOML).unwrap();
        let _g = EnvGuard::new(
            &[("HOME", home.path().to_str().unwrap())],
            &["HERDR_PLUGIN_CONFIG_DIR", "XDG_CONFIG_HOME"],
        );
        assert_eq!(config_path(), herdrs);
    }

    #[test]
    fn a_shell_run_falls_back_to_the_legacy_table() {
        let home = TempDir::new();
        let legacy = write_legacy(home.path());
        let _g = EnvGuard::new(
            &[("HOME", home.path().to_str().unwrap())],
            &["HERDR_PLUGIN_CONFIG_DIR", "XDG_CONFIG_HOME"],
        );
        assert_eq!(config_path(), legacy);
    }

    #[test]
    fn with_no_table_anywhere_herdrs_path_is_the_one_named() {
        let home = TempDir::new();
        let _g = EnvGuard::new(
            &[("HOME", home.path().to_str().unwrap())],
            &["HERDR_PLUGIN_CONFIG_DIR", "XDG_CONFIG_HOME"],
        );
        assert_eq!(
            config_path(),
            home.path()
                .join(".config/herdr/plugins/config/herdr-estate/estates.toml")
        );
    }
}
