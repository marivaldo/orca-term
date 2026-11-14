//! Neovim Lua lint: the only Lua in the repository is the code the core sends to a Neovim over
//! its socket (`nvim_exec_lua`, #194), and that code needs nothing the person's Neovim may lack:
//!
//! - every `.lua` file lives in `src/domain/nvim/`, beside the Rust that sends it, so orca-term
//!   never grows a Neovim plugin again by accident (ADR 0014);
//! - none calls `require`: it loads no module, orca-term's or anyone's, so it runs in a Neovim
//!   with no plugin at all;
//! - none starts a process: the core runs git and hands the Neovim what it needs.
//!
//! The check is textual, like the other lints: `--` comments are skipped. Hidden directories
//! (`.git`, `.claude`, ...) and `target` are not searched.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The one directory Lua may live in, relative to the repository root.
const LUA_DIR: &str = "src/domain/nvim";

/// Directories never searched, besides the hidden ones.
const SKIPPED_DIRS: &[&str] = &["target"];

/// Calls that start a process. `vim.fn.system` also catches `vim.fn.systemlist`, and `uv.spawn`
/// catches `vim.uv.spawn` and a local alias named `uv`.
const PROCESS_CALLS: &[&str] = &[
    "vim.system",
    "vim.fn.system",
    "vim.fn.jobstart",
    "vim.fn.termopen",
    "uv.spawn",
    "loop.spawn",
    "io.popen",
    "os.execute",
];

pub(crate) fn lint_repository(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_lua_files(root, &mut files)?;
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

fn collect_lua_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            if !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_ref()) {
                collect_lua_files(&path, files)?;
            }
        } else if path.extension().is_some_and(|ext| ext == "lua") {
            files.push(path);
        }
    }
    Ok(())
}

/// The findings for the Lua file at `path` (relative to the root, such as
/// `src/domain/nvim/open_diff.lua`) holding `text`, each as `path[:line]: message`.
fn check_file(path: &str, text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let dir = Path::new(path).parent().unwrap_or(Path::new(""));
    if dir != Path::new(LUA_DIR) {
        problems.push(format!(
            "{path}: Lua lives only in {LUA_DIR}/, as code the core sends to Neovim; orca-term \
             ships no Neovim plugin (ADR 0014)"
        ));
    }
    for (n, line) in text.lines().enumerate() {
        let at = format!("{path}:{}", n + 1);
        let code = line.find("--").map_or(line, |at| &line[..at]);
        if calls(code, "require") {
            problems.push(format!(
                "{at}: `require` loads a module the person's Neovim may not have; the code the \
                 core sends uses Neovim's own API only"
            ));
        }
        for call in PROCESS_CALLS.iter().filter(|call| calls(code, call)) {
            problems.push(format!(
                "{at}: `{call}` starts a process; the core runs git and hands Neovim what it needs"
            ));
        }
    }
    problems
}

/// Whether `code` holds `call` as a name of its own, not the tail of a longer one
/// (`myos.execute` is not `os.execute`).
fn calls(code: &str, call: &str) -> bool {
    code.match_indices(call).any(|(at, _)| {
        let before = code[..at].chars().next_back();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_beside_the_rust_that_sends_it_using_neovims_api_passes() {
        let text = "local worktree = ...\nvim.cmd.diffthis() -- require is named in a comment\n";
        assert!(check_file("src/domain/nvim/open_diff.lua", text).is_empty());
    }

    #[test]
    fn lua_anywhere_else_is_a_plugin_and_is_refused() {
        for path in [
            "lua/orca-term/init.lua",
            "plugin/orca-term.lua",
            "build.lua",
            "src/domain/open_diff.lua",
            "src/domain/nvim/sub/x.lua",
        ] {
            let problems = check_file(path, "return 1\n");
            assert_eq!(problems.len(), 1, "{path}: {problems:?}");
            assert!(problems[0].starts_with(&format!("{path}: Lua lives only in")));
        }
    }

    #[test]
    fn requiring_a_module_is_refused() {
        let problems = check_file(
            "src/domain/nvim/open_diff.lua",
            "local x = 1\nreturn require('orca-term.actions').diff_from_core(...)\n",
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("src/domain/nvim/open_diff.lua:2: `require`"));
        assert!(check_file("src/domain/nvim/a.lua", "local my_require = 1\n").is_empty());
    }

    #[test]
    fn starting_a_process_is_refused() {
        let problems = check_file(
            "src/domain/nvim/a.lua",
            "vim.system({ 'git', 'show' })\nlocal x = myos.execute\nio.popen('ls')\n",
        );
        assert_eq!(
            problems,
            [
                "src/domain/nvim/a.lua:1: `vim.system` starts a process; the core runs git and \
                 hands Neovim what it needs",
                "src/domain/nvim/a.lua:3: `io.popen` starts a process; the core runs git and \
                 hands Neovim what it needs",
            ]
        );
    }

    #[test]
    fn the_repository_holds_only_the_cores_lua() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let problems = lint_repository(&root).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
    }
}
