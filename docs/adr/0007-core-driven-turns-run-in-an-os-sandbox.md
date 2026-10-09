---
status: accepted
---

# Core-driven turns run in an OS sandbox, with everything allowed inside it

When the core drives a turn, nobody is there to answer a permission prompt, and both agents turn an
unanswered prompt into a denial. We do not answer this with permission rules. Every core-driven
turn runs inside an **operating-system sandbox that confines writes to the lane's worktree**, and
inside that boundary **everything is allowed without asking**. claude gets its native sandbox
(`sandbox.enabled`, Bash auto-allowed only inside it, `allowUnsandboxedCommands: false`) under
`--permission-mode acceptEdits`. opencode keeps its own permissive defaults, but the core wraps
`opencode run` in `sandbox-exec` on macOS or bubblewrap on Linux, using a profile it generates for
each turn. The network stays open for both. The policy is fixed. The only configurable part is a
list of extra writable paths, such as package-manager caches, read from `orca-term.yaml` and its
local override, with the precedence printed. On a machine without the sandbox, the core refuses
the turn.

A reader will expect us to copy Orca, which launches claude with `--dangerously-skip-permissions`
by default behind a global Yolo/Manual toggle, or else to rely on each agent's own permission
rules. Both fall short of what a lane is supposed to guarantee. Decided in
[issue #18](https://github.com/marivaldo/orca-term/issues/18), which holds the measurements (claude
2.1.295, opencode 1.18.0, macOS).

## Considered options

- **Orca's Yolo** (bypass everything). Rejected: nothing confines the agent to its worktree. Even
  claude's newer `auto` mode was measured writing to `../outside.txt` and running `git push`
  without asking.
- **Each agent's native rules**: deny rules in claude, `external_directory` in opencode. Rejected:
  both inspect the text of a command. opencode caught `cd ..` and its write tool, but it let through
  `echo leak > ../outside.txt` and a write to our `lane.json`.
- **Conservative defaults** (edits only, Bash asks). Rejected: with nobody to ask, the turn cannot
  even `mkdir` or `git add`, and ends `success` with a list of `permission_denials`.
- **A domain allowlist for the network.** Rejected for now: `sandbox-exec` cannot filter by domain,
  and opencode reaches the model API through the same process. An allowlist would therefore need a
  proxy of ours for the length of each turn.

## Consequences

- The git common dir must stay writable so the agent can commit from the lane (measured: signed
  commits work inside both sandboxes). The profile therefore denies writes to what is ours
  specifically: every lane's `worktrees/*/orca-term`, `refs/orca-term/` and `.git/orca-term.yaml`.
  `packed-refs` is an accepted gap.
- Open network means an agent can exfiltrate what it reads and push its branch. The sandbox guards
  the filesystem, and nothing more.
- Takeover is outside this policy: the person runs the agent with their own settings and answers
  its prompts.
- Denials the agents report (claude's `permission_denials`, opencode's rejected tool calls) are
  normalised into the turn record and shown as a count on the finished turn. They are never a
  state. A sandbox block inside a Bash command shows up only as a failed command.
- The harness must assert that no launch carries `--dangerously-skip-permissions`,
  `bypassPermissions` or `--auto`, and must exercise the boundary with an integration test for
  each agent.
- claude's sandbox was seen failing even inside a worktree under `/private/tmp`, but not under
  `$HOME`, so lane bases should not be placed behind the `/tmp` symlink until that is explained.
