//! Per-kind adapters: turn a resolved Campaign plus a verb into the
//! agent's own argv. Generic keys map through the small table the design
//! note gives (paths, prompts, address, conversation id); a kind's own
//! frontmatter table (`claude:` in campaign.md) passes straight through as
//! `--<key>` flags, true meaning a bare flag and any other value meaning
//! `--<key> <value>`.
//!
//! Shipped: claude. An unknown kind gets no mapped flags at all — one
//! warning line, then bin/herdr-estate runs `<kind>` with nothing appended
//! but the bare conversation verb; adding a kind later is adding one more
//! match arm.

use std::path::Path;

use crate::campaign::{AgentKind, Campaign};
use crate::frontmatter::Scalar;
use crate::verb::Verb;

fn kind_flags_argv(campaign: &Campaign) -> Vec<String> {
    let mut argv = Vec::new();
    for (key, value) in &campaign.kind_flags {
        let flag = format!("--{key}");
        match value {
            Scalar::Bool(true) => argv.push(flag),
            Scalar::Bool(false) => continue,
            Scalar::Str(s) => {
                argv.push(flag);
                argv.push(s.clone());
            }
        }
    }
    argv
}

pub fn claude_argv(
    campaign: &Campaign,
    verb: Verb,
    conversation_id: &str,
    prompt_file: Option<&Path>,
) -> Vec<String> {
    let mut argv = vec!["claude".to_string()];
    match verb {
        Verb::Resume => {
            argv.push("--resume".to_string());
            argv.push(conversation_id.to_string());
        }
        Verb::Fresh => {
            argv.push("--session-id".to_string());
            argv.push(conversation_id.to_string());
        }
    }
    argv.push("--name".to_string());
    argv.push(campaign.address.clone());
    if let Some(pf) = prompt_file {
        argv.push("--append-system-prompt-file".to_string());
        argv.push(pf.display().to_string());
    }
    for d in &campaign.add_dirs {
        argv.push("--add-dir".to_string());
        argv.push(d.display().to_string());
    }
    argv.extend(kind_flags_argv(campaign));
    argv
}

/// Dispatch on `campaign.kind`. An unrecognised kind warns once to
/// stderr and starts the bare kind name with nothing appended but the
/// bare conversation verb — adding a kind later is adding one match arm
/// plus one `AgentKind` variant, not a registry lookup that can silently
/// return nothing for a typo'd shipped kind.
pub fn build_argv(
    campaign: &Campaign,
    verb: Verb,
    conversation_id: &str,
    prompt_file: Option<&Path>,
) -> Vec<String> {
    match &campaign.kind {
        AgentKind::Claude => claude_argv(campaign, verb, conversation_id, prompt_file),
        AgentKind::Other(name) => {
            eprintln!(
                "herdr-estate: no adapter for agent kind '{name}', starting with no mapped flags"
            );
            vec![name.clone()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn campaign() -> Campaign {
        Campaign {
            address: "home/garden".to_string(),
            estate: "home".to_string(),
            name: "garden".to_string(),
            dir: PathBuf::from("/tmp/garden"),
            workdir: PathBuf::from("/tmp/home"),
            kind: AgentKind::Claude,
            add_dirs: Vec::new(),
            prompt_files: Vec::new(),
            kind_flags: BTreeMap::new(),
            env: BTreeMap::new(),
            prepare: None,
        }
    }

    #[test]
    fn resume_uses_resume_flag() {
        let argv = claude_argv(&campaign(), Verb::Resume, "SID", None);
        let i = argv.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(argv[i + 1], "SID");
        assert!(!argv.contains(&"--session-id".to_string()));
    }

    #[test]
    fn fresh_uses_session_id_flag() {
        let argv = claude_argv(&campaign(), Verb::Fresh, "NEWSID", None);
        let i = argv.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(argv[i + 1], "NEWSID");
        assert!(!argv.contains(&"--resume".to_string()));
    }

    #[test]
    fn name_is_address() {
        let mut c = campaign();
        c.address = "home/taxes".to_string();
        let argv = claude_argv(&c, Verb::Fresh, "S", None);
        let i = argv.iter().position(|a| a == "--name").unwrap();
        assert_eq!(argv[i + 1], "home/taxes");
    }

    #[test]
    fn add_dir_per_path() {
        let mut c = campaign();
        c.add_dirs = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        let argv = claude_argv(&c, Verb::Fresh, "S", None);
        assert_eq!(argv.iter().filter(|a| *a == "--add-dir").count(), 2);
        assert!(argv.contains(&"/a".to_string()));
        assert!(argv.contains(&"/b".to_string()));
    }

    #[test]
    fn prompt_file_appended() {
        let argv = claude_argv(
            &campaign(),
            Verb::Fresh,
            "S",
            Some(Path::new("/tmp/x.prompt.md")),
        );
        let i = argv
            .iter()
            .position(|a| a == "--append-system-prompt-file")
            .unwrap();
        assert_eq!(argv[i + 1], "/tmp/x.prompt.md");
    }

    #[test]
    fn no_prompt_file_omitted() {
        let argv = claude_argv(&campaign(), Verb::Fresh, "S", None);
        assert!(!argv.contains(&"--append-system-prompt-file".to_string()));
    }

    #[test]
    fn kind_flag_true_is_bare_flag() {
        let mut c = campaign();
        c.kind_flags
            .insert("remote-control".to_string(), Scalar::Bool(true));
        let argv = claude_argv(&c, Verb::Fresh, "S", None);
        assert!(argv.contains(&"--remote-control".to_string()));
    }

    #[test]
    fn kind_flag_value_takes_argument() {
        let mut c = campaign();
        c.kind_flags.insert(
            "mcp-config".to_string(),
            Scalar::Str(".mcp.json".to_string()),
        );
        let argv = claude_argv(&c, Verb::Fresh, "S", None);
        let i = argv.iter().position(|a| a == "--mcp-config").unwrap();
        assert_eq!(argv[i + 1], ".mcp.json");
    }

    #[test]
    fn kind_flag_false_is_omitted() {
        let mut c = campaign();
        c.kind_flags
            .insert("remote-control".to_string(), Scalar::Bool(false));
        let argv = claude_argv(&c, Verb::Fresh, "S", None);
        assert!(!argv.contains(&"--remote-control".to_string()));
    }

    #[test]
    fn no_adapter_warns_and_returns_bare_kind() {
        let mut c = campaign();
        c.kind = AgentKind::Other("opencode".to_string());
        let argv = build_argv(&c, Verb::Fresh, "S", None);
        assert_eq!(argv, ["opencode"]);
    }

    #[test]
    fn claude_kind_uses_claude_adapter() {
        let c = campaign();
        let argv = build_argv(&c, Verb::Resume, "S", None);
        assert_eq!(argv[0], "claude");
    }

    // --- mutation-note gaps: argv accumulation and the false-flag continue ---

    #[test]
    fn every_argv_component_survives_together() {
        // Guards the "argv = [...]" (wipe) vs "argv += [...]" (accumulate)
        // mutation class: with every optional piece present at once,
        // nothing earlier in the vector may be dropped by something
        // built later.
        let mut c = campaign();
        c.address = "home/garden".to_string();
        c.add_dirs = vec![
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/c"),
        ];
        c.kind_flags
            .insert("remote-control".to_string(), Scalar::Bool(true));
        c.kind_flags.insert(
            "mcp-config".to_string(),
            Scalar::Str(".mcp.json".to_string()),
        );
        let argv = claude_argv(&c, Verb::Resume, "SID", Some(Path::new("/tmp/p.md")));
        assert_eq!(
            argv,
            vec![
                "claude",
                "--resume",
                "SID",
                "--name",
                "home/garden",
                "--append-system-prompt-file",
                "/tmp/p.md",
                "--add-dir",
                "/a",
                "--add-dir",
                "/b",
                "--add-dir",
                "/c",
                "--mcp-config",
                ".mcp.json",
                "--remote-control",
            ]
        );
    }

    #[test]
    fn a_false_flag_does_not_truncate_flags_that_sort_after_it() {
        // Guards the `continue` (skip this one) vs `break` (drop
        // everything after it) mutation class in the kind-flags loop:
        // "aaa" sorts before the false "bbb", "ccc" sorts after it, and
        // BTreeMap iterates in key order, so this exercises a false
        // value with real entries on both sides of it.
        let mut c = campaign();
        c.kind_flags.insert("aaa".to_string(), Scalar::Bool(true));
        c.kind_flags.insert("bbb".to_string(), Scalar::Bool(false));
        c.kind_flags
            .insert("ccc".to_string(), Scalar::Str("val".to_string()));
        let argv = claude_argv(&c, Verb::Fresh, "S", None);
        assert!(argv.contains(&"--aaa".to_string()));
        assert!(!argv.contains(&"--bbb".to_string()));
        let i = argv.iter().position(|a| a == "--ccc").unwrap();
        assert_eq!(argv[i + 1], "val");
    }
}
