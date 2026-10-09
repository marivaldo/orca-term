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
- `tab`: Neovim's tabpage, which each worktree gets one of.
- `unit`: unit tests.
