//! A minimal, tolerant reader for campaign.md's YAML frontmatter.
//!
//! Not a YAML parser: it understands exactly the three shapes a campaign
//! file uses: a top-level scalar (`agent: claude`), a list (flow
//! `paths: [a, b]` or block `paths:` / `  - a`), and one level of nested
//! table for a per-kind flag block (`claude:` / `  remote-control: true`).
//! Unknown top-level keys are kept, not rejected, so an older or newer
//! campaign file, or a key some other tool reads, never breaks the parse.

use std::collections::BTreeMap;

/// A scalar leaf value: either a boolean literal (`true`/`false`) or, for
/// anything else, the string as written (comment-stripped, quote-stripped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scalar {
    Bool(bool),
    Str(String),
}

/// A frontmatter value: a bare scalar, a list of strings, or a one-level
/// table of scalars. An empty `key:` with nothing under it parses as an
/// empty table, and becomes a list only once a `- item` line under it
/// is seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Scalar(Scalar),
    List(Vec<String>),
    Table(BTreeMap<String, Scalar>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Scalar(Scalar::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            Value::List(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&BTreeMap<String, Scalar>> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Pending,
    List,
    Table,
}

/// Drop a trailing ` # comment` and surrounding matching quotes.
fn strip_value(v: &str) -> String {
    let t = comment_trim(v);
    strip_quotes(&t)
}

fn comment_trim(v: &str) -> String {
    v.split(" #").next().unwrap_or("").trim().to_string()
}

fn strip_quotes(v: &str) -> String {
    let chars: Vec<char> = v.chars().collect();
    if chars.len() >= 2 {
        let first = chars[0];
        if let Some(&last) = chars.last() {
            if first == last && (first == '"' || first == '\'') {
                return chars[1..chars.len() - 1].iter().collect();
            }
        }
    }
    v.to_string()
}

fn scalar(v: &str) -> Scalar {
    let s = strip_value(v);
    match s.as_str() {
        "true" => Scalar::Bool(true),
        "false" => Scalar::Bool(false),
        _ => Scalar::Str(s),
    }
}

/// A column-0 `key: rest` line. Requires the key to start with a letter
/// or underscore; returns the raw (un-stripped) remainder after the
/// colon and any immediately following whitespace.
fn match_top(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    if !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_') {
        return None;
    }
    let mut i = 1;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
    {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b':' {
        return None;
    }
    let key = &line[..i];
    let mut j = i + 1;
    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
        j += 1;
    }
    Some((key, &line[j..]))
}

/// An indented `- item` line. Requires at least one leading whitespace
/// char, a `-`, then at least one more whitespace char; returns the raw
/// remainder.
fn match_list_item(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i == 0 || i >= bytes.len() || bytes[i] != b'-' {
        return None;
    }
    i += 1;
    let ws_start = i;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i == ws_start {
        return None;
    }
    Some(&line[i..])
}

/// An indented `key: rest` line inside a table block. Same key alphabet
/// as `match_top` (word chars and `-`), but no restriction on the first
/// character.
fn match_table_item(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let key_start = i;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
    {
        i += 1;
    }
    if i == key_start || i >= bytes.len() || bytes[i] != b':' {
        return None;
    }
    let key = &line[key_start..i];
    let mut j = i + 1;
    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
        j += 1;
    }
    Some((key, &line[j..]))
}

pub fn parse_frontmatter(text: &str) -> BTreeMap<String, Value> {
    let mut data: BTreeMap<String, Value> = BTreeMap::new();
    let lines: Vec<&str> = text.lines().collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return data;
    }
    let mut key: Option<String> = None;
    let mut mode: Option<Mode> = None;
    for raw in &lines[1..] {
        if raw.trim() == "---" {
            break;
        }
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        let first_is_space = raw.chars().next().is_some_and(|c| c.is_whitespace());
        if !first_is_space {
            if let Some((k, rest_raw)) = match_top(raw) {
                let rest1 = comment_trim(rest_raw);
                if rest1.starts_with('[') && rest1.ends_with(']') {
                    let inner = rest1[1..rest1.len() - 1].trim();
                    let list: Vec<String> = if inner.is_empty() {
                        Vec::new()
                    } else {
                        inner.split(',').map(strip_value).collect()
                    };
                    data.insert(k.to_string(), Value::List(list));
                    mode = None;
                    key = Some(k.to_string());
                } else if !rest1.is_empty() {
                    data.insert(k.to_string(), Value::Scalar(scalar(&rest1)));
                    mode = None;
                    key = Some(k.to_string());
                } else {
                    data.insert(k.to_string(), Value::Table(BTreeMap::new()));
                    mode = Some(Mode::Pending);
                    key = Some(k.to_string());
                }
            }
            continue;
        }
        let Some(k) = key.clone() else { continue };
        if let Some(item_rest) = match_list_item(raw) {
            if mode == Some(Mode::Pending) {
                data.insert(k.clone(), Value::List(Vec::new()));
                mode = Some(Mode::List);
            }
            if mode == Some(Mode::List) {
                if let Some(Value::List(v)) = data.get_mut(&k) {
                    v.push(strip_value(item_rest));
                }
            }
            continue;
        }
        if let Some((tk, trest)) = match_table_item(raw) {
            if mode == Some(Mode::Pending) {
                data.insert(k.clone(), Value::Table(BTreeMap::new()));
                mode = Some(Mode::Table);
            }
            if mode == Some(Mode::Table) {
                if let Some(Value::Table(t)) = data.get_mut(&k) {
                    t.insert(tk.to_string(), scalar(trest));
                }
            }
        }
    }
    data
}

/// A campaign's one-line purpose, for the picker's brief column: the
/// first non-blank line after the frontmatter that is not a heading,
/// with `**` and backticks stripped. Empty when there is none, including
/// when the frontmatter never closes.
pub fn brief(text: &str) -> String {
    let mut lines = text.lines().peekable();
    if lines.next_if(|l| l.trim() == "---").is_some() {
        lines.find(|l| l.trim() == "---");
    }
    lines
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.replace("**", "").replace('`', ""))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &Value) -> &str {
        v.as_str().expect("expected a string scalar")
    }

    #[test]
    fn no_fence_is_empty() {
        assert!(parse_frontmatter("# garden\n\nbody text\n").is_empty());
    }

    #[test]
    fn scalar_value() {
        let d = parse_frontmatter("---\nagent: claude\n---\n");
        assert_eq!(s(&d["agent"]), "claude");
    }

    #[test]
    fn flow_list() {
        let d = parse_frontmatter("---\npaths: [~/code/a, ~/code/b]\n---\n");
        assert_eq!(d["paths"].as_list().unwrap(), ["~/code/a", "~/code/b"]);
    }

    #[test]
    fn empty_flow_list() {
        let d = parse_frontmatter("---\npaths: []\n---\n");
        assert_eq!(
            d["paths"].as_list().unwrap(),
            Vec::<String>::new().as_slice()
        );
    }

    #[test]
    fn block_list() {
        let d = parse_frontmatter("---\npaths:\n  - ~/code/a\n  - ~/code/b\n---\n");
        assert_eq!(d["paths"].as_list().unwrap(), ["~/code/a", "~/code/b"]);
    }

    #[test]
    fn inline_comment_stripped() {
        let d = parse_frontmatter("---\nagent: claude  # default kind\n---\n");
        assert_eq!(s(&d["agent"]), "claude");
    }

    #[test]
    fn quotes_stripped() {
        let d = parse_frontmatter("---\nagent: \"claude\"\n---\n");
        assert_eq!(s(&d["agent"]), "claude");
    }

    #[test]
    fn single_char_value_is_left_alone() {
        // Below strip_quotes's len>=2 guard, so it must not touch the
        // byte, let alone index past it.
        let d = parse_frontmatter("---\nagent: x\n---\n");
        assert_eq!(s(&d["agent"]), "x");
    }

    // --- match_top / match_list_item / match_table_item: line-end and
    // single-char-line boundaries, tested directly against the private
    // matchers so an off-by-one at the end of a line panics loudly in
    // the test rather than silently reading as "no match".

    #[test]
    fn match_top_colon_is_the_last_byte() {
        assert_eq!(match_top("key:"), Some(("key", "")));
    }

    #[test]
    fn match_top_no_colon_anywhere_is_none_not_a_panic() {
        assert_eq!(match_top("key"), None);
    }

    #[test]
    fn match_table_item_colon_is_the_last_byte() {
        assert_eq!(match_table_item("  key:"), Some(("key", "")));
    }

    #[test]
    fn match_table_item_no_colon_anywhere_is_none_not_a_panic() {
        assert_eq!(match_table_item("  key"), None);
    }

    #[test]
    fn match_list_item_dash_is_the_last_byte() {
        // One char of indent, a dash, and nothing after it: no
        // separator whitespace, so not a valid item.
        assert_eq!(match_list_item(" -"), None);
    }

    #[test]
    fn match_list_item_minimal_valid_item() {
        assert_eq!(match_list_item(" - x"), Some("x"));
    }

    #[test]
    fn frontmatter_text_ending_exactly_at_a_keys_colon() {
        // The whole document ends at the colon, not just the line: no
        // trailing newline, nothing after it at all.
        let d = parse_frontmatter("---\nworktree:");
        let t = d["worktree"].as_table().unwrap();
        assert!(t.is_empty());
    }

    #[test]
    fn kind_table() {
        let d = parse_frontmatter(
            "---\nclaude:\n  remote-control: true\n  mcp-config: .mcp.json\n---\n",
        );
        let t = d["claude"].as_table().unwrap();
        assert_eq!(t["remote-control"], Scalar::Bool(true));
        assert_eq!(t["mcp-config"], Scalar::Str(".mcp.json".to_string()));
    }

    #[test]
    fn worktree_table() {
        let d =
            parse_frontmatter("---\nworktree:\n  repo: ~/code/site\n  branch: wt/site-work\n---\n");
        let t = d["worktree"].as_table().unwrap();
        assert_eq!(t["repo"], Scalar::Str("~/code/site".to_string()));
        assert_eq!(t["branch"], Scalar::Str("wt/site-work".to_string()));
    }

    #[test]
    fn boolean_false() {
        let d = parse_frontmatter("---\nclaude:\n  remote-control: false\n---\n");
        assert_eq!(
            d["claude"].as_table().unwrap()["remote-control"],
            Scalar::Bool(false)
        );
    }

    #[test]
    fn unknown_keys_kept_not_rejected() {
        let d = parse_frontmatter(
            "---\nagent: claude\nnotes: something a future tool reads\npaths: []\n---\n",
        );
        assert_eq!(s(&d["agent"]), "claude");
        assert!(d.contains_key("notes"));
    }

    #[test]
    fn no_crash_on_malformed_line() {
        let d = parse_frontmatter("---\n: no key here\nagent: claude\n---\n");
        assert_eq!(d.get("agent").and_then(|v| v.as_str()), Some("claude"));
    }

    #[test]
    fn mixed_shape_under_one_key_after_first_choice_is_dropped() {
        // The first indented line under a key decides whether it becomes
        // a list or a table; a later line of the OTHER shape under the
        // same key is silently dropped rather than erroring, matching
        // the source grammar's tolerance for stray lines.
        let d = parse_frontmatter("---\nclaude:\n  remote-control: true\n  - stray\n---\n");
        let t = d["claude"].as_table().unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t["remote-control"], Scalar::Bool(true));
    }

    #[test]
    fn brief_is_the_first_body_line_that_is_not_a_heading() {
        let text =
            "---\nagent: claude\n---\n\n# garden\n\n## What\n\nGrow **the** `beds`.\nMore.\n";
        assert_eq!(brief(text), "Grow the beds.");
        assert_eq!(brief("Plain body.\n"), "Plain body.");
        assert_eq!(brief("---\n---\n# only a heading\n"), "");
        assert_eq!(brief("---\nagent: claude\nnever closed\n"), "");
    }
}
