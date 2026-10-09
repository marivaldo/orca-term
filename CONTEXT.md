# orca-term

A fleet of CLI coding agents working in parallel on one repository, each in a git worktree of its
own, reviewed from inside Neovim. This glossary fixes the vocabulary that every issue, module and
document in this repo is written in.

## Language

**Worktree**:
The unit a person creates, names, lists and switches between: a git worktree of the repository
other than the primary checkout, together with at most one agent and its session. A worktree is
identified by its path, and its name is that directory's basename. A worktree with no agent
attached is still a worktree. The primary checkout is never one.
_Avoid_: lane, unit, task, workspace, slot, station, tab, orca worktree, clone

**Fleet**:
Every worktree of one repository. No fleet spans repositories.
_Avoid_: swarm, pool, roster

**Primary checkout**:
The repository's main working tree, the one the person works in. It is listed by git alongside the
worktrees but is not one of them, so no agent runs in it.
_Avoid_: main worktree, root worktree, base worktree

**Agent**:
The CLI program driven in a worktree — `claude-code` or `opencode`. An agent is a mutable attribute
of a worktree: swapping it leaves the worktree the same worktree.
_Avoid_: worker, bot, model, assistant

**Session**:
One agent's conversation, identified by that agent's own id for it. A worktree holds at most one
current session, and holds it mutably: discarding a conversation starts a new session on the same
worktree. A discarded session is not forgotten: the worktree keeps its turns, and the person can
make it the current session again, even after swapping agents.
_Avoid_: run, thread, conversation, chat

**Turn**:
One cycle of prompt → response → diff inside a session: it starts when the person submits a prompt
to the agent and ends when the agent stops. A turn is what a change's authorship is attributed to — never the worktree, whose
agent can change. Turns are numbered across the whole worktree, never per session, so no two turns
of a worktree share a number. Edits the person makes between turns belong to no turn.
_Avoid_: run, round, iteration, request

**Unseen**:
A worktree whose latest turn ended after the person last reviewed it, whatever the outcome. It is a
mark on the worktree, independent of its state, and opening the worktree's review clears it.
_Avoid_: unread, new, unacknowledged

**Note**:
A remark the person attaches to a line, a range of lines or a whole file of a worktree's diff. A
note waits, pending, until the person sends it to the worktree's current session, and sending it
consumes it.
_Avoid_: comment, annotation, review comment

**Core**:
The orchestrator itself: a library plus a CLI, invoked per action, running no daemon of its own.
It keeps each running agent alive through that agent's holder. Neovim is a client of the core,
never its host.
_Avoid_: daemon, server, engine, plugin

**Holder**:
The detached process that keeps one worktree's agent running and holds its screen, so the agent
survives Neovim quitting and Neovim can attach to it again. A worktree has at most one holder, and
it lives exactly as long as the agent.
_Avoid_: daemon, server, multiplexer
