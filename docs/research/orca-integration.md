# Research: how Orca integrates a worktree's work and surfaces overlapping edits

Resolves [#21](https://github.com/marivaldo/orca-term/issues/21).

Facts and sources only, no recommendation. Source of truth: a local clone of
[`stablyai/orca`](https://github.com/stablyai/orca) at commit
`ad7af63b20ad0e8a873686a89c70526aa62b4f9d`. Every citation is `path:line` inside that clone; docs
live under `docs/site/content/docs/`, abbreviated below as `docs/…`. Where something was searched
for and not found, this document says **does not exist** and names what was searched.

In Orca's vocabulary a "worktree" or "workspace" is what this repo calls a lane (`CONTEXT.md`).

## 1. Integration actions

All integration actions live in the **Source Control** panel of the right sidebar and in the PR
view. None of them is exposed by the `orca` CLI: the only git RPC the CLI calls is `git.status`
(grep of `src/cli` for `'git.*'` / `'github.*'` method names), and `docs/cli/reference.mdx` has no
merge, rebase, push or PR section (its sections are runtime, repos, worktrees, terminals, files,
browser, computer use, emulator, Linear, skills, account, artifacts, automations).

The panel's action set is a closed union (`src/renderer/src/components/right-sidebar/source-control-dropdown-item-types.ts:13-28`):
`commit`, `commit_push`, `commit_sync`, `abort_merge`, `abort_rebase`, `create_pr`,
`push_create_pr`, `push`, `force_push`, `pull`, `fast_forward`, `sync`, `rebase_base`, `fetch`,
`publish`.

| Action | Exists? | What it does | Source |
| --- | --- | --- | --- |
| **Create PR / MR** | Yes | After the branch is pushed (or via **Push & Create PR**), a composer asks for base, title, body, draft. GitHub runs `gh pr create --repo --base --title --body-file [--head] [--draft]`; an empty body can be filled from the repo's PR template. On success the PR number is written into the worktree's metadata (`linkedPR` etc.) and a `post-push` refresh is queued. GitLab, Bitbucket, Azure DevOps and Gitea have their own creators. | `docs/review/commit-push.mdx:23-29`; `src/main/github/client/create/create-github-pull-request.ts:79-105`; `src/renderer/src/components/right-sidebar/source-control/review/use-hosted-review-created.ts:74-75,102-115`; `src/main/source-control/forge-provider.ts:96-128,315-323` |
| **Stacked PR** | Yes, GitHub only | "Stack this PR above #N" when the chosen base already has an open PR; stack-aware merge merges the PR and every PR below it, atomically (fails closed). | `docs/review/github.mdx:31-37` |
| **Merge the PR on the host** | Yes | PR view merge button runs `gh pr merge <n> --merge|--squash|--rebase` (default `squash`), deliberately **without** `--delete-branch` "because it deletes the local branch, which fails while the worktree is checked out on it". Pre-flight refuses when review is required, changes are requested, a merge queue is required, or GitHub reports `CONFLICTING` (then it lists conflicting files). | `src/main/github/client/merge/merge-pr.ts:12-23,88-96,110-164` |
| **Auto-merge / merge queue** | Yes, GitHub only | **Enable auto-merge** with the repo's default method; **Merge when ready** when the base uses a merge queue. Hidden for draft, closed, conflicting or unstable PRs. | `docs/review/github.mdx:27-29` |
| **Merge locally into base** | **Does not exist** | No action merges a worktree's branch into the base branch or the primary checkout. Searched `merge into`, `mergeInto`, `localMerge`, `mergeBranch`, `'merge', '--` across `src`: the only local `git merge` invocations are `merge --ff-only` (rebase fallback for a branch with no HEAD) and `merge --abort`. | `src/main/git/remote-rebase.ts:83-90`; `src/main/git/source-control/git-conflict-operation.ts:56` |
| **Rebase onto base** | Yes | **Rebase from `<base>`** is offered only when the base is a remote-tracking ref. It fetches the base branch into a private temp ref `refs/orca/rebase/<uuid>` (so concurrent fetches cannot move it), then runs `git rebase --onto <temp> <fork-point>` (fork point from `merge-base --fork-point`, else plain `rebase <temp>`), and deletes the temp ref. Runs under a per-worktree operation lock. | `src/renderer/src/components/right-sidebar/source-control-dropdown-remote-items.ts:167-182`; `src/main/git/remote-rebase.ts:15-23,38-104` |
| **Pull / Sync / Fast-forward** (against the branch's upstream, not the base) | Yes | `pull` uses the user's configured pull strategy ("merge by default"); `fast_forward` is `pull --ff-only`. | `src/main/git/remote.ts:101-130`; `source-control-dropdown-remote-items.ts:97-165` |
| **Squash locally** | **Does not exist** as an action | Squash appears only as a host-side PR merge method. The docs mention local squash only as something the user did ("When you've rewritten history (rebase, amend, squash)…"). | `merge-pr.ts:19`; `docs/review/commit-push.mdx:21` |
| **Push** | Yes | Pushes to `origin`, setting upstream the first time. **Force push with lease** is a separate explicit action, never a fallback. | `docs/review/commit-push.mdx:17-21`; `src/main/git/remote.ts:52-53` |

**What each does to the worktree and branch afterwards.** None of the integration actions removes,
archives or renames the worktree or its branch. Rebase and pull rewrite/advance the branch in place
(`remote-rebase.ts:83-90`, `remote.ts:101-115`). Host merge leaves the local branch untouched by
design (`merge-pr.ts:88`); the renderer only patches the PR state to `merged` and shows a toast
(`src/renderer/src/components/github-item-dialog/land-pull-request/pr-actions-panel.tsx:216-245`).
Removal is a separate, user-invoked delete (section 5).

**Where the agent is involved.** Orca never runs an agent implicitly during integration. It offers
explicit "AI action" buttons, each backed by an editable per-repo **action recipe** (agent, CLI
args, prompt template) (`docs/review/commit-push.mdx:31-39`;
`src/shared/source-control-ai-actions.ts:9-17,78-88`):

- Text generation (no agent session, just text): **Generate with AI** for the commit message,
  **Generate pull request details with AI** for title/body/draft (`docs/review/commit-push.mdx:10,29`).
- Agent launches in the active worktree: **Fix with AI** for a failed commit or push hook
  (prompt says not to bypass hooks, commit, push or open a review), **Fix broken checks** for
  failing PR checks, **Resolve with AI** for conflicts, and review-comment resolution
  (`docs/review/commit-push.mdx:15,27`; `docs/review/github.mdx:25`;
  `src/shared/source-control-ai-actions.ts:12-17,65-71`). The conflict prompts are in section 3.

## 2. Base drift

- **Ahead/behind against the base: yes, in the Source Control panel only.** `getBranchCompare`
  resolves base and HEAD, computes the merge base, and counts divergence both ways
  (`summary.commitsAhead`, `summary.commitsBehind`)
  (`src/main/git/source-control/branch-compare.ts:17-121`, counts at `107-114`; type at
  `src/shared/git-diff-compare-types.ts:15-26`). The branch context row (`branch → base`) renders
  them as uncoloured `↑N` / `↓N` chips titled "N commits ahead of / behind `<base>`"; the code
  comment says behind was added because "Ahead alone hid … a rebased branch that has also fallen
  behind its base" (`src/renderer/src/components/right-sidebar/source-control/panel/branch-context-stats.ts:5-31,83-111`).
  Next to it a chip shows lines added/removed against the fork point (`docs/review/commit-push.mdx:49`).
- **Ahead/behind against the upstream** drives the Push/Pull/Sync labels and the primary button
  (`source-control-dropdown-remote-items.ts:46-47,53-165`; `docs/review/commit-push.mdx:53`). This is
  the branch's own remote, not the base.
- **On the sidebar row: does not exist.** No sidebar component renders commits-behind-base. A grep
  of `src/renderer/src/components/sidebar` for `behind|outdated|out of date|stale base` hits only
  the local-base toast below and a merged-PR comment.
- **"Behind base" inside PR conflict details:** when GitHub reports the PR as conflicting, the
  Checks panel shows "N commits behind (base commit: `abc1234`)" above the conflicting files
  (`src/renderer/src/components/right-sidebar/checks-panel/conflict-summary.tsx:23-60`;
  computed as `rev-list --count <head>..<latest base>` in `src/main/github/conflict-summary.ts:96-109`).
- **Offer to update:** the only update path is the manual **Rebase from `<base>`** action (section 1).
  There is no GitHub "Update branch" call (grep `update-branch|updateBranch` finds only comments).
  No automatic rebase or merge of the base into a worktree exists.
- **Local base branch drift (at creation time):** when creating a worktree from a remote-tracking
  base, Orca may report that the *local* base branch (e.g. `main`) is behind, with a persistent
  toast: "Local `main` is behind `origin/main` … AI diffs may compare to stale history", offering
  **Keep `main` up to date** (an opt-in setting that fast-forwards the local base branch on create)
  (`src/renderer/src/components/sidebar/local-base-ref-suggestion-toast.tsx:89-148`;
  `src/main/git/worktree-base-refresh.ts:19-25,60`).
- Not drift UI: `worktree-base-divergence.ts` measures ahead+behind (cap 100 commits) only to
  decide whether a pre-prepared checkout can be cheaply retargeted at create time
  (`src/main/git/worktree-base-divergence.ts:22-31,144-174`).

## 3. Overlap and conflicts

- **Cross-worktree overlap detection: does not exist.** Orca never compares two worktrees' changes
  with each other, before or at integration. Searched `src` and `docs` for `cross-worktree`,
  `other worktree(s)`, `sibling worktree`, `same file(s)`, `overlapping edit/change/file`,
  `files touched`, `potential conflict`, `collision`, `conflicts with another`: every hit is about
  tab routing, terminal history, branch reuse or UI layout, none about file overlap. The docs pitch
  isolation as the mechanism instead: worktrees are "what makes parallel agents safe — they never
  step on each other's files" (`docs/model/worktrees.mdx:8`).
- **Conflicts are only ever detected against the base branch, and only for a PR.** When GitHub's
  PR lookup returns `mergeable === 'CONFLICTING'` (and the repo is local, not SSH), Orca derives a
  conflict summary locally: fetch `origin <base>` (10 s timeout, falls back to GitHub's
  `baseRefOid` offline), `merge-base`, then `git merge-tree --write-tree --name-only` of the PR head
  against the latest base tip; the conflicting file list is cached per (head, base) OID pair. Git
  older than 2.38 fails closed (no list).
  (`src/main/github/client/lookup/branch-lookup-derived-data.ts:61-75`;
  `src/main/github/conflict-summary.ts:32-116,150-185,210-276`). Without a PR on GitHub there is no
  pre-integration conflict check at all.
- **Presentation of PR conflicts:** the Checks panel lists "Conflicting files" with the behind count
  (`checks-panel/conflict-summary.tsx:23-60`) and offers copyable commands to make GitHub recompute
  mergeability (`git fetch origin`, an empty commit, `git push`) (`conflict-summary.tsx:15-21`).
  The merge button refuses with the same file list (`merge-pr.ts:142-178`).
- **Presentation of live local conflicts** (after Rebase from base, Pull, or anything the user ran in
  a terminal): Source Control shows an amber card "Merge|Rebase|Cherry-pick conflicts: N unresolved"
  with **Resolve with AI**, **Review conflicts** and **Abort merge/rebase**, plus an
  "… in progress" banner when an operation is mid-way with no conflicts
  (`src/renderer/src/components/right-sidebar/source-control/listing/conflict-status-cards.tsx:8-174`;
  `docs/review/commit-push.mdx:55`). The sidebar row shows a "Merging" / "Cherry-picking" badge;
  the rebase badge is suppressed "because rebases already surface in source control"
  (`src/renderer/src/components/sidebar/worktree-card-presentation.tsx:94-96`;
  `src/renderer/src/components/sidebar/WorktreeCardHelpers.tsx:26-27`).
- **Resolution in the app:** a merge-conflict UI with three-way view and inline resolution
  (`docs/review/diff-viewer.mdx:13`); the combined diff excludes unresolved conflicts "because the
  normal two-way diff pipeline is not conflict-safe" and links to **Review conflicts**
  (`src/renderer/src/components/editor/combined-diff/review-controls/combined-diff-skipped-conflicts.tsx:31,84,99`).
- **Resolution by the agent:** **Resolve with AI** opens a composer that launches the recipe's agent
  in the worktree with a generated prompt (`src/renderer/src/components/right-sidebar/source-control/ai/use-ai.ts:150-173`).
  For a live local operation the prompt lists conflicted files and kinds, the continue/skip command,
  and rules: start from `git status`, don't take ours/theirs wholesale, never `reset --hard`,
  `checkout .`, `stash` or abort, stage resolved paths, run `--continue`, run `git diff --check`,
  don't push or make extra commits, and reply with per-file decisions
  (`src/shared/source-control-conflict-prompts.ts:78-131`). For a PR whose host reports conflicts
  but which has no local `MERGE_HEAD`, the prompt tells the agent to **fetch the base and merge it
  into the worktree branch** (`git merge --no-ff --no-edit FETCH_HEAD`) to reproduce and resolve
  them (`source-control-conflict-prompts.ts:133-187`;
  `src/renderer/src/components/right-sidebar/checks-panel/use-checks-panel-ai-queue.tsx:54,67`).
- **Resolution in the terminal:** always possible; Orca picks up plain-git changes on the next
  render (`docs/model/worktrees.mdx:137-139`).

## 4. PR / CI state source (the sidebar row's icon)

- **Provider detection:** five forge providers are tried in order (GitLab, GitHub, Bitbucket,
  Azure DevOps, Gitea); the first whose `resolveRepository` accepts the repo wins, otherwise the
  provider is `'unsupported'` (`src/main/source-control/forge-provider.ts:315-344`). Clients: `gh`,
  `glab`, and REST clients (`forge-provider.ts:83`; e.g. Bitbucket uses `fetch`,
  `src/main/bitbucket/client.ts:89`).
- **GitHub depends on the `gh` CLI.** "Orca talks to GitHub through the GitHub CLI (`gh`) on your
  machine" (`docs/github-errors.mdx:6`). The branch lookup is `gh api repos/<o>/<r>/pulls?head=…&state=all&per_page=1`
  or `gh pr view <branch> --json …` (`src/main/github/client/lookup/pr-branch-lookup.ts:22-23,149-150`),
  requesting `state`, `statusCheckRollup`, `isDraft`, `mergeable`, `reviewDecision`,
  `mergeStateStatus`, `autoMergeRequest`, base/head refs and OIDs
  (`src/main/github/client/lookup/pull-request-lookup-data.ts:65-66`). The CI status on the row is
  derived from `statusCheckRollup` (`src/main/github/client/lookup/pr-refresh-outcome-assembly.ts:41`);
  the Checks panel's detail uses `gh api` / GraphQL with `gh pr checks` as fallback
  (`src/main/github/client/check/get-pr-checks.ts:48-50,149`).
- **Polling, not webhooks.** A main-process refresh coordinator with a queue, pacing, retry and
  visibility tracking (`src/main/github/pr-refresh-coordinator.ts:1-31`). Refresh reasons are
  `visible`, `active`, `post-push`, `manual`, `swr` (`src/shared/github/pull-request-refresh-types.ts:39`).
  Only worktrees whose rows are visible are enqueued as `visible`; archived, bare and
  disconnected-SSH worktrees are skipped (`src/renderer/src/store/github/visible-hosted-review-refresh-targets.ts:24-60`).
  Intervals (`src/shared/review-refresh-policy.ts:4-24`):

  | Cached state | Selected worktree | Other visible worktree |
  | --- | --- | --- |
  | No PR found | 60 s | 15 min |
  | Open / draft | 60 s | 120 s |
  | Merged | 60 s while checks pending or unknown, then **stops** | same |
  | Closed | 15 min | 15 min |

  A changed local HEAD bypasses the freshness window
  (`src/main/github/pr-refresh-candidate-policy.ts:105-117,160-166`); `manual`, `active` and
  `post-push` (2.5 s after push) bypass it too (`pr-refresh-candidate-policy.ts:11-12,85-87`).
  Background refreshes are paced: at least 10 s apart within a 5-minute budget window, active
  bursts in a 30 s window (`src/main/github/pr-refresh-pacing.ts:3-6`).
- **The icon:** state glyph tinted purple for merged, emerald for open, muted for closed/draft; CI
  tone (rose failure, amber pending, emerald success) overrides it only while the review is open
  (`src/renderer/src/components/sidebar/worktree-review-helpers.tsx:28-84`). Failed GitHub Actions
  also show as a red chip on the worktree (`docs/review/github.mdx:49-51`).
- **Without GitHub / without `gh`:** missing `gh` surfaces "GitHub CLI is unavailable"
  (`docs/github-errors.mdx:19,129-136`). On rate limits Orca keeps the last known status, shows a
  banner, and a circuit breaker refuses new `gh` spawns for that bucket for a short window
  (`docs/github-errors.mdx:33,138-142`). Auth is per host, so an SSH/remote host needs its own
  `gh auth login` (`docs/github-errors.mdx:104-105`). A repo no provider recognises is
  `'unsupported'`, which Source Control's remote-repo helper maps to "no provider" (`null`)
  (`forge-provider.ts:340-344`; `src/renderer/src/components/right-sidebar/source-control/review/remote-repo.ts:73`).
  Local ahead/behind (section 2) and local conflict state (section 3) need no host.

## 5. Cleanup after merge

- **Auto-removal of merged worktrees: does not exist.** Searched `src` and `docs` for
  `archiveMerged`, `autoArchive`, `cleanupMerged`, `deleteMerged`, `merged worktree(s)`: the only
  hit is advice in a recipe, "Delete merged worktrees aggressively. Orca makes this cheap — one
  click, worktree and branch both gone" (`docs/recipes/jump-worktrees.mdx:16`). Host merge itself
  touches nothing local (`merge-pr.ts:88`).
- **Flagged, passively:** the row's review glyph turns purple `merged`
  (`worktree-review-helpers.tsx:53-56`), shown while the worktree HEAD still equals the PR head or a
  commit confirmed contained in it (`src/renderer/src/components/sidebar/worktree-card-pr-display.ts:8-22`).
  In Source Control, Pull, Fast-forward, Sync and Publish read "PR is already merged"
  (`source-control-dropdown-remote-items.ts:104-105,125-126,148-149,200-213`).
- **Delete (user-invoked) removes directory and branch** with confirmation
  (`docs/model/worktrees.mdx:15,23`). The branch is deleted with `git branch -d` so unmerged work is
  kept; if that refuses, Orca proves the branch has no unmerged changes against
  `branch.<name>.base`, `origin/HEAD` or `HEAD` (after a `fetch --prune` of that remote, since
  "deleting a worktree often follows a PR merge"), recognising squash merges by `git cherry` and by
  a stable patch-id match plus a tree-neutral `merge-tree`, and only then force-deletes via
  `update-ref -d <ref> <expected-head>`. Otherwise the branch is preserved and a **Review N
  Branches** toast offers force-delete
  (`src/main/git/worktree-branch-removal.ts:15-62,105-189`;
  `src/shared/git-branch-cleanup.ts:48-88,141-227`; `docs/model/worktrees.mdx:125-127`).
- An `orca.yaml` `scripts.archive` hook runs before removal; a failed hook blocks removal unless the
  user waives it (`docs/model/orca-yaml.mdx:39,102`;
  `src/renderer/src/components/sidebar/delete-worktree-failure-toast.tsx:20-21`).
- **Resource Manager → Clean up workspaces** is the bulk review surface. Its automatic suggestion
  reasons are inactivity only: `archived` (idle ≥ 7 days) and `idle-clean` (idle ≥ 30 days); merged
  is **not** a reason (`src/shared/workspace-cleanup.ts:7-12,242-257`). The user can filter by
  review state, which includes `merged` (`src/shared/workspace-cleanup-filter-model.ts:21`;
  `docs/model/worktrees.mdx:121-123`).

## Implications for orca-term

Facts only; the decisions belong to the integration ticket.

- Orca has no local merge-into-base action: its integration path is push → hosted PR → host-side
  merge (`merge-pr.ts:88-96`), with rebase-onto-base as the only local base integration
  (`remote-rebase.ts:83-90`).
- Orca does not detect two lanes touching the same file; its only conflict signal is a lane's
  branch against the base, and only once a GitHub PR reports `CONFLICTING`
  (`branch-lookup-derived-data.ts:61-75`). The computation itself is plain local git
  (`merge-tree --write-tree --name-only`, Git ≥ 2.38) and needs no host (`conflict-summary.ts:210-276`).
- Ahead/behind against the base is computed locally with merge-base and rev-list
  (`branch-compare.ts:87-114`) and shown in the per-lane panel, not on the fleet row
  (`branch-context-stats.ts:83-111`).
- The agent is involved only through explicit, user-triggered buttons with editable prompt
  recipes (commit message, PR details, fix hook failure, fix checks, resolve conflicts, resolve
  review comments) (`source-control-ai-actions.ts:9-17`). The conflict prompts forbid the agent
  from pushing, stashing, resetting or aborting (`source-control-conflict-prompts.ts:120-127`).
- PR/CI state on the row depends on `gh` (GitHub) or the other forges' clients, is polled with
  state-dependent intervals from 60 s to 15 min and stops for merged reviews with settled checks
  (`review-refresh-policy.ts:4-24`), and degrades to last-known state on failure
  (`docs/github-errors.mdx:138-142`).
- Nothing is removed after a merge without the person asking; merged is a passive flag
  (`worktree-review-helpers.tsx:53-56`), and branch deletion on removal is guarded by a
  squash-aware "no unmerged changes" proof (`git-branch-cleanup.ts:141-227`).
- None of the integration actions is reachable from Orca's CLI (`src/cli` calls only `git.status`).

## Could not determine

- **What "archive" does today.** The docs describe "Archive or delete — one click removes the
  worktree and branch" and an archive action in the context menu (`docs/model/worktrees.mdx:23,113`),
  and an `isArchived` flag hides a worktree from the sidebar
  (`src/renderer/src/components/sidebar/visible-worktrees.ts:100`), but no non-test code path in
  `src` was found that sets `isArchived` to `true`, and the sidebar components mention "archive"
  only as the pre-removal hook. Whether archive is a distinct, still-reachable action was not
  established.
- Runtime behaviour was not exercised: everything above is read from source and docs, not run.
