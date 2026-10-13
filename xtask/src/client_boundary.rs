//! Client boundary lint: in the Neovim client, only `lua/orca-term/core.lua` starts processes
//! (`ARCHITECTURE.md`, "Neovim client"). Every other Lua file under `lua/` and `plugin/` reaches
//! the core through that module.
//!
//! The check is textual, like the layers lint: `--` comments are skipped, and each line is
//! searched for the calls in [`PROCESS_CALLS`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The client's directories, relative to the repository root.
const CLIENT_DIRS: &[&str] = &["lua", "plugin"];

/// The one file allowed to start processes.
const CORE_MODULE: &str = "lua/orca-term/core.lua";

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
    for dir in CLIENT_DIRS {
        let dir = root.join(dir);
        if dir.is_dir() {
            collect_lua_files(&dir, &mut files)?;
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

fn collect_lua_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            collect_lua_files(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "lua") {
            files.push(path);
        }
    }
    Ok(())
}

/// The findings for the file at `path` (such as `lua/orca-term/sidebar.lua`) holding `text`, each
/// as `path:line: message`.
fn check_file(path: &str, text: &str) -> Vec<String> {
    if path == CORE_MODULE {
        return Vec::new();
    }
    let mut problems = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let code = line.find("--").map_or(line, |at| &line[..at]);
        for call in PROCESS_CALLS.iter().filter(|call| calls(code, call)) {
            problems.push(format!(
                "{path}:{}: `{call}` starts a process; only {CORE_MODULE} may start one",
                n + 1
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
    fn the_core_module_may_start_processes() {
        let text = "local r = vim.system({ 'orca-term' }):wait()\n";
        assert!(check_file("lua/orca-term/core.lua", text).is_empty());
    }

    #[test]
    fn any_other_client_file_is_refused_with_its_line() {
        let text = "local M = {}\n\nvim.fn.jobstart({ 'git', 'status' })\n";
        assert_eq!(
            check_file("lua/orca-term/sidebar.lua", text),
            [
                "lua/orca-term/sidebar.lua:3: `vim.fn.jobstart` starts a process; only lua/orca-term/core.lua may start one"
            ]
        );
    }

    #[test]
    fn every_way_to_start_a_process_is_refused() {
        let text = "vim.system({})\nvim.fn.systemlist('ls')\nvim.fn.termopen('sh')\n\
                    local uv = vim.uv\nuv.spawn('sh', {})\nvim.loop.spawn('sh', {})\n\
                    io.popen('ls')\nos.execute('ls')\n";
        let problems = check_file("plugin/orca-term.lua", text);
        let found: Vec<&str> = problems
            .iter()
            .map(|p| p.split('`').nth(1).unwrap_or_default())
            .collect();
        assert_eq!(
            found,
            [
                "vim.system",
                "vim.fn.system",
                "vim.fn.termopen",
                "uv.spawn",
                "loop.spawn",
                "io.popen",
                "os.execute"
            ]
        );
    }

    #[test]
    fn comments_and_longer_names_are_not_calls() {
        let text =
            "-- only core.lua calls vim.system\nlocal x = myos.execute\nlocal y = 1 -- io.popen\n";
        assert!(check_file("lua/orca-term/init.lua", text).is_empty());
    }
}
