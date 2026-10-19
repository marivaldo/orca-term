//! Client boundary lint: the Neovim client keeps each kind of work in one place
//! (`ARCHITECTURE.md`, "Neovim client"):
//!
//! - only `lua/orca-term/core.lua` starts processes; every other file reaches the core through it;
//! - only `lua/orca-term/sidebar.lua`, `lua/orca-term/tabs.lua`, `lua/orca-term/terminal.lua`,
//!   `lua/orca-term/review.lua` and `lua/orca-term/notes.lua` touch windows, tabs and buffers;
//! - `lua/orca-term/fleet.lua` and `lua/orca-term/render.lua` are pure: no `vim.api`, no `vim.cmd`.
//!
//! The check is textual, like the layers lint: `--` comments are skipped, and each line of each Lua
//! file under `lua/` and `plugin/` is searched for the calls in the lists below.

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

/// The files allowed to touch windows, tabs and buffers: the sidebar, the worktrees' tabs, the
/// agents' terminals, the review tabs and the notes drawn in them.
const WINDOW_MODULES: &[&str] = &[
    "lua/orca-term/sidebar.lua",
    "lua/orca-term/tabs.lua",
    "lua/orca-term/terminal.lua",
    "lua/orca-term/review.lua",
    "lua/orca-term/notes.lua",
];

/// The same files, as the findings name them.
const WINDOW_MODULES_TEXT: &str = "lua/orca-term/sidebar.lua, lua/orca-term/tabs.lua, \
     lua/orca-term/terminal.lua, lua/orca-term/review.lua and lua/orca-term/notes.lua";

/// Calls that open, change or read windows, tabs and buffers. A prefix such as `nvim_win_` stands
/// for every API function that starts with it (`nvim_set_current_` covers the window, the tab and
/// the buffer). A `vim.cmd` that splits or works tabs is caught apart, by [`splits_with_vim_cmd`]
/// and [`works_tabs_with_vim_cmd`].
const WINDOW_CALLS: &[&str] = &[
    "nvim_open_win",
    "nvim_win_",
    "nvim_buf_",
    "nvim_create_buf",
    "nvim_tabpage_",
    "nvim_set_current_",
    "nvim_list_wins",
    "nvim_list_tabpages",
    "nvim_list_bufs",
    "vim.bo",
    "vim.wo",
];

/// Ex commands that open, close or point tabs elsewhere.
const TAB_COMMANDS: &[&str] = &["tabnew", "tabedit", "tabclose", "tabonly", "tcd", "tchdir"];

/// The files that must stay pure: data in, data out.
const PURE_MODULES: &[&str] = &["lua/orca-term/fleet.lua", "lua/orca-term/render.lua"];

/// What a pure file may not use: the Neovim API and Ex commands.
const IMPURE_CALLS: &[&str] = &["vim.api", "vim.cmd"];

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
    let mut problems = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let at = format!("{path}:{}", n + 1);
        let code = line.find("--").map_or(line, |at| &line[..at]);
        if path != CORE_MODULE {
            for call in PROCESS_CALLS.iter().filter(|call| calls(code, call)) {
                problems.push(format!(
                    "{at}: `{call}` starts a process; only {CORE_MODULE} may start one"
                ));
            }
        }
        if !WINDOW_MODULES.contains(&path) {
            for call in WINDOW_CALLS.iter().filter(|call| calls(code, call)) {
                problems.push(format!(
                    "{at}: `{call}` touches windows or buffers; only {WINDOW_MODULES_TEXT} may"
                ));
            }
            if splits_with_vim_cmd(code) {
                problems.push(format!(
                    "{at}: `vim.cmd` opens a split; only {WINDOW_MODULES_TEXT} may open windows"
                ));
            }
            if works_tabs_with_vim_cmd(code) {
                problems.push(format!(
                    "{at}: `vim.cmd` opens, closes or moves a tab; only {WINDOW_MODULES_TEXT} may"
                ));
            }
        }
        if PURE_MODULES.contains(&path) {
            for call in IMPURE_CALLS.iter().filter(|call| calls(code, call)) {
                problems.push(format!(
                    "{at}: `{call}` is not allowed in a pure module; move it to lua/orca-term/sidebar.lua or lua/orca-term/tabs.lua"
                ));
            }
        }
    }
    problems
}

/// Whether `code` runs an Ex command that splits a window through `vim.cmd`, such as
/// `vim.cmd('topleft vsplit')` or `vim.cmd.vsplit()`.
fn splits_with_vim_cmd(code: &str) -> bool {
    calls(code, "vim.cmd") && code.contains("split")
}

/// Whether `code` runs an Ex command that opens, closes or points a tab elsewhere through
/// `vim.cmd`, such as `vim.cmd('$tabnew')`, `vim.cmd.tcd(dir)` or `vim.cmd('tabclose! 2')`.
fn works_tabs_with_vim_cmd(code: &str) -> bool {
    calls(code, "vim.cmd") && TAB_COMMANDS.iter().any(|command| has_word(code, command))
}

/// Whether `code` holds `word` with no letter, digit or `_` on either side (`tcd` is not in
/// `etcd`).
fn has_word(code: &str, word: &str) -> bool {
    let is_name = |c: char| c.is_alphanumeric() || c == '_';
    code.match_indices(word).any(|(at, _)| {
        let before = code[..at].chars().next_back();
        let after = code[at + word.len()..].chars().next();
        !before.is_some_and(is_name) && !after.is_some_and(is_name)
    })
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

    #[test]
    fn the_sidebar_tabs_terminal_review_and_notes_modules_may_touch_windows_tabs_and_buffers() {
        let text = "local win = vim.api.nvim_open_win(buf, false, {})\n\
                    vim.api.nvim_buf_set_lines(buf, 0, -1, false, {})\n\
                    vim.wo[win].number = false\nvim.cmd('topleft vsplit')\n\
                    vim.cmd('$tabnew')\nvim.cmd.tcd(dir)\nvim.cmd('tabclose! 2')\n\
                    vim.api.nvim_set_current_tabpage(tab)\n";
        for path in WINDOW_MODULES {
            assert!(check_file(path, text).is_empty(), "{path}");
        }
    }

    #[test]
    fn any_other_client_file_working_tabs_is_refused() {
        let text = "vim.cmd('$tabnew')\nvim.cmd.tcd(dir)\nvim.cmd('tabclose! ' .. n)\n\
                    vim.cmd.tabonly()\nvim.api.nvim_set_current_tabpage(tab)\n\
                    vim.api.nvim_list_tabpages()\nvim.api.nvim_set_current_win(win)\n";
        let problems = check_file("lua/orca-term/actions.lua", text);
        assert_eq!(problems.len(), 7, "{problems:#?}");
        assert_eq!(
            problems[0],
            "lua/orca-term/actions.lua:1: `vim.cmd` opens, closes or moves a tab; only lua/orca-term/sidebar.lua, lua/orca-term/tabs.lua, lua/orca-term/terminal.lua, lua/orca-term/review.lua and lua/orca-term/notes.lua may"
        );
        assert_eq!(
            problems[4],
            "lua/orca-term/actions.lua:5: `nvim_set_current_` touches windows or buffers; only lua/orca-term/sidebar.lua, lua/orca-term/tabs.lua, lua/orca-term/terminal.lua, lua/orca-term/review.lua and lua/orca-term/notes.lua may"
        );
    }

    #[test]
    fn a_tab_command_is_a_word_of_its_own() {
        assert!(works_tabs_with_vim_cmd("vim.cmd.tcd(path)"));
        assert!(!works_tabs_with_vim_cmd("vim.cmd('echo etcd')"));
        assert!(!works_tabs_with_vim_cmd("local tabnew_count = 1"));
    }

    #[test]
    fn any_other_client_file_touching_windows_or_buffers_is_refused() {
        let text = "vim.api.nvim_open_win(0, true, {})\nvim.api.nvim_win_close(0, true)\n\
                    vim.api.nvim_buf_set_lines(0, 0, -1, false, {})\nvim.api.nvim_create_buf(false, true)\n\
                    vim.api.nvim_tabpage_list_wins(0)\nvim.bo.modifiable = true\nvim.wo.wrap = false\n\
                    vim.cmd('topleft vsplit')\nvim.cmd.split()\n";
        let problems = check_file("plugin/orca-term.lua", text);
        assert_eq!(problems.len(), 9, "{problems:#?}");
        assert_eq!(
            problems[0],
            "plugin/orca-term.lua:1: `nvim_open_win` touches windows or buffers; only lua/orca-term/sidebar.lua, lua/orca-term/tabs.lua, lua/orca-term/terminal.lua, lua/orca-term/review.lua and lua/orca-term/notes.lua may"
        );
        assert_eq!(
            problems[7],
            "plugin/orca-term.lua:8: `vim.cmd` opens a split; only lua/orca-term/sidebar.lua, lua/orca-term/tabs.lua, lua/orca-term/terminal.lua, lua/orca-term/review.lua and lua/orca-term/notes.lua may open windows"
        );
    }

    #[test]
    fn a_pure_module_may_not_use_the_neovim_api_or_ex_commands() {
        let text = "local width = vim.fn.strdisplaywidth('x')\n\
                    vim.api.nvim_set_hl(0, 'X', {})\nvim.cmd('redraw')\n";
        for path in PURE_MODULES {
            assert_eq!(
                check_file(path, text),
                [
                    format!(
                        "{path}:2: `vim.api` is not allowed in a pure module; move it to lua/orca-term/sidebar.lua or lua/orca-term/tabs.lua"
                    ),
                    format!(
                        "{path}:3: `vim.cmd` is not allowed in a pure module; move it to lua/orca-term/sidebar.lua or lua/orca-term/tabs.lua"
                    ),
                ]
            );
        }
    }

    #[test]
    fn other_modules_may_use_the_neovim_api() {
        let text = "vim.api.nvim_set_hl(0, 'X', { link = 'Title' })\nvim.cmd('redraw')\n";
        assert!(check_file("lua/orca-term/highlights.lua", text).is_empty());
    }
}
