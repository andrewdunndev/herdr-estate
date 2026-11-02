use std::path::PathBuf;

use crate::campaign::Campaign;
use crate::errors::EstateError;
use crate::paths::state_dir;

pub fn prompt_path(estate: &str, campaign: &str) -> PathBuf {
    state_dir().join(format!("{estate}-{campaign}.prompt.md"))
}

fn identity_block(campaign: &Campaign) -> String {
    let dir = &campaign.dir;
    let address = format!("{}/{}", campaign.estate, campaign.name);
    if dir.as_os_str().is_empty() {
        return format!("## This session\n\nAddress: `{address}`.");
    }
    format!(
        "## This session\n\nAddress: `{address}`. Campaign directory: `{}` (read campaign.md and the log.md tail there on re-entry).",
        dir.display()
    )
}

pub fn write_prompt(campaign: &Campaign) -> Result<PathBuf, EstateError> {
    let out = prompt_path(&campaign.estate, &campaign.name);
    let mut parts = vec![identity_block(campaign)];
    for f in &campaign.prompt_files {
        if !f.is_file() {
            return Err(EstateError::Config(format!(
                "prompt file missing: {}",
                f.display()
            )));
        }
        let text = std::fs::read_to_string(f)
            .map_err(|e| EstateError::Config(format!("could not read {}: {e}", f.display())))?;
        parts.push(text.trim().to_string());
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            EstateError::Config(format!("could not create {}: {e}", parent.display()))
        })?;
    }
    let content = format!("{}\n", parts.join("\n\n"));
    std::fs::write(&out, content)
        .map_err(|e| EstateError::Config(format!("could not write {}: {e}", out.display())))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::campaign::AgentKind;
    use crate::test_support::{EnvGuard, TempDir};
    use std::collections::BTreeMap;

    fn campaign(files: Vec<PathBuf>) -> Campaign {
        Campaign {
            address: "home/garden".to_string(),
            estate: "home".to_string(),
            name: "garden".to_string(),
            dir: PathBuf::from("/srv/home/campaigns/garden"),
            workdir: PathBuf::from("/srv/home"),
            kind: AgentKind::Claude,
            add_dirs: Vec::new(),
            missing_add_dirs: Vec::new(),
            prompt_files: files,
            kind_flags: BTreeMap::new(),
            env: BTreeMap::new(),
            prepare: None,
            worktree: None,
        }
    }

    #[test]
    fn path_does_no_io() {
        let d = TempDir::new();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        let p = prompt_path("home", "garden");
        assert!(!p.exists());
        assert_eq!(p.file_name().unwrap(), "home-garden.prompt.md");
    }

    #[test]
    fn concatenates_in_order_after_identity() {
        let d = TempDir::new();
        let base = d.path().join("base.md");
        let harness = d.path().join("harness.md");
        std::fs::write(&base, "BASE\n").unwrap();
        std::fs::write(&harness, "HARNESS\n").unwrap();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        let out = write_prompt(&campaign(vec![base, harness])).unwrap();
        let text = std::fs::read_to_string(out).unwrap();
        let id = text.find("## This session").unwrap();
        assert_eq!(id, 0);
        assert!(id < text.find("BASE").unwrap());
        assert!(text.find("BASE").unwrap() < text.find("HARNESS").unwrap());
        assert!(text.contains(
            "Address: `home/garden`. Campaign directory: `/srv/home/campaigns/garden` (read campaign.md"
        ));
    }

    #[test]
    fn missing_prompt_file_raises() {
        let d = TempDir::new();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        assert!(write_prompt(&campaign(vec![d.path().join("missing.md")])).is_err());
    }

    #[test]
    fn no_prompt_files_still_writes_identity() {
        let d = TempDir::new();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        let out = write_prompt(&campaign(Vec::new())).unwrap();
        let text = std::fs::read_to_string(out).unwrap();
        assert!(text.starts_with("## This session\n\nAddress: `home/garden`."));
    }

    #[test]
    fn no_dir_states_address_only() {
        let mut c = campaign(Vec::new());
        c.dir = PathBuf::new();
        let b = identity_block(&c);
        assert!(b.contains("`home/garden`"));
        assert!(!b.contains("Campaign directory"));
    }
}
