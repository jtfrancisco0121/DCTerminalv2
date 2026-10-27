# AFK runs: cleanup, typed app actions, no pasted commands

Status: brief from JT 2026-10-27 (one PR, `afk-cleanup`). Plugin 0.5.0.

## Why

JT drives runs AFK from Claude remote-control on his phone. He can neither paste shell commands nor click app buttons. After run `r-20261026-170436-216f` finished, the Eagle-Eye session tried `git worktree remove` / `git branch -d` / `git push origin --delete` in the main checkout; the guard blocked it (correctly) and Claude told JT to run the commands himself. Some actions exist only as app buttons (Re-run checks, Clean up worktrees, Check for ticket changes, the Claude Code self-test).

Decision: the guard stays exactly as strict. The run gets narrow, safe ways to do what it routinely needs, the app cleans up merged runs on its own, and the skill never hands the user a command. Merging PRs stays JT's: nothing here merges, closes or edits a PR.

## What changes

| Area | Change |
|---|---|
| Auto cleanup (app) | A background sweep (on launch, then every 30 s it looks; each run is asked about at most every 10 min) takes runs at `stage: done` that have PRs with a `prUrl`, are at most 30 days old, are the app's own (not adopted) and are not fully handled yet. Per PR, `gh pr view <url> -R <recorded repo> --json state,mergeCommit,headRefName,headRefOid` (one call at a time). Only `MERGED` PRs whose head is the run's own `dcv2/<runId>-<pr>` branch: the janitor removes the worktree (`verify_worktree`, no `--force`, a dirty one is kept and reported), `git branch -d` (never `-D`), and the remote branch is deleted only when the remote tip equals `headRefOid`, with `git -c core.hooksPath=/dev/null push <recorded url> --delete refs/heads/<branch>` (no force; "already deleted" is fine). Open PRs are asked again later; closed-unmerged, local-only and foreign PRs are left alone. Outcome per PR in `<run>/app-cleanup.json` (app-written, guard-protected), a line in the run panel and an `app:` line in `log.md`. Setting "Clean up merged runs automatically" (Settings › Runs, default on). |
| `state.sh cleanup <run>` | For "clean up" typed by the user: only at `done` or `failed`. Per PR of `.guard.json`: removes the worktree with the janitor's rules (in shell: verified path and branch, no `--force`, dirty or with kept ignored files → kept), `git branch -d`, and deletes the remote branch only for a merged PR (`gh pr view -R`, head = the run's branch, remote tip = `headRefOid`, the recorded remote URL, no force). Prints one line per PR, logs once. |
| Guard | The orchestrator still runs no raw `git worktree <add/remove/prune/move…>`, `git branch -d/-D/-m…` or `git push --delete` anywhere in a bound run (the `worktree` exemption for the main checkout is gone): only `state.sh` does it. Sub-agents never run `state.sh cleanup`/`request`. `requests/` and `app-cleanup.json` are written only by state.sh / the app. `check_node` normalises the runtime path (cwd-relative, `..`) before matching. |
| Request channel | `state.sh request <run> recheck\|cleanup\|tickets\|selftest` writes `requests/<id>.json` (`{"schema":1,"id","kind","at"}`, no free text). The app's request sweep (every 2 s, own runs) performs only those fixed, idempotent actions: R1 check re-run, the cleanup above for this run plus the janitor's clean worktrees (finished runs only), the R8 ticket-change check (the token stays in the app; the result names ticket ids and changed fields only), the R3 self-test with the run's account. Unknown kinds and malformed files are ignored; one request per run and kind per 60 s (the rest answered `refused`). The result goes to `requests/<id>.result.json` (`state: running\|done\|refused\|ignored`, `ok`, one-line `message`) and an `app:` line in `log.md`. `state.sh request-wait <run> <id> [--timeout <s>]` (default 90, max 600) waits for it and prints one line. |
| Skill | Never hand the user a shell command: use the state.sh/request command if one exists; otherwise one line saying what could not be done and why, also in the run report. Typed "re-run the checks", "check the tickets", "clean up", "run the self-test" → `S request` + `S request-wait`, then report the result. At `done`: "merged PRs are cleaned up automatically (or type 'clean up')". `S draft-pr`/`push`/`post-review`/`ready` log their own lines: the orchestrator does not log them again. |
| Live tests | Per-repo app setting "Allow live Claude tests in checks" (default off) → repos.json `liveTests: true`. The guard did not block launching `claude` before; now a pipeline sub-agent never runs `claude` directly, and `scripts/skill-e2e/golden.sh` / `bmad_spike.sh` run by a sub-agent only for the Implementer or PR Reviewer, inside its recorded worktree, when that repo has `liveTests`. The orchestrator passes `liveTests` in the repo list; the Planner then puts such runs in the PR's check instead of Manual steps (and prefers automating over Manual steps generally). |
| Fixes from r-20261026-170436-216f | Guard `check_node` path normalisation; BMAD doc links also on `failed` runs; a ticket no story covers gets no ClickUp comment draft (`comment: null`, so nothing is auto-posted; status logic unchanged), and the app's auto-post skips a draft with an empty `prs` list; `state.sh log` drops a line identical to the last one, so "draft PR opened" is logged once. |
| Docs, version | launch.md / run-folder.md: request channel, `app-cleanup.json`, repos.json `liveTests`, the settings. Plugin `VERSION` and `plugin.json` 0.5.0. |

## Things to know

- **`git branch -d` after a squash merge** refuses (the branch is not merged into the main checkout's HEAD). The branch is then kept and the record says so; the brief forbids `-D`.
- **Requests are untrusted Claude output.** The app reads only the kind (from the file name and the body, which must agree), never a path, PR id or text from them. Results carry no token and no ticket text beyond ids and field names.
- **Adopted runs** (the dev app's) are left to the app that owns them, as with auto-post.

## Tests

| What | Where |
|---|---|
| Auto cleanup decision: merged → cleaned; open → asked again; closed → untouched; dirty → kept; tip mismatch → remote kept; local-only → untouched; foreign head → untouched; setting off → nothing; already handled → no gh call | Rust unit tests (`run/cleanup.rs`) with a fake `gh` lookup and real git + a local bare remote |
| Request handler: unknown kind ignored, rate limit, result written, kind/name mismatch ignored | Rust unit tests (`run/requests.rs`) with fake actions |
| `state.sh cleanup` / `request` / `request-wait` / `log` dedupe / writeback with an uncovered ticket | `scripts/skill-e2e/state_test.sh` (gh shim, local bare remote) |
| Guard: raw cleanup commands blocked for the orchestrator, `state.sh cleanup` allowed, sub-agents refused, `requests/` protected, runtime path forms, `claude` / golden rules with and without `liveTests` | `scripts/skill-e2e/guard_test.sh` |
| Panel: cleanup lines, BMAD links on failed runs; Settings toggle | vitest |
| Live (JT): none changed in `golden.sh`; the skill text changed, so a `golden.sh github` run is worth one pass | — |
