# Vocabulary waivers

The vocabulary lint (`cargo xtask vocab`, part of `just check`) rejects every term that
`CONTEXT.md` lists under *Avoid*, in every tracked file except the glossary, the ADRs and the
skill setup docs. A term listed here is not enforced, because it has an unavoidable meaning of its
own. Each waiver needs a reason, and a waiver for a term the glossary no longer avoids fails the
lint.

- `new`: Rust's constructor idiom (`Type::new`) and plain English ("a new branch").
- `run`: the verb for executing a program (`fn run`, "run git"), never a session or a turn.
- `workspace`: Cargo's workspace, the build unit of this repository.
- `clone`: Rust's `Clone` trait and `.clone()`, never a copy of the repository.
- `comment`: git's comment lines in a commit message, and code comments.
- `request`: GitHub's pull request and the `pull_request` event.
- `tab`: Neovim's tabpage, which each worktree gets one of, and the application's tabs over a worktree's panels (`1 agent`, `2 nvim`, `3 shell`).
- `unit`: unit tests.
- `thread`: Rust's `std::thread`, never a session.
- `plugin`: the opencode plugin orca-term ships, Neovim plugins the person installs (claudecode.nvim), and the Neovim plugin orca-term no longer ships (ADR 0014); never the core.
- `server`: Neovim's server address (`v:servername`, `nvim --server`) and claudecode.nvim's server; never the core or a holder.
- `assistant`: claude's hook payload field `last_assistant_message`, which the core reads; never the agent.
- `chat`: opencode's plugin hook `chat.message`, which the plugin orca-term ships registers to see the person's prompt (#35); never a session.
