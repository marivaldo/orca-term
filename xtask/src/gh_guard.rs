//! gh guard lint: the GitHub CLI runs only from the `pr refresh` op (#87). gh is a program that
//! speaks to GitHub with the person's credentials, so it is named, as a program to find and run,
//! in one file alone, `src/ops/pr_refresh.rs`; this lint rejects the string literal `"gh"` in every
//! other file under `src/`, and any `"gh"` or `'gh'` in the Neovim client (`lua/`, `plugin/`,
//! `build.lua`), which only ever asks the core.
//!
//! The check is textual, like the launch guard: comments are skipped, and only a literal that is
//! exactly `gh` counts (`"gh auth login"` in a message is not a program name).

use std::path::Path;

use anyhow::{Context, Result};

use crate::launch_guard::{collect_files, without_comment};

/// The one file that may name gh: the op that runs it.
const GH_MODULE: &str = "src/ops/pr_refresh.rs";

/// The directories scanned, relative to the repository root, with the extension of their files.
const SCANNED: &[(&str, &str)] = &[("src", "rs"), ("lua", "lua"), ("plugin", "lua")];

/// The client's files outside those directories: the build hook a plugin manager runs.
const SCANNED_FILES: &[&str] = &["build.lua"];

pub(crate) fn lint_repository(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for (dir, extension) in SCANNED {
        let dir = root.join(dir);
        if dir.is_dir() {
            collect_files(&dir, extension, &mut files)?;
        }
    }
    for file in SCANNED_FILES {
        let file = root.join(file);
        if file.is_file() {
            files.push(file);
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

/// The findings for the file at `path` holding `text`, each as `path:line: message`.
fn check_file(path: &str, text: &str) -> Vec<String> {
    if path == GH_MODULE {
        return Vec::new();
    }
    let is_lua = Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"));
    let (marker, literals): (&str, &[&str]) = if is_lua {
        ("--", &["\"gh\"", "'gh'"])
    } else {
        ("//", &["\"gh\""])
    };
    let mut problems = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let code = without_comment(line, marker);
        if literals.iter().any(|literal| code.contains(literal)) {
            problems.push(format!(
                "{path}:{}: names the program gh; only {GH_MODULE} (`pr refresh`) may run it",
                n + 1
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pr_refresh_op_may_name_gh() {
        assert!(check_file(GH_MODULE, "const GH: &str = \"gh\";\n").is_empty());
    }

    #[test]
    fn any_other_rust_file_naming_gh_is_refused_with_its_line() {
        let text = "use x;\nlet gh = fs::executable_in(&dirs, \"gh\");\n";
        assert_eq!(
            check_file("src/ops/worktree_ls.rs", text),
            [
                "src/ops/worktree_ls.rs:2: names the program gh; only src/ops/pr_refresh.rs \
                 (`pr refresh`) may run it"
            ]
        );
        assert_eq!(
            check_file("src/adapters/program.rs", "Command::new(\"gh\")\n").len(),
            1
        );
    }

    #[test]
    fn the_client_may_not_name_gh_in_either_quote() {
        assert_eq!(
            check_file("lua/orca-term/core.lua", "vim.system({ 'gh', 'pr' })\n").len(),
            1
        );
        assert_eq!(
            check_file("plugin/orca-term.lua", "local p = \"gh\"\n").len(),
            1
        );
        assert_eq!(check_file("build.lua", "run({ 'gh' })\n").len(), 1);
    }

    #[test]
    fn comments_and_longer_strings_are_not_the_program() {
        let rust = "// \"gh\" runs only from pr refresh\nlet why = \"gh auth login\";\n";
        assert!(check_file("src/domain/pr.rs", rust).is_empty());
        let lua = "-- 'gh' is never run here\nlocal hint = 'run gh auth login'\n";
        assert!(check_file("lua/orca-term/pr.lua", lua).is_empty());
    }
}
