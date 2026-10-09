//! PR-title lint: `#<issue> - <description>`, the issue number standing where the general
//! convention puts a Jira id, with a lowercase description and no type prefix.

/// Returns every rule the title breaks.
pub fn lint(title: &str) -> Vec<String> {
    let title = title.trim();
    let Some((id, desc)) = title.split_once(" - ") else {
        return vec![format!(
            "the title must read `#<issue> - <description>`, e.g. `#28 - create and remove \
             worktrees`; got `{title}`"
        )];
    };
    let mut problems = Vec::new();
    let is_issue = id
        .strip_prefix('#')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if !is_issue {
        problems.push(format!(
            "`{id}` is not an issue reference: the title starts with `#<number> - `"
        ));
    }
    let desc = desc.trim();
    if desc.is_empty() {
        problems.push("the description after ` - ` is empty".to_owned());
    } else if desc.starts_with(|c: char| c.is_uppercase()) {
        problems.push(format!(
            "the description must start in lowercase; got `{desc}`"
        ));
    }
    if has_type_prefix(desc) {
        problems.push(
            "the description must carry no type prefix (`feat:`, `fix:`...); commits do".to_owned(),
        );
    }
    problems
}

fn has_type_prefix(desc: &str) -> bool {
    desc.split_once(':').is_some_and(|(head, _)| {
        let head = head.strip_suffix('!').unwrap_or(head);
        let ty = head.split_once('(').map_or(head, |(ty, _)| ty);
        !ty.is_empty() && !ty.contains(' ') && ty.chars().all(|c| c.is_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_issue_and_a_lowercase_description() {
        assert!(lint("#28 - create and remove worktrees").is_empty());
        assert!(lint("#7 - rename the unit to worktree").is_empty());
        assert!(lint("#27 - add `just check` and list worktrees").is_empty());
    }

    #[test]
    fn rejects_a_missing_or_malformed_issue() {
        assert_eq!(lint("create and remove worktrees").len(), 1);
        assert_eq!(lint("28 - create").len(), 1);
        assert_eq!(lint("CHECK-3043 - create").len(), 1);
        assert_eq!(lint("# - create").len(), 1);
    }

    #[test]
    fn rejects_an_uppercase_description_or_a_type_prefix() {
        assert_eq!(lint("#28 - Create worktrees").len(), 1);
        assert_eq!(lint("#28 - feat: create worktrees").len(), 1);
        assert_eq!(lint("#28 - fix(cli)!: refuse").len(), 1);
    }

    #[test]
    fn keeps_colons_that_are_not_a_type_prefix() {
        assert!(lint("#28 - refuse paths like a:b in names").is_empty());
    }
}
