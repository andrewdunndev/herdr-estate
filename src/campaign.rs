use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{parse_address, DEFAULT_AGENT_KIND};
use crate::errors::EstateError;
use crate::frontmatter::{self, Scalar};
use crate::paths::expand;
use crate::worktree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Other(String),
}

impl AgentKind {
    pub fn from_name(s: &str) -> Self {
        match s {
            "claude" => AgentKind::Claude,
            other => AgentKind::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Other(s) => s,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Campaign {
    pub address: String,
    pub estate: String,
    pub name: String,
    pub dir: PathBuf,
    pub workdir: PathBuf,
    pub kind: AgentKind,
    pub add_dirs: Vec<PathBuf>,
    pub missing_add_dirs: Vec<PathBuf>,
    pub prompt_files: Vec<PathBuf>,
    pub kind_flags: BTreeMap<String, Scalar>,
    pub env: BTreeMap<String, String>,
    pub prepare: Option<Vec<String>>,
    pub worktree: Option<WorktreeSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeSpec {
    pub repo: String,
    pub branch: String,
}

fn rooted(root: &Path, p: &str) -> PathBuf {
    let ep = expand(p);
    if ep.is_absolute() {
        ep
    } else {
        root.join(ep)
    }
}

fn toml_str_array(row: &toml::Table, key: &str) -> Vec<String> {
    row.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn non_empty_strings(v: Vec<String>) -> Vec<String> {
    v.into_iter().filter(|s| !s.is_empty()).collect()
}

fn frontmatter_str_list(fm: &BTreeMap<String, frontmatter::Value>, key: &str) -> Vec<String> {
    fm.get(key)
        .and_then(|v| v.as_list())
        .map(|l| l.to_vec())
        .unwrap_or_default()
}

fn toml_to_scalar(v: &toml::Value) -> Scalar {
    match v {
        toml::Value::Boolean(b) => Scalar::Bool(*b),
        toml::Value::String(s) => Scalar::Str(s.clone()),
        toml::Value::Integer(i) => Scalar::Str(i.to_string()),
        toml::Value::Float(f) => Scalar::Str(f.to_string()),
        toml::Value::Datetime(d) => Scalar::Str(d.to_string()),
        toml::Value::Array(_) | toml::Value::Table(_) => Scalar::Str(String::new()),
    }
}

fn toml_env_value(v: &toml::Value) -> Option<String> {
    match v {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Boolean(b) => Some(b.to_string()),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Float(f) => Some(f.to_string()),
        toml::Value::Datetime(d) => Some(d.to_string()),
        toml::Value::Array(_) | toml::Value::Table(_) => None,
    }
}

fn scalar_non_empty_str(s: &Scalar) -> Option<&str> {
    match s {
        Scalar::Str(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

fn estate_prepare(
    row: &toml::Table,
    estate_name: &str,
) -> Result<Option<Vec<String>>, EstateError> {
    let Some(v) = row.get("prepare") else {
        return Ok(None);
    };
    let err = || {
        EstateError::Config(format!(
            "estate '{estate_name}' prepare: must be a non-empty array of non-empty strings"
        ))
    };
    let arr = v.as_array().ok_or_else(err)?;
    if arr.is_empty() {
        return Err(err());
    }
    let argv: Vec<String> = arr
        .iter()
        .map(|x| {
            x.as_str()
                .filter(|s| !s.is_empty())
                .map(|s| expand(s).to_string_lossy().into_owned())
                .ok_or_else(err)
        })
        .collect::<Result<_, _>>()?;
    Ok(Some(argv))
}

fn estate_dirs<'a>(
    estates: &'a toml::Table,
    estate_name: &str,
) -> Result<(&'a toml::Table, PathBuf, PathBuf), EstateError> {
    let row = estates
        .get(estate_name)
        .and_then(|v| v.as_table())
        .ok_or_else(|| {
            EstateError::Config(format!("no estate '{estate_name}' in the estate table"))
        })?;
    let root_str = row
        .get("root")
        .and_then(|v| v.as_str())
        .ok_or_else(|| EstateError::Config(format!("estate '{estate_name}' has no 'root'")))?;
    let root = expand(root_str);
    if !root.is_dir() {
        return Err(EstateError::Config(format!(
            "estate '{estate_name}' not cloned at {}",
            root.display()
        )));
    }
    let campaigns = root.join(
        row.get("campaigns")
            .and_then(|v| v.as_str())
            .unwrap_or(crate::config::DEFAULT_CAMPAIGNS_DIR),
    );
    Ok((row, root, campaigns))
}

pub fn campaigns_dir(estates: &toml::Table, estate_name: &str) -> Result<PathBuf, EstateError> {
    estate_dirs(estates, estate_name).map(|(_, _, campaigns)| campaigns)
}

pub fn resolve(estates: &toml::Table, address: &str) -> Result<Campaign, EstateError> {
    let (estate_name, campaign_name) = parse_address(address)?;
    let (row, root, campaigns) = estate_dirs(estates, &estate_name)?;
    let cdir = campaigns.join(&campaign_name);
    let md = cdir.join("campaign.md");
    if !md.is_file() {
        return Err(EstateError::Config(format!(
            "no campaign at {}. Create {} first",
            cdir.display(),
            md.display()
        )));
    }
    let text = std::fs::read_to_string(&md)
        .map_err(|e| EstateError::Config(format!("could not read {}: {e}", md.display())))?;
    let fm = frontmatter::parse_frontmatter(&text);

    let kind_str = fm
        .get("agent")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            row.get("agent")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| DEFAULT_AGENT_KIND.to_string());
    let kind = AgentKind::from_name(&kind_str);

    let estate_paths = non_empty_strings(toml_str_array(row, "paths"));
    let campaign_paths = non_empty_strings(frontmatter_str_list(&fm, "paths"));
    let mut add_dirs: Vec<PathBuf> = Vec::new();
    let mut missing_add_dirs: Vec<PathBuf> = Vec::new();
    for p in estate_paths.iter().chain(campaign_paths.iter()) {
        let rp = rooted(&root, p);
        if add_dirs.contains(&rp) || missing_add_dirs.contains(&rp) {
            continue;
        }
        if !rp.exists() {
            missing_add_dirs.push(rp);
            continue;
        }
        add_dirs.push(rp);
    }

    let prepare = estate_prepare(row, &estate_name)?;

    let prompt_files: Vec<PathBuf> = toml_str_array(row, "prompts")
        .iter()
        .map(|p| rooted(&root, p))
        .collect();

    let estate_flags: BTreeMap<String, Scalar> = row
        .get(kind.as_str())
        .and_then(|v| v.as_table())
        .map(|t| {
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_scalar(v)))
                .collect()
        })
        .unwrap_or_default();
    let campaign_flags: BTreeMap<String, Scalar> = fm
        .get(kind.as_str())
        .and_then(|v| v.as_table())
        .cloned()
        .unwrap_or_default();
    let mut kind_flags = estate_flags;
    kind_flags.extend(campaign_flags);

    let mut workdir = root.clone();
    let mut worktree_spec = None;
    if let Some(wt) = fm.get("worktree").and_then(|v| v.as_table()) {
        for key in ["repo", "branch"] {
            if !wt.contains_key(key) {
                return Err(EstateError::Config(format!(
                    "campaign '{address}': worktree table is missing '{key}'"
                )));
            }
        }
        let repo = wt.get("repo").and_then(scalar_non_empty_str);
        let branch = wt.get("branch").and_then(scalar_non_empty_str);
        if let (Some(repo), Some(branch)) = (repo, branch) {
            workdir = worktree::would_ensure_worktree(repo, branch)?;
            add_dirs.push(workdir.clone());
            worktree_spec = Some(WorktreeSpec {
                repo: repo.to_string(),
                branch: branch.to_string(),
            });
        }
    }

    let mut env: BTreeMap<String, String> = BTreeMap::new();
    if let Some(t) = row.get("env").and_then(|v| v.as_table()) {
        for (k, v) in t.iter() {
            let rendered = toml_env_value(v).ok_or_else(|| {
                EstateError::Config(format!(
                    "estate '{estate_name}' env.{k}: must be a string, number or boolean"
                ))
            })?;
            env.insert(k.clone(), rendered.replace("{address}", address));
        }
    }

    Ok(Campaign {
        address: address.to_string(),
        estate: estate_name,
        name: campaign_name,
        dir: cdir,
        workdir,
        kind,
        add_dirs,
        missing_add_dirs,
        prompt_files,
        kind_flags,
        env,
        prepare,
        worktree: worktree_spec,
    })
}

pub fn materialise(campaign: &Campaign) -> Result<Campaign, EstateError> {
    let mut materialised = campaign.clone();
    if let Some(spec) = &campaign.worktree {
        let made = worktree::ensure_worktree(&spec.repo, &spec.branch)?;
        for dir in materialised.add_dirs.iter_mut() {
            if *dir == campaign.workdir {
                *dir = made.clone();
            }
        }
        materialised.workdir = made;
    }
    Ok(materialised)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn make_estate(root: &Path, campaign: &str, frontmatter: &str) -> PathBuf {
        let cdir = root.join("campaigns").join(campaign);
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(cdir.join("campaign.md"), frontmatter).unwrap();
        cdir
    }

    fn estates_with(home_row: toml::Table) -> toml::Table {
        let mut estates = toml::Table::new();
        estates.insert("home".to_string(), toml::Value::Table(home_row));
        estates
    }

    fn row(root: &Path) -> toml::Table {
        let mut t = toml::Table::new();
        t.insert(
            "root".to_string(),
            toml::Value::String(root.to_string_lossy().to_string()),
        );
        t
    }

    #[test]
    fn basic_resolution() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        r.insert(
            "agent".to_string(),
            toml::Value::String("claude".to_string()),
        );
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.estate, "home");
        assert_eq!(c.name, "garden");
        assert_eq!(c.kind, AgentKind::Claude);
        assert_eq!(c.workdir, root);
        assert!(c.add_dirs.is_empty());
        assert!(c.kind_flags.is_empty());
    }

    #[test]
    fn unknown_estate_raises() {
        assert!(resolve(&toml::Table::new(), "nope/garden").is_err());
    }

    #[test]
    fn uncloned_root_raises() {
        let mut r = toml::Table::new();
        r.insert(
            "root".to_string(),
            toml::Value::String("/nonexistent-estate-root-xyz".to_string()),
        );
        let estates = estates_with(r);
        assert!(resolve(&estates, "home/garden").is_err());
    }

    #[test]
    fn missing_campaign_md_raises() {
        let d = TempDir::new();
        let root = d.path().join("home");
        std::fs::create_dir_all(root.join("campaigns")).unwrap();
        let estates = estates_with(row(&root));
        assert!(resolve(&estates, "home/garden").is_err());
    }

    #[test]
    fn campaign_overrides_agent() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\nagent: codex\n---\n");
        let mut r = row(&root);
        r.insert(
            "agent".to_string(),
            toml::Value::String("claude".to_string()),
        );
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.kind, AgentKind::Other("codex".to_string()));
    }

    #[test]
    fn empty_string_campaign_agent_falls_back_to_estate_default() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\nagent: \"\"\n---\n");
        let mut r = row(&root);
        r.insert(
            "agent".to_string(),
            toml::Value::String("codex".to_string()),
        );
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.kind, AgentKind::Other("codex".to_string()));
    }

    #[test]
    fn empty_string_estate_agent_falls_back_to_default_kind() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        r.insert("agent".to_string(), toml::Value::String(String::new()));
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.kind, AgentKind::Claude);
    }

    #[test]
    fn paths_expanded() {
        let d = TempDir::new();
        let root = d.path().join("home");
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        make_estate(
            &root,
            "garden",
            &format!("---\npaths: [{}, {}]\n---\n", a.display(), b.display()),
        );
        let estates = estates_with(row(&root));
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.add_dirs, [a, b]);
    }

    #[test]
    fn kind_flags_selected_by_active_kind_only() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(
            &root,
            "garden",
            "---\nclaude:\n  remote-control: true\ncodex:\n  some-flag: 1\n---\n",
        );
        let mut r = row(&root);
        r.insert(
            "agent".to_string(),
            toml::Value::String("claude".to_string()),
        );
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.kind_flags.len(), 1);
        assert_eq!(c.kind_flags["remote-control"], Scalar::Bool(true));
    }

    #[test]
    fn estate_env_carried_through() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        let mut env = toml::Table::new();
        env.insert("K".to_string(), toml::Value::String("V".to_string()));
        r.insert("env".to_string(), toml::Value::Table(env));
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.env["K"], "V");
    }

    #[test]
    fn estate_env_address_placeholder_names_the_campaign() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        let mut env = toml::Table::new();
        env.insert(
            "H".to_string(),
            toml::Value::String("X-Owner: {address}".to_string()),
        );
        r.insert("env".to_string(), toml::Value::Table(env));
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.env["H"], "X-Owner: home/garden");
    }

    #[test]
    fn estate_env_integer_value_renders_not_drops() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        let mut env = toml::Table::new();
        env.insert("PORT".to_string(), toml::Value::Integer(8080));
        r.insert("env".to_string(), toml::Value::Table(env));
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.env["PORT"], "8080");
    }

    #[test]
    fn estate_env_table_value_is_a_named_config_error() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        let mut env = toml::Table::new();
        env.insert("BAD".to_string(), toml::Value::Table(toml::Table::new()));
        r.insert("env".to_string(), toml::Value::Table(env));
        let estates = estates_with(r);
        let err = resolve(&estates, "home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Config(_)));
        assert!(err.to_string().contains("BAD"));
    }

    #[test]
    fn no_worktree_key_uses_estate_root() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let estates = estates_with(row(&root));
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.workdir, root);
    }

    #[test]
    fn estate_paths_precede_campaign_paths() {
        let d = TempDir::new();
        let root = d.path().join("home");
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        make_estate(
            &root,
            "garden",
            &format!("---\npaths: [{}]\n---\n", b.display()),
        );
        let mut r = row(&root);
        r.insert(
            "paths".to_string(),
            toml::Value::Array(vec![toml::Value::String(a.to_string_lossy().into_owned())]),
        );
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.add_dirs, [a, b]);
    }

    #[test]
    fn duplicate_estate_and_campaign_path_collapses() {
        let d = TempDir::new();
        let root = d.path().join("home");
        let b = d.path().join("b");
        std::fs::create_dir_all(&b).unwrap();
        let fake_home = d.path().join("fake-home");
        std::fs::create_dir_all(fake_home.join("code")).unwrap();
        make_estate(
            &root,
            "garden",
            &format!("---\npaths: [~/code, {}]\n---\n", b.display()),
        );
        let mut r = row(&root);
        r.insert(
            "paths".to_string(),
            toml::Value::Array(vec![toml::Value::String("~/code".to_string())]),
        );
        let estates = estates_with(r);
        let _g = crate::test_support::EnvGuard::set(&[("HOME", fake_home.to_str().unwrap())]);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.add_dirs, [fake_home.join("code"), b]);
    }

    #[test]
    fn estate_kind_default_appears_in_flags() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        let mut claude = toml::Table::new();
        claude.insert("remote-control".to_string(), toml::Value::Boolean(true));
        r.insert("claude".to_string(), toml::Value::Table(claude));
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.kind_flags["remote-control"], Scalar::Bool(true));
    }

    #[test]
    fn campaign_kind_flag_overrides_estate_default() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(
            &root,
            "garden",
            "---\nclaude:\n  mcp-config: campaign.json\n---\n",
        );
        let mut r = row(&root);
        let mut claude = toml::Table::new();
        claude.insert(
            "mcp-config".to_string(),
            toml::Value::String("estate.json".to_string()),
        );
        r.insert("claude".to_string(), toml::Value::Table(claude));
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(
            c.kind_flags["mcp-config"],
            Scalar::Str("campaign.json".to_string())
        );
    }

    #[test]
    fn campaign_false_turns_off_estate_kind_default() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(
            &root,
            "garden",
            "---\nclaude:\n  remote-control: false\n---\n",
        );
        let mut r = row(&root);
        let mut claude = toml::Table::new();
        claude.insert("remote-control".to_string(), toml::Value::Boolean(true));
        r.insert("claude".to_string(), toml::Value::Table(claude));
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.kind_flags["remote-control"], Scalar::Bool(false));
    }

    #[test]
    fn worktree_key_is_created_by_materialise_and_becomes_workdir() {
        let d = TempDir::new();
        let root = d.path().join("home");
        let repo_dir = d.path().join("repo");
        init_git_repo(&repo_dir);
        make_estate(
            &root,
            "garden",
            &format!(
                "---\nworktree:\n  repo: {}\n  branch: wt/site-work\n---\n",
                repo_dir.display()
            ),
        );
        let estates = estates_with(row(&root));
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().join("state").to_str().unwrap(),
        )]);
        let resolved = resolve(&estates, "home/garden").unwrap();
        assert!(!resolved.workdir.exists(), "resolve creates nothing");
        let c = materialise(&resolved).unwrap();
        assert!(c.workdir.is_dir());
        assert_ne!(c.workdir, root);
        assert!(c.add_dirs.contains(&c.workdir));
        assert_eq!(c.add_dirs.len(), resolved.add_dirs.len());
        assert_eq!(materialise(&resolved).unwrap().workdir, c.workdir);
    }

    #[test]
    fn materialise_without_a_worktree_changes_nothing() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(
            &root, "garden", "---
---
",
        );
        let estates = estates_with(row(&root));
        let resolved = resolve(&estates, "home/garden").unwrap();
        let c = materialise(&resolved).unwrap();
        assert_eq!(c.workdir, root);
        assert_eq!(c.add_dirs, resolved.add_dirs);
        assert_eq!(resolved.worktree, None);
    }

    #[test]
    fn worktree_table_missing_branch_is_a_config_error() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\nworktree:\n  repo: x\n---\n");
        let estates = estates_with(row(&root));
        let e = resolve(&estates, "home/garden").unwrap_err().to_string();
        assert!(e.contains("home/garden") && e.contains("'branch'"), "{e}");
    }

    #[test]
    fn worktree_table_missing_repo_is_a_config_error() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\nworktree:\n  branch: y\n---\n");
        let estates = estates_with(row(&root));
        let e = resolve(&estates, "home/garden").unwrap_err().to_string();
        assert!(e.contains("home/garden") && e.contains("'repo'"), "{e}");
    }

    #[test]
    fn empty_string_worktree_repo_is_treated_as_no_worktree_key() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(
            &root,
            "garden",
            "---\nworktree:\n  repo: \"\"\n  branch: wt/site-work\n---\n",
        );
        let estates = estates_with(row(&root));
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.workdir, root);
    }

    #[test]
    fn resolve_never_creates_the_worktree() {
        let d = TempDir::new();
        let root = d.path().join("home");
        let repo_dir = d.path().join("repo");
        init_git_repo(&repo_dir);
        make_estate(
            &root,
            "garden",
            &format!(
                "---\nworktree:\n  repo: {}\n  branch: wt/site-work\n---\n",
                repo_dir.display()
            ),
        );
        let estates = estates_with(row(&root));
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().join("state").to_str().unwrap(),
        )]);
        let c = resolve(&estates, "home/garden").unwrap();
        assert!(!c.workdir.exists());
        assert_eq!(
            c.worktree,
            Some(WorktreeSpec {
                repo: repo_dir.display().to_string(),
                branch: "wt/site-work".to_string(),
            })
        );
        let listing = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo_dir)
            .args(["worktree", "list"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&listing.stdout).lines().count(),
            1,
            "resolve must not add a worktree"
        );
    }

    #[test]
    fn relative_prompts_resolve_against_root_not_cwd() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        r.insert(
            "prompts".to_string(),
            toml::Value::Array(vec![toml::Value::String("prompts/base.md".to_string())]),
        );
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.prompt_files, [root.join("prompts").join("base.md")]);
    }

    #[test]
    fn absolute_prompts_pass_through() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        r.insert(
            "prompts".to_string(),
            toml::Value::Array(vec![toml::Value::String("/etc/base.md".to_string())]),
        );
        let estates = estates_with(r);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.prompt_files, [PathBuf::from("/etc/base.md")]);
    }

    #[test]
    fn relative_campaign_paths_resolve_against_root() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\npaths: [notes]\n---\n");
        std::fs::create_dir_all(root.join("notes")).unwrap();
        let estates = estates_with(row(&root));
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.add_dirs, [root.join("notes")]);
    }

    #[test]
    fn absolute_campaign_paths_pass_through() {
        let d = TempDir::new();
        let root = d.path().join("home");
        let a = d.path().join("a");
        std::fs::create_dir_all(&a).unwrap();
        make_estate(
            &root,
            "garden",
            &format!("---\npaths: [{}]\n---\n", a.display()),
        );
        let estates = estates_with(row(&root));
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.add_dirs, [a]);
    }

    #[test]
    fn absent_add_dir_is_set_aside_unannounced_present_one_is_kept() {
        let d = TempDir::new();
        let root = d.path().join("home");
        let present = d.path().join("present");
        std::fs::create_dir_all(&present).unwrap();
        let absent = d.path().join("absent");
        make_estate(
            &root,
            "garden",
            &format!(
                "---\npaths: [{}, {}]\n---\n",
                absent.display(),
                present.display()
            ),
        );
        let estates = estates_with(row(&root));
        let state_dir = d.path().join("state");
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            state_dir.to_str().unwrap(),
        )]);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.add_dirs, [present]);
        assert_eq!(c.missing_add_dirs, [absent]);
        assert!(crate::progress::logged_messages().is_empty());
    }

    #[test]
    fn no_prepare_key_is_none() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let estates = estates_with(row(&root));
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.prepare, None);
    }

    #[test]
    fn prepare_argv_expands_a_leading_tilde() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        r.insert(
            "prepare".to_string(),
            toml::Value::Array(vec![
                toml::Value::String("~/bin/prepare.sh".to_string()),
                toml::Value::String("--fast".to_string()),
            ]),
        );
        let estates = estates_with(r);
        let fake_home = d.path().join("fake-home");
        let _g = crate::test_support::EnvGuard::set(&[("HOME", fake_home.to_str().unwrap())]);
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(
            c.prepare,
            Some(vec![
                fake_home
                    .join("bin/prepare.sh")
                    .to_string_lossy()
                    .into_owned(),
                "--fast".to_string(),
            ])
        );
    }

    #[test]
    fn prepare_must_be_an_array() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        r.insert(
            "prepare".to_string(),
            toml::Value::String("bare".to_string()),
        );
        let estates = estates_with(r);
        let err = resolve(&estates, "home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Config(_)));
        assert!(err.to_string().contains("prepare"));
    }

    #[test]
    fn prepare_must_be_non_empty() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        r.insert("prepare".to_string(), toml::Value::Array(Vec::new()));
        let estates = estates_with(r);
        let err = resolve(&estates, "home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Config(_)));
    }

    #[test]
    fn prepare_element_must_be_a_string() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        r.insert(
            "prepare".to_string(),
            toml::Value::Array(vec![toml::Value::Integer(1)]),
        );
        let estates = estates_with(r);
        let err = resolve(&estates, "home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Config(_)));
    }

    #[test]
    fn prepare_element_must_be_non_empty() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let mut r = row(&root);
        r.insert(
            "prepare".to_string(),
            toml::Value::Array(vec![toml::Value::String(String::new())]),
        );
        let estates = estates_with(r);
        let err = resolve(&estates, "home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Config(_)));
    }

    #[test]
    fn estate_with_neither_paths_nor_kind_table_yields_empty_add_dirs_and_flags() {
        let d = TempDir::new();
        let root = d.path().join("home");
        make_estate(&root, "garden", "---\n---\n");
        let estates = estates_with(row(&root));
        let c = resolve(&estates, "home/garden").unwrap();
        assert_eq!(c.add_dirs, Vec::<PathBuf>::new());
        assert!(c.kind_flags.is_empty());
    }

    fn init_git_repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        run(&["add", "README.md"]);
        run(&["commit", "-m", "init"]);
    }
}
