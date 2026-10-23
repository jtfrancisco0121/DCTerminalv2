# Review on GitHub: draft PRs + review comments

Status: approved 2026-10-23 (one comment per round; start-screen toggle, default on; draft opens after round 1 when checks pass). Build after JT's current local run is finished (the dev app loads `plugin/` live from this repo).

## Why

Today the PR Reviewer loop runs quietly on local branches; nothing appears on GitHub until stop #2. JT wants the loop visible where PRs normally live: after the first build a **draft PR** exists, every review round is **posted as a PR comment**, every fix is pushed to the same PR, and stop #2 turns the drafts into real PRs.

## The new flow

```
Planner ⇄ Plan Reviewer
⏸ Stop #1 "Approve the plan? (this also lets me push dcv2/ branches and open draft PRs)"
Per PR:
  Implementer round 1 → commits locally, checks pass
  → push branch, open DRAFT PR (title, body from plan; "🤖 Eagle-Eye run … — review in progress")
  PR Reviewer round n → reads the diff, writes reviews/PRn-round<n>.md
  → post it as a PR comment ("Eagle-Eye review — round n: REQUEST CHANGES, 2 MEDIUM, 1 LOW" + findings with file:line)
  → MEDIUM+? Implementer fixes → push new commits (never force) → comment "Round n fixes: <commits>" → next review round
  → clean: post "Eagle-Eye review — round n: APPROVED", gate PASS
⏸ Stop #2 "Mark these N draft PRs ready for review, and queue the ClickUp updates?"
  → `gh pr ready` per PR, final PR body (stories, checks, review rounds) → ClickUp drafts as today
```

"No" at stop #2 leaves the drafts as drafts (nothing is closed or deleted). Leaving a PR out = it stays a draft.

## What changes

| Area | Change |
|---|---|
| D7 | Stop #1's yes now also authorises pushing the run's `dcv2/` branches and opening **draft** PRs. Stop #2's yes authorises `gh pr ready` + ClickUp. The question text says so. |
| `state.sh` | New `draft-pr <run> <PR>` (push without force + `gh pr create --draft`, records `prUrl`, `draft: true`), `push <run> <PR>` (push new commits of a drafted PR), `post-review <run> <PR> <round>` (posts `reviews/<PR>-round<n>.md` as a PR comment, records the comment URL), `ready <run> <PR>` (after ship approval: final body + `gh pr ready`). `ship-pr` becomes `ready`. Every post is secret-scanned (the memory.sh scanner) and capped at GitHub's 65 536 chars (longer → summary + "full review in the run folder"). |
| Guard | Push / `gh pr create --draft` / `gh pr comment` allowed for the orchestrator after the **plan** approval, only through state.sh, only `dcv2/` branches from the run's worktrees, only the run's own PR URLs. `gh pr ready` only after ship approval. Still never force-push, merge, close, or edit others' PRs. Sub-agents still never touch GitHub. |
| Guard gap to close (exists today) | The orchestrator is blocked from `gh pr comment/edit/merge/...` but **not** from `gh api` or `gh issue comment` (only sub-agents are). New rule: the orchestrator runs no GitHub-writing `gh` command at all (`gh api` with a method other than GET or with `-f/-F/--input`, any `gh <x> <write verb>`); every GitHub write goes through a state.sh command, which checks the stage, the approval, the branch and that the PR URL is the run's own before calling `gh`. |
| PR Reviewer | Unchanged review; its file gets a fixed comment-friendly format (verdict line, findings as `SEVERITY — path:line — what — fix`). It never posts itself; the orchestrator calls `post-review`. |
| status.json | `prs[].draft` (bool), `prs[].comments` ([{round, url}]), new state `drafted`, `opened` now means "ready for review". Old runs read as before (schema stays 1, fields optional). |
| No remote | Repo without a remote: as today (local-only, reviews stay in files); the run says so. |
| App panel | PR link shown from the first draft, a link per review comment, "draft" / "ready" badge; R1 app check re-run unchanged at stop #2. |
| Resume | A drafted PR is found by its branch (`gh pr list --head`), never opened twice; comments already posted for a round are not re-posted (recorded URL). |
| Setting | Per run on the start screen: "Review on GitHub (draft PRs)", default on; off = today's quiet local loop. |
| Tests | `gh` shim + local bare remote in the e2e/golden suites (no real GitHub); guard tests for the new allow/deny rules; state.sh tests for each new command. One live check on a throwaway personal repo only. |

## Things to know

- **Teammates see it.** On work repos, draft PRs and review comments are visible to everyone with access, under JT's GitHub account; watchers may get notifications for comments. Draft PRs don't request reviewers.
- **Own-PR limits.** GitHub doesn't allow "Request changes" or "Approve" reviews on your own PR, so rounds are posted as PR comments (not formal reviews).
- **More pushes.** Each fix round adds commits to a public branch; history is never rewritten (no force-push), so the PR shows the whole loop.
- **CI runs earlier.** Repos with CI on PRs will run it on the draft and every fix push. The R1/app check can later read the CI status too (follow-up).

## Build

One PR (plugin + app panel + tests), the usual loop: build in a worktree → review → fix MEDIUM+ → merge, **when no Eagle-Eye run is active**.

## Added 2026-10-23

### ClickUp comments post automatically (JT: comments only)

- After the yes at stop #2, the **app** posts every comment draft from `writeback.json` to the run's own tickets, with no ticks. Status changes stay as ticks (they move tickets on the team board).
- The token stays in the app (keychain, per account). Claude never gets it: no token in env, files or prompts. The skill keeps writing drafts only.
- Trigger: the app sees `shipApproval` in status.json and `writeback.json` written. Because status.json is written by Claude, auto-post is limited to what P4 already trusts: only tickets in the signed run-trust snapshot, the run's own account, one comment per ticket per run (recorded in the signed posted record, never twice), secret scan + size cap on the text, and the comment text itself is shown in the panel after posting with a link.
- Setting per account: "Post ClickUp comments automatically after ship approval", default on. Off = today's ticks.
- The panel shows each comment as posted / failed (retry button) / skipped.

### Worktree setup (JT: yes)

- `state.sh worktree` runs a setup step after making a new worktree: install packages from the repo's lockfile (`npm ci` / `pnpm install --frozen-lockfile` / `yarn install --frozen-lockfile` / `go mod download` / none for Cargo), output to `<run>/logs/<PR>-setup.txt`, one status line back. A failed install marks the PR's check `error` with the log path, not a crash.
- Per-repo setting in the app, off by default: "Copy these local files into run worktrees" (e.g. `.env`, `.env.local`), an explicit list of paths from the main checkout. Copied files are git-ignored ones only (refuse a tracked path), never committed (guard + `git status` check before each commit), and removed by the janitor with the worktree.
- `repos.json` gains `setup` (command or null) and `copyFiles` ([paths]) per repo, written by the app.
