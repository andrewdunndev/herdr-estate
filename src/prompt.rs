//! Assemble an estate's prompt files into ONE file: repeated
//! --append-system-prompt-file is last-wins on claude, so herdr-estate
//! passes exactly one, concatenated in the estate table's own order.

use std::path::PathBuf;

use crate::errors::EstateError;
use crate::paths::state_dir;

/// Where the assembled prompt would land — a pure computation, no I/O, so
/// a dry-run can name the flag it would pass without writing anything.
pub fn prompt_path(estate: &str, campaign: &str, prompt_files: &[PathBuf]) -> Option<PathBuf> {
    if prompt_files.is_empty() {
        return None;
    }
    Some(state_dir().join(format!("{estate}-{campaign}.prompt.md")))
}

pub fn write_prompt(
    estate: &str,
    campaign: &str,
    prompt_files: &[PathBuf],
) -> Result<Option<PathBuf>, EstateError> {
    let out = match prompt_path(estate, campaign, prompt_files) {
        Some(p) => p,
        None => return Ok(None),
    };
    let mut parts = Vec::with_capacity(prompt_files.len());
    for f in prompt_files {
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
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, TempDir};

    #[test]
    fn empty_list_is_none() {
        assert!(prompt_path("home", "garden", &[]).is_none());
    }

    #[test]
    fn no_io_for_path_only() {
        let d = TempDir::new();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        let p = prompt_path("home", "garden", &[PathBuf::from("/does/not/exist.md")]).unwrap();
        assert!(!p.exists());
        assert_eq!(p.file_name().unwrap(), "home-garden.prompt.md");
    }

    #[test]
    fn concatenates_in_order() {
        let d = TempDir::new();
        let base = d.path().join("base.md");
        let harness = d.path().join("harness.md");
        std::fs::write(&base, "BASE\n").unwrap();
        std::fs::write(&harness, "HARNESS\n").unwrap();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        let out = write_prompt("home", "garden", &[base, harness])
            .unwrap()
            .unwrap();
        let text = std::fs::read_to_string(out).unwrap();
        assert!(text.find("BASE").unwrap() < text.find("HARNESS").unwrap());
    }

    #[test]
    fn missing_prompt_file_raises() {
        let d = TempDir::new();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        assert!(write_prompt("home", "garden", &[d.path().join("missing.md")]).is_err());
    }

    #[test]
    fn no_prompt_files_returns_none() {
        assert_eq!(write_prompt("home", "garden", &[]).unwrap(), None);
    }
}
