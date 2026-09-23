//! What `create` accepts, and the agent name an address runs under. Pure:
//! no I/O, so the picker's core, the verb and the CLI share one set of
//! rules and one set of reasons, word for word.

/// The longest brief `create` takes, in characters.
pub const BRIEF_MAX: usize = 120;

/// The longest agent name herdr is given; `agent_name` cuts there.
const AGENT_NAME_MAX: usize = 32;

fn is_agent_name_byte(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'
}

pub fn agent_name(address: &str) -> String {
    let sanitized: String = address
        .to_lowercase()
        .chars()
        .map(|c| if is_agent_name_byte(c) { c } else { '-' })
        .collect();
    let trimmed = sanitized.trim_start_matches('-');
    let base = if trimmed.is_empty() {
        "estate".to_string()
    } else {
        trimmed.to_string()
    };
    let named = if base.chars().next().is_some_and(|c| c.is_alphabetic()) {
        base
    } else {
        format!("a{base}")
    };
    named.chars().take(AGENT_NAME_MAX).collect()
}

/// Why `estate/name` can't be created, or `None` when it can. The name
/// is kebab-case (`[a-z0-9]+(-[a-z0-9]+)*`), never `archive` (the
/// estates' archive directory), short enough that its agent name is
/// never cut, and neither an address in `known` nor one whose agent
/// name it would share.
pub fn refusal(estate: &str, name: &str, known: &[&str]) -> Option<String> {
    let kebab = name.split('-').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    });
    if !kebab {
        return Some("can't create: a-z 0-9 and single -".to_string());
    }
    if name == "archive" {
        return Some("'archive' is reserved".to_string());
    }
    let max = AGENT_NAME_MAX.saturating_sub(agent_name(&format!("{estate}/")).len());
    if name.len() > max {
        return Some(format!("name too long (max {max})"));
    }
    let address = format!("{estate}/{name}");
    let ours = agent_name(&address);
    known.iter().find(|k| agent_name(k) == ours).map(|clash| {
        if *clash == address {
            "already a campaign".to_string()
        } else {
            format!("agent name clashes with {clash}")
        }
    })
}

/// Why `brief` can't be a campaign's brief, or `None` when it can: it
/// needs text, at most `BRIEF_MAX` characters, on one line.
pub fn brief_refusal(brief: &str) -> Option<&'static str> {
    if brief.trim().is_empty() {
        Some("type a one-line brief first")
    } else if brief.chars().count() > BRIEF_MAX {
        Some("brief too long (max 120)")
    } else if brief.chars().any(char::is_control) {
        Some("the brief must be one line")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_name_sanitizes_and_lowercases() {
        assert_eq!(agent_name("Home/Garden"), "home-garden");
    }

    #[test]
    fn agent_name_truncates_to_32() {
        let name = agent_name(&format!("home/{}", "x".repeat(40)));
        assert!(name.chars().count() <= 32);
    }

    #[test]
    fn agent_name_leading_digit_gets_prefixed() {
        assert!(agent_name("9lives/x")
            .chars()
            .next()
            .unwrap()
            .is_alphabetic());
    }

    #[test]
    fn is_agent_name_byte_accepts_underscore() {
        assert!(is_agent_name_byte('_'));
    }

    #[test]
    fn is_agent_name_byte_accepts_hyphen() {
        assert!(is_agent_name_byte('-'));
    }

    #[test]
    fn is_agent_name_byte_rejects_a_slash() {
        assert!(!is_agent_name_byte('/'));
    }

    #[test]
    fn a_kebab_case_name_is_accepted() {
        for name in ["q", "quern", "a-1", "2026-plan"] {
            assert_eq!(refusal("home", name, &[]), None, "{name}");
        }
    }

    #[test]
    fn each_clause_of_the_name_rule_refuses_with_one_reason() {
        // Empty, a leading, trailing or double hyphen, and outside the
        // class: uppercase, '_', '.', a space and a second '/'.
        for name in ["", "-a", "a-", "a--b", "Ab", "a_b", "a.b", "a b", "a/b"] {
            assert_eq!(
                refusal("home", name, &[]).as_deref(),
                Some("can't create: a-z 0-9 and single -"),
                "{name:?}"
            );
        }
        assert_eq!(
            refusal("home", "archive", &[]).as_deref(),
            Some("'archive' is reserved")
        );
    }

    #[test]
    fn the_longest_name_leaves_the_agent_name_uncut() {
        // "dunn-" leaves 27 of the agent name's 32 characters, and
        // "nomograph-" 22.
        for (estate, max) in [("dunn", 27), ("nomograph", 22)] {
            assert_eq!(refusal(estate, &"x".repeat(max), &[]), None, "{estate}");
            assert_eq!(
                refusal(estate, &"x".repeat(max + 1), &[]),
                Some(format!("name too long (max {max})")),
                "{estate}"
            );
        }
    }

    #[test]
    fn an_existing_address_or_a_shared_agent_name_is_refused() {
        let known = ["home/garden", "home/foo.bar"];
        assert_eq!(
            refusal("home", "garden", &known).as_deref(),
            Some("already a campaign")
        );
        assert_eq!(
            refusal("home", "foo-bar", &known).as_deref(),
            Some("agent name clashes with home/foo.bar")
        );
        assert_eq!(refusal("home", "foo", &known), None);
    }

    #[test]
    fn a_brief_needs_one_line_of_at_most_120_characters() {
        assert_eq!(brief_refusal(&"b".repeat(120)), None);
        assert_eq!(
            brief_refusal(&"b".repeat(121)),
            Some("brief too long (max 120)")
        );
        assert_eq!(brief_refusal(" \t "), Some("type a one-line brief first"));
        assert_eq!(
            brief_refusal("one\ntwo"),
            Some("the brief must be one line")
        );
    }
}
