//! Launch guard lint: no launch of an agent ever bypasses permissions (ADR 0007). The flags and
//! modes that would are listed in one place, `src/domain/launch.rs`, where the run-time guard
//! refuses them; this lint rejects their literals anywhere else under `src/`, `lua/` and
//! `plugin/`, so no other code can build a launch that carries them.
//!
//! The check is textual, like the other lints: comments are skipped, and a literal counts only as
//! a whole token (`disableBypassPermissionsMode` is not `bypassPermissions`; `--autocomplete` is
//! not `--auto`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The one file that may name the forbidden literals: the run-time guard.
const GUARD_MODULE: &str = "src/domain/launch.rs";

/// The literals no other file may contain.
const FORBIDDEN: &[&str] = &[
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "--auto",
    "--yolo",
    "bypassPermissions",
];

/// The directories scanned, relative to the repository root, with the extension of their files.
const SCANNED: &[(&str, &str)] = &[("src", "rs"), ("lua", "lua"), ("plugin", "lua")];

pub(crate) fn lint_repository(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for (dir, extension) in SCANNED {
        let dir = root.join(dir);
        if dir.is_dir() {
            collect_files(&dir, extension, &mut files)?;
        }
    }
    files.sort();
    let mut problems = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        let rel = file.strip_prefix(root).unwrap_or(&file);
        problems.extend(check_file(&rel.to_string_lossy(), &text));
    }
    Ok(problems)
}

pub(crate) fn collect_files(dir: &Path, extension: &str, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, extension, files)?;
        } else if path.extension().is_some_and(|ext| ext == extension) {
            files.push(path);
        }
    }
    Ok(())
}

/// The findings for the file at `path` holding `text`, each as `path:line: message`.
fn check_file(path: &str, text: &str) -> Vec<String> {
    if path == GUARD_MODULE {
        return Vec::new();
    }
    let is_lua = Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"));
    let marker = if is_lua { "--" } else { "//" };
    let mut problems = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let code = without_comment(line, marker);
        for literal in FORBIDDEN.iter().filter(|literal| has_token(code, literal)) {
            problems.push(format!(
                "{path}:{}: `{literal}` bypasses permissions; no launch may carry it, and only \
                 {GUARD_MODULE} may name it",
                n + 1
            ));
        }
    }
    problems
}

/// `line` up to its comment, which starts at the first `marker` outside a quoted string: a Lua
/// string holding `--auto` is code, not a comment.
pub(crate) fn without_comment<'a>(line: &'a str, marker: &str) -> &'a str {
    let mut quote: Option<char> = None;
    for (at, c) in line.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            None if c == '\'' || c == '"' => quote = Some(c),
            None if line[at..].starts_with(marker) => return &line[..at],
            Some(_) | None => {}
        }
    }
    line
}

/// Whether `code` holds `token` on its own: not run into a longer word on either side.
fn has_token(code: &str, token: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    code.match_indices(token).any(|(at, _)| {
        let before = code[..at].chars().next_back();
        let after = code[at + token.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_module_may_name_the_literals() {
        let text =
            "const FORBIDDEN: &[&str] = &[\"--dangerously-skip-permissions\", \"--auto\"];\n";
        assert!(check_file("src/domain/launch.rs", text).is_empty());
    }

    #[test]
    fn any_other_file_is_refused_with_its_line() {
        let text = "let argv = vec![\"claude\",\n    \"--dangerously-skip-permissions\"];\n";
        assert_eq!(
            check_file("src/ops/agent_start.rs", text),
            [
                "src/ops/agent_start.rs:2: `--dangerously-skip-permissions` bypasses permissions; \
                 no launch may carry it, and only src/domain/launch.rs may name it"
            ]
        );
        let lua = "local args = { 'claude', '--auto' }\n";
        assert_eq!(check_file("lua/orca-term/core.lua", lua).len(), 1);
        let opencode = "let argv = [\"opencode\", \"--yolo\"];\n";
        assert_eq!(check_file("src/ops/agent_start.rs", opencode).len(), 1);
    }

    #[test]
    fn every_literal_is_refused_and_comments_are_not() {
        let text = "a(\"--allow-dangerously-skip-permissions\")\nb(\"bypassPermissions\")\n\
                    // --auto is fine in a comment\n";
        let problems = check_file("src/x.rs", text);
        let found: Vec<&str> = problems
            .iter()
            .map(|p| p.split('`').nth(1).unwrap_or_default())
            .collect();
        assert_eq!(
            found,
            ["--allow-dangerously-skip-permissions", "bypassPermissions"]
        );
    }

    #[test]
    fn a_comment_starts_outside_a_string() {
        assert_eq!(without_comment("x = 1 -- note", "--"), "x = 1 ");
        assert_eq!(
            without_comment("x = '--auto' -- note", "--"),
            "x = '--auto' "
        );
        assert_eq!(
            without_comment("let u = \"https://x\"; // c", "//"),
            "let u = \"https://x\"; "
        );
    }

    #[test]
    fn longer_tokens_containing_a_literal_are_not_that_literal() {
        let text = "\"disableBypassPermissionsMode\": \"disable\",\n\
                    \"disableBypassPermissions\"\nlet x = autoAllowBash;\n--autocomplete\n";
        assert!(check_file("src/domain/other.rs", text).is_empty());
        assert!(has_token("x --auto y", "--auto"));
        assert!(!has_token("x --auto-fix y", "--auto"));
        assert!(has_token("(\"bypassPermissions\")", "bypassPermissions"));
    }
}
