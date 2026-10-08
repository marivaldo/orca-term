# orca-term

A fleet of CLI coding agents working in parallel on one repository, each in its own git worktree,
reviewed from inside Neovim. This glossary fixes the vocabulary that every issue, module and
document in this repo is written in.

## Language

**Lane**:
The unit a person creates, names, lists and switches between: one tracked git worktree together
with at most one agent and its session. A lane is identified by the path of its worktree, and its
name is that directory's basename. A lane with no agent attached is still a lane. The primary
checkout is never a lane.
_Avoid_: unit, task, workspace, slot, station, tab

**Fleet**:
Every lane of one repository. No fleet spans repositories.
_Avoid_: swarm, pool, roster

**Primary checkout**:
The repository's main worktree, the one the person works in. It is listed by git alongside the
lanes but is not one of them, so no agent runs in it.
_Avoid_: main lane, root lane, base worktree

**Agent**:
The CLI program driven in a lane — `claude-code` or `opencode`. An agent is a mutable attribute of
a lane: swapping it leaves the lane the same lane.
_Avoid_: worker, bot, model, assistant

**Session**:
One agent's conversation, identified by that agent's own id for it. A lane holds at most one
session, and holds it mutably: discarding a conversation starts a new session on the same lane.
_Avoid_: run, thread, conversation, chat

**Turn**:
One cycle of prompt → response → diff inside a session. A turn is what the core is invoked for,
and what a change's authorship is attributed to — never the lane, whose agent can change.
_Avoid_: run, round, iteration, request

**Takeover**:
A person holding a lane's session directly instead of driving it through the core. Takeover is
exclusive: while it lasts, the core does not drive that lane.
_Avoid_: attach, interactive mode, manual mode

**Core**:
The orchestrator itself: a library plus a CLI, invoked once per turn, owning no agent process and
no daemon of its own. Neovim is a client of the core, never its host.
_Avoid_: daemon, server, engine, plugin

**Worktree**:
Git's worktree and nothing more: a checkout of the repository at a path of its own. A lane has
one. The lane is the tracked thing; the worktree is the checkout.
_Avoid_: orca worktree, checkout, clone
