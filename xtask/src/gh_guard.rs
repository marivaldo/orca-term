//! gh guard lint: the GitHub CLI runs only from the `pr refresh` op (#87), the `pr create` op
//! (#132) and the `pr merge` op (#136), and only `pr create` and `worktree push` (#146) push,
//! never forced. gh is a program that speaks
//! to GitHub with the person's credentials, so it is named, as a program to find and run, in two
//! files alone, `src/ops/pr_refresh.rs` and `src/ops/pr_create.rs`; this lint rejects the string
//! literal `"gh"` in every other file under `src/`, and any `"gh"` or `'gh'` in the Neovim client
//! (`lua/`, `plugin/`, `build.lua`), which only ever asks the core. gh is found through
//! `pr_refresh::find_gh`, which only those three ops may call.
//!
//! A merge never deletes the branch (#136): `gh pr merge --delete-branch` fails while the worktree
//! has the branch checked out, and nothing happens to the worktree after a merge, as in Orca. So
//! `delete-branch` is refused in the code of every file scanned.
//!
//! A push sends the person's commits to the network the same way, so `git push` is started in one
//! function alone, `push_branch` in `src/adapters/git.rs`, the only file that may hold the literal
//! `"push"`, and only `src/ops/worktree_push.rs` (#146) may call it, in `send_to_origin`, which
//! `pr create` pushes through too: that one road refuses the repository's default branch for
//! every push (#195). Every read path (`pr compose`, `worktree ls`, ...) stays off the network.
//!
//! A push never forces (#146): the string literal `--force` is refused in every file scanned, so
//! git's force is never written (`branch -D` and `worktree remove -f` spell theirs out), and a
//! literal starting with `--force-with-lease` is refused outside the git adapter, the one place a
//! push with lease is built. The two files that name orca-term's own `worktree rm --force` and
//! `worktree push --force-with-lease`, which git never sees, are left out (`OWN_FLAGS`).
//!
//! A fetch reaches the network too (#141), so `git fetch` is started in one function alone,
//! `fetch_into` in `src/adapters/git.rs`, the only file that may hold the literal `"fetch"`, and
//! only `src/ops/worktree_rebase.rs` (`worktree rebase`) may call it.
//!
//! The check is textual, like the launch guard: comments are skipped, and only a literal that is
//! exactly `gh`, `push`, `fetch` or `--force` counts (`"gh auth login"` in a message is not a
//! program name, and `"pass --force to remove it"` names no force given to git).

use std::path::Path;

use anyhow::{Context, Result};

use crate::launch_guard::{collect_files, without_comment};

/// The files that may name gh: the ops that run it.
const GH_MODULES: &[&str] = &["src/ops/pr_refresh.rs", "src/ops/pr_create.rs"];

/// The same files, as the findings name them.
const GH_MODULES_TEXT: &str =
    "src/ops/pr_refresh.rs (`pr refresh`) and src/ops/pr_create.rs (`pr create`)";

/// The ops that may find gh, through `find_gh`, to run it.
const GH_OPS: &[&str] = &[
    "src/ops/pr_refresh.rs",
    "src/ops/pr_create.rs",
    "src/ops/pr_merge.rs",
];

/// The same ops, as the findings name them.
const GH_OPS_TEXT: &str = "src/ops/pr_refresh.rs (`pr refresh`), src/ops/pr_create.rs \
     (`pr create`) and src/ops/pr_merge.rs (`pr merge`)";

/// The call that finds gh.
const FIND_GH_CALL: &str = "find_gh(";

/// gh's flag that deletes the branch after a merge, without its dashes.
const DELETE_BRANCH: &str = "delete-branch";

/// The one file that may start `git push`: the git adapter, in `push_branch`.
const PUSH_ADAPTER: &str = "src/adapters/git.rs";

/// The one op that may call `push_branch`: `worktree push`, in `send_to_origin`, which refuses the
/// default branch and which `pr create` pushes through too (#195).
const PUSH_OP: &str = "src/ops/worktree_push.rs";

/// The same op, as the findings name it.
const PUSH_OP_TEXT: &str =
    "src/ops/worktree_push.rs (`send_to_origin`, which `worktree push` and `pr create` share)";

/// The files that name orca-term's own flags `worktree rm --force` and `worktree push
/// --force-with-lease`, which git never sees: the reason `worktree rm --json` prints, and the
/// client passing them to the core.
const OWN_FLAGS: &[&str] = &["src/output/json.rs", "lua/orca-term/core.lua"];

/// The call that pushes.
const PUSH_CALL: &str = "push_branch(";

/// The one op that may fetch, through `fetch_into`.
const FETCH_OP: &str = "src/ops/worktree_rebase.rs";

/// The call that fetches.
const FETCH_CALL: &str = "fetch_into(";

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
    let is_lua = Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"));
    let (marker, literals): (&str, &[&str]) = if is_lua {
        ("--", &["\"gh\"", "'gh'"])
    } else {
        ("//", &["\"gh\""])
    };
    problems.extend(check_force(path, text, marker));
    for (n, line) in text.lines().enumerate() {
        let code = without_comment(line, marker);
        if !GH_MODULES.contains(&path) && literals.iter().any(|literal| code.contains(literal)) {
            problems.push(format!(
                "{path}:{}: names the program gh; only {GH_MODULES_TEXT} may run it",
                n + 1
            ));
        }
        if !GH_OPS.contains(&path) && code.contains(FIND_GH_CALL) {
            problems.push(format!(
                "{path}:{}: finds gh to run it; only {GH_OPS_TEXT} may",
                n + 1
            ));
        }
        if code.contains(DELETE_BRANCH) {
            problems.push(format!(
                "{path}:{}: names --delete-branch; a merge never deletes the worktree's branch",
                n + 1
            ));
        }
    }
    problems
}

/// The findings about forcing in the file at `path` holding `text`, each line's code read with
/// `marker` starting its comment: a string literal that is exactly `--force`, and one that starts
/// with `--force-with-lease` outside the git adapter, in either quote. The files naming
/// orca-term's own flags ([`OWN_FLAGS`]) may hold both.
fn check_force(path: &str, text: &str, marker: &str) -> Vec<String> {
    if OWN_FLAGS.contains(&path) {
        return Vec::new();
    }
    let mut problems = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let code = without_comment(line, marker);
        if ["\"--force\"", "'--force'"]
            .iter()
            .any(|literal| code.contains(literal))
        {
            problems.push(format!(
                "{path}:{}: names --force; nothing is forced but a push with lease, in \
                 {PUSH_ADAPTER}",
                n + 1
            ));
        }
        let names_lease = ["\"--force-with-lease", "'--force-with-lease"]
            .iter()
            .any(|literal| code.contains(literal));
        if names_lease && path != PUSH_ADAPTER {
            problems.push(format!(
                "{path}:{}: names --force-with-lease; only {PUSH_ADAPTER} may push with a lease",
                n + 1
            ));
        }
    }
    problems
}

/// The findings about the network in the Rust file at `path` holding `text`: the literal
/// `"push"` or `"fetch"` outside the git adapter, a call of `push_branch` outside the adapter and
/// `worktree push`, and a call of `fetch_into` outside the adapter and `worktree rebase`.
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
                "{path}:{}: pushes a branch; only {PUSH_OP_TEXT} may push",
                n + 1
            ));
        }
        if code.contains("\"fetch\"") {
            problems.push(format!(
                "{path}:{}: names git's fetch; only {PUSH_ADAPTER} may start it",
                n + 1
            ));
        }
        if code.contains(FETCH_CALL) && path != FETCH_OP {
            problems.push(format!(
                "{path}:{}: fetches from origin; only {FETCH_OP} (`worktree rebase`) may fetch",
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
    fn only_the_pr_refresh_pr_create_and_pr_merge_ops_find_gh() {
        for op in GH_OPS {
            let finds = "let gh = pr_refresh::find_gh();\n";
            assert_eq!(check_file(op, finds), Vec::<String>::new());
        }
        assert_eq!(
            check_file(
                "src/ops/worktree_rm.rs",
                "if let Some(gh) = pr_refresh::find_gh() {\n"
            ),
            [
                "src/ops/worktree_rm.rs:1: finds gh to run it; only src/ops/pr_refresh.rs (`pr \
                 refresh`), src/ops/pr_create.rs (`pr create`) and src/ops/pr_merge.rs (`pr \
                 merge`) may"
            ]
        );
        let comment = "// find_gh() in a comment\n";
        assert_eq!(
            check_file("src/ops/worktree_rm.rs", comment),
            Vec::<String>::new()
        );
    }

    #[test]
    fn delete_branch_is_refused_in_every_file_even_the_merge_op() {
        let rust = "args.push(\"--delete-branch\".to_owned());\n";
        assert_eq!(
            check_file("src/ops/pr_merge.rs", rust),
            [
                "src/ops/pr_merge.rs:1: names --delete-branch; a merge never deletes the \
                 worktree's branch"
            ]
        );
        assert_eq!(
            check_file("src/domain/pr/merge.rs", "\"delete-branch\",\n").len(),
            1
        );
        assert_eq!(
            check_file(
                "lua/orca-term/core.lua",
                "local args = { '--delete-branch' }\n"
            )
            .len(),
            1
        );
        let none = Vec::<String>::new();
        let comments = "//! never `--delete-branch`\n// delete-branch\n";
        assert_eq!(check_file("src/domain/pr/merge.rs", comments), none);
        let lua = "-- no --delete-branch\n";
        assert_eq!(check_file("lua/orca-term/actions.lua", lua), none);
    }

    #[test]
    fn only_the_git_adapter_names_push_and_only_worktree_push_calls_it() {
        let adapter = "invoke(worktree, &[\"push\", \"-u\", \"origin\", branch])\n";
        assert!(check_file(PUSH_ADAPTER, adapter).is_empty());
        let pushes = "git::push_branch(&worktree.path, branch, mode)?\n";
        assert_eq!(check_file(PUSH_OP, pushes), Vec::<String>::new());
        let refused = |path: &str| {
            vec![format!(
                "{path}:1: pushes a branch; only src/ops/worktree_push.rs (`send_to_origin`, \
                 which `worktree push` and `pr create` share) may push"
            )]
        };
        // `pr create` pushes through `worktree_push::send_to_origin`, which refuses the default
        // branch (#195), never through the adapter itself.
        assert_eq!(
            check_file("src/ops/pr_create.rs", pushes),
            refused("src/ops/pr_create.rs")
        );
        assert_eq!(
            check_file(
                "src/ops/pr_compose.rs",
                "git::push_branch(&path, &branch)?;\n"
            ),
            refused("src/ops/pr_compose.rs")
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
    fn only_the_git_adapter_names_fetch_and_only_worktree_rebase_calls_it() {
        let adapter = ".args([\"fetch\", \"--quiet\", \"--no-tags\", \"origin\", refspec])\n";
        assert_eq!(check_file(PUSH_ADAPTER, adapter), Vec::<String>::new());
        let fetches = "git::fetch_into(&worktree.path, &refspec)?\n";
        assert_eq!(check_file(FETCH_OP, fetches), Vec::<String>::new());
        assert_eq!(
            check_file("src/ops/worktree_ls.rs", fetches),
            [
                "src/ops/worktree_ls.rs:1: fetches from origin; only src/ops/worktree_rebase.rs \
                 (`worktree rebase`) may fetch"
            ]
        );
        assert_eq!(
            check_file("src/ops/pr_refresh.rs", "git(&[\"fetch\", \"origin\"])\n"),
            ["src/ops/pr_refresh.rs:1: names git's fetch; only src/adapters/git.rs may start it"]
        );
        let comment = "// \"fetch\" in a comment\n";
        assert_eq!(check_file(FETCH_OP, comment), Vec::<String>::new());
    }

    #[test]
    fn the_literal_force_is_refused_everywhere_even_in_the_git_adapter() {
        assert_eq!(
            check_file(PUSH_ADAPTER, "args.push(OsStr::new(\"--force\"));\n"),
            [
                "src/adapters/git.rs:1: names --force; nothing is forced but a push with lease, \
                 in src/adapters/git.rs"
            ]
        );
        assert_eq!(
            check_file("src/ops/worktree_push.rs", "let force = \"--force\";\n").len(),
            1
        );
        assert_eq!(
            check_file("lua/orca-term/actions.lua", "local a = { '--force' }\n").len(),
            1
        );
        assert_eq!(check_file("build.lua", "run({ \"--force\" })\n").len(), 1);
        let none = Vec::<String>::new();
        // A message naming orca-term's flag, a comment, git's short spelling and branch -D pass.
        let fine = "let why = \"pass --force to remove it\";\n// \"--force\" in a comment\n\
                    args.push(OsStr::new(\"-f\"));\noutput(&path, &[\"branch\", \"-D\", b])\n";
        assert_eq!(check_file(PUSH_ADAPTER, fine), none);
        // The files naming orca-term's own `worktree rm --force`.
        let reason = "(Some(branch), \"deleted\", Some(\"--force\".to_owned()))\n";
        assert_eq!(check_file("src/output/json.rs", reason), none);
        let lua = "table.insert(args, '--force')\n";
        assert_eq!(check_file("lua/orca-term/core.lua", lua), none);
    }

    #[test]
    fn only_the_git_adapter_names_the_lease() {
        let none = Vec::<String>::new();
        let adapter = "args.push(format!(\"--force-with-lease={branch}:{expected}\"));\n";
        assert_eq!(check_file(PUSH_ADAPTER, adapter), none);
        assert_eq!(
            check_file("src/ops/worktree_push.rs", adapter),
            [
                "src/ops/worktree_push.rs:1: names --force-with-lease; only src/adapters/git.rs \
                 may push with a lease"
            ]
        );
        assert_eq!(
            check_file(
                "lua/orca-term/actions.lua",
                "local a = { '--force-with-lease' }\n"
            )
            .len(),
            1
        );
        // A message naming orca-term's flag, and the client passing it to the core, pass.
        let message = "\"`orca-term worktree push {name} --force-with-lease` would replace it\"\n";
        assert_eq!(check_file("src/domain/error.rs", message), none);
        let lua = "table.insert(args, '--force-with-lease')\n";
        assert_eq!(check_file("lua/orca-term/core.lua", lua), none);
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
