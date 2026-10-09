//! Commit-message lint: a single line, a type prefix, no Jira id, no AI attribution.

const TYPES: &[&str] = &[
    "feat", "fix", "docs", "chore", "refactor", "test", "ci", "build", "perf", "style", "revert",
];

/// Uppercase prefixes of tokens shaped like a Jira id that are not one.
const NOT_JIRA: &[&str] = &["UTF", "SHA", "ISO", "RFC", "CVE", "GHSA"];

const ATTRIBUTION: &[&str] = &[
    "co-authored-by",
    "generated with claude",
    "generated with [claude",
];

/// Drops git's comment lines and everything below the scissors line, as git does.
pub fn strip_git_comments(raw: &str) -> String {
    raw.lines()
        .take_while(|l| !l.starts_with("# ------------------------ >8 ------------------------"))
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Returns every rule the message breaks.
pub fn lint(message: &str) -> Vec<String> {
    let message = message.trim_end();
    let mut problems = Vec::new();
    if message.trim().is_empty() {
        return vec!["the message is empty".to_owned()];
    }
    if message.lines().count() > 1 {
        problems.push("the message must be a single line, with no body or trailers".to_owned());
    }
    let subject = message.lines().next().unwrap_or_default();
    if !has_type_prefix(subject) {
        problems.push(format!(
            "the subject must start with `<type>: `, where type is one of {}",
            TYPES.join(", ")
        ));
    }
    if let Some(id) = jira_id(message) {
        problems.push(format!(
            "`{id}` looks like a Jira id, which belongs in the PR title"
        ));
    }
    let lower = message.to_lowercase();
    if ATTRIBUTION.iter().any(|a| lower.contains(a)) || message.contains('🤖') {
        problems.push("the message must carry no AI attribution".to_owned());
    }
    problems
}

fn has_type_prefix(subject: &str) -> bool {
    let Some((head, desc)) = subject.split_once(": ") else {
        return false;
    };
    let head = head.strip_suffix('!').unwrap_or(head);
    let ty = match head.split_once('(') {
        Some((ty, scope)) if scope.ends_with(')') && scope.len() > 1 => ty,
        Some(_) => return false,
        None => head,
    };
    TYPES.contains(&ty) && !desc.trim().is_empty() && !desc.starts_with(' ')
}

fn jira_id(message: &str) -> Option<&str> {
    message
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find(|token| {
            let Some((key, num)) = token.split_once('-') else {
                return false;
            };
            (2..=10).contains(&key.len())
                && key.starts_with(|c: char| c.is_ascii_uppercase())
                && key
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && !num.is_empty()
                && num.chars().all(|c| c.is_ascii_digit())
                && !NOT_JIRA.contains(&key)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_conventional_single_line() {
        assert!(lint("feat: list lanes with orca-term lane ls\n").is_empty());
        assert!(lint("fix(fleet): keep prunable worktrees").is_empty());
        assert!(lint("refactor!: rename the contract module").is_empty());
        assert!(lint("chore: commit .claude/settings.json guards").is_empty());
        assert!(lint("docs: note the UTF-8 and SHA-256 requirements").is_empty());
    }

    #[test]
    fn rejects_a_body() {
        assert_eq!(lint("feat: x\n\nmore detail").len(), 1);
    }

    #[test]
    fn rejects_a_missing_or_unknown_type() {
        assert_eq!(lint("list lanes").len(), 1);
        assert_eq!(lint("feature: list lanes").len(), 1);
        assert_eq!(lint("feat:list lanes").len(), 1);
        assert_eq!(lint("feat: ").len(), 1);
    }

    #[test]
    fn rejects_a_jira_id() {
        assert_eq!(
            lint("fix: CHECK-3043 allow staging deploy"),
            vec!["`CHECK-3043` looks like a Jira id, which belongs in the PR title".to_owned()]
        );
    }

    #[test]
    fn rejects_ai_attribution() {
        let msg = "feat: x\n\nCo-Authored-By: Claude <noreply@anthropic.com>";
        assert!(lint(msg).iter().any(|p| p.contains("AI attribution")));
        assert_eq!(
            lint("feat: x 🤖 Generated with [Claude Code](https://claude.com)").len(),
            1
        );
    }

    #[test]
    fn strips_comments_and_the_scissors() {
        let raw = "feat: x\n# Please enter the message\n# ------------------------ >8 ------------------------\ndiff --git";
        assert_eq!(strip_git_comments(raw), "feat: x");
    }
}
