//! gh guard lint: the GitHub CLI runs only from the `pr refresh` op (#87) and the `pr create` op
//! (#132), and only `pr create` pushes. gh is a program that speaks to GitHub with the person's
//! credentials, so it is named, as a program to find and run, in those two files alone,
//! `src/ops/pr_refresh.rs` and `src/ops/pr_create.rs`; this lint rejects the string literal `"gh"`
//! in every other file under `src/`, and any `"gh"` or `'gh'` in the Neovim client (`lua/`,
//! `plugin/`, `build.lua`), which only ever asks the core.
//!
//! A push sends the person's commits to the network the same way, so `git push` is started in one
//! function alone, `push_branch` in `src/adapters/git.rs`, the only file that may hold the literal
//! `"push"`, and only `src/ops/pr_create.rs` may call it. Every read path (`pr compose`,
//! `worktree ls`, ...) stays off the network.
//!
//! The check is textual, like the launch guard: comments are skipped, and only a literal that is
//! exactly `gh` or `push` counts (`"gh auth login"` in a message is not a program name).

use std::path::Path;

use anyhow::{Context, Result};

use crate::launch_guard::{collect_files, without_comment};

/// The files that may name gh: the ops that run it.
const GH_MODULES: &[&str] = &["src/ops/pr_refresh.rs", "src/ops/pr_create.rs"];

/// The same files, as the findings name them.
const GH_MODULES_TEXT: &str =
    "src/ops/pr_refresh.rs (`pr refresh`) and src/ops/pr_create.rs (`pr create`)";

/// The one file that may start `git push`: the git adapter, in `push_branch`.
const PUSH_ADAPTER: &str = "src/adapters/git.rs";

/// The one op that may push, through `push_branch`.
const PUSH_OP: &str = "src/ops/pr_create.rs";

/// The call that pushes.
const PUSH_CALL: &str = "push_branch(";

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
    let mut problems = check_push(path, text);
    if GH_MODULES.contains(&path) {
        return problems;
    }
    let is_lua = Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"));
    let (marker, literals): (&str, &[&str]) = if is_lua {
        ("--", &["\"gh\"", "'gh'"])
    } else {
        ("//", &["\"gh\""])
    };
    for (n, line) in text.lines().enumerate() {
        let code = without_comment(line, marker);
        if literals.iter().any(|literal| code.contains(literal)) {
            problems.push(format!(
                "{path}:{}: names the program gh; only {GH_MODULES_TEXT} may run it",
                n + 1
            ));
        }
    }
    problems
}

/// The findings about pushing in the Rust file at `path` holding `text`: the literal `"push"`
/// outside the git adapter, and a call of `push_branch` outside the adapter and `pr create`.
fn check_push(path: &str, text: &str) -> Vec<String> {
    let is_rust = Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"));
    if !is_rust || path == PUSH_ADAPTER {
        return Vec::new();
    }
    let mut problems = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let code = without_comment(line, "//");
        if code.contains("\"push\"") {
            problems.push(format!(
                "{path}:{}: names git's push; only {PUSH_ADAPTER} may start it",
                n + 1
            ));
        }
        if code.contains(PUSH_CALL) && path != PUSH_OP {
            problems.push(format!(
                "{path}:{}: pushes a branch; only {PUSH_OP} (`pr create`) may push",
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
    fn the_pr_refresh_and_pr_create_ops_may_name_gh() {
        for module in GH_MODULES {
            assert!(check_file(module, "const GH: &str = \"gh\";\n").is_empty());
        }
    }

    #[test]
    fn only_the_git_adapter_names_push_and_only_pr_create_calls_it() {
        let adapter = "invoke(worktree, &[\"push\", \"-u\", \"origin\", branch])\n";
        assert!(check_file(PUSH_ADAPTER, adapter).is_empty());
        assert!(check_file(PUSH_OP, "git::push_branch(&worktree.path, branch)?\n").is_empty());
        assert_eq!(
            check_file(
                "src/ops/pr_compose.rs",
                "git::push_branch(&path, &branch)?;\n"
            ),
            [
                "src/ops/pr_compose.rs:1: pushes a branch; only src/ops/pr_create.rs (`pr create`) \
                 may push"
            ]
        );
        assert_eq!(
            check_file(
                "src/adapters/program.rs",
                "Command::new(\"git\").arg(\"push\")\n"
            ),
            ["src/adapters/program.rs:1: names git's push; only src/adapters/git.rs may start it"]
        );
        assert!(check_file("src/ops/pr_create.rs", "// \"push\" in a comment\n").is_empty());
    }

    #[test]
    fn any_other_rust_file_naming_gh_is_refused_with_its_line() {
        let text = "use x;\nlet gh = fs::executable_in(&dirs, \"gh\");\n";
        assert_eq!(
            check_file("src/ops/worktree_ls.rs", text),
            [
                "src/ops/worktree_ls.rs:2: names the program gh; only src/ops/pr_refresh.rs \
                 (`pr refresh`) and src/ops/pr_create.rs (`pr create`) may run it"
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
