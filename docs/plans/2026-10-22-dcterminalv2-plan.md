# DCTerminalv2: plan

Status: approved 2026-10-22.

## Why

DCTerminal grew in layers (chat → terminal roles → pipeline v2 → v3 lanes, rounds, sizes). On real Koneksi tickets the v3 fan-out was too much: tabs opened on their own, closing one broke its PR, and "waiting" lanes were unclear. DCTerminalv2 starts again around one idea:

**One Claude session per run. It deploys sub-agents for every stage, asks you in the same tab when it needs you, and the app shows its progress.**

## Decisions

| # | Decision |
|---|---|
| D1 | Same stack: Tauri 2, React 19 + TS strict, Rust core (std threads, no async runtime), JSON stores with atomic write + `.bak`. |
| D2 | The pipeline is a **Claude Code skill**, not app code. The app launches `claude` with it and watches files. The same flow also works in a plain terminal. |
| D3 | Claude only. No Cursor, no chat (ACP) mode. |
| D4 | Keep: multiple windows, one Claude account per window (config dir per account), ClickUp intake + write-back per account. |
| D5 | Port proven DCTerminal code; never port the chain engine (bridge, run_actor, pipelines, rounds, roles/forms, lanes). |
| D6 | One PR per repo per run. A folder of repos (e.g. `~/Documents/Koneksi`) works without setup. |
| D7 | Two stops for you: approve the plan, then approve opening PRs + ClickUp posts. Anything else you say in the tab at any time. |
| D8 | Six roles also work alone, each in its own tab, not only inside the pipeline: Planner, Plan Reviewer, Developer, PR Reviewer, Recommendation, Codebase Audit. Each is a plugin skill (`/dcv2:<role>`); a role shared with the pipeline uses one guidance file for both, so they cannot drift. The start screen shows them as role tiles next to Eagle-Eye (P3). |
| D9 | Codebase memory: a small always-loaded index + on-demand topic files per repo (+ a workspace file), shared across accounts, refreshed per topic from git diffs, hint-not-truth (see "Codebase memory"). |
| D10 | Ideas from ADE: proof-based done (gates + app re-run), truth from Claude's session logs and hooks, guardrail Stop hooks, per-launch token savers, pinned roles + recorded fixtures (see "Ideas taken from ADE"). |

## How a run works

```
Start screen: folder + tickets (Fetch) or a typed request → Start
 └─ one terminal tab: claude  /eagle-eye  run=<run dir>
     ├─ sub-agent  Planner        → plan.md (stories, acceptance criteria, repo per story, PR list)
     ├─ sub-agent  Plan Reviewer  → fresh context; Planner fixes until clean
     │   ⏸ "Approve the plan?"            (you answer in the tab)
     ├─ sub-agents Implementer ×PR → own worktree in its repo; parallel unless a PR depends on another
     │                               runs that repo's checks, commits; story by story
     ├─ sub-agents PR Reviewer ×PR → fresh context per PR; fix loop until no MEDIUM+
     │   ⏸ "Open N PRs and post to ClickUp?"
     └─ pushes, opens PRs, writes the ClickUp drafts → done
```

Side panel (app): reads `status.json` and shows Plan ✓ / PR1 backend: building / PR3 engine: reviewed ✓, test results, PR links, and the ClickUp ticks.

## The run folder (the contract between skill and app)

`<app data>/runs/<runId>/`
- `request.md`: what you typed, plus the folder and repos.
- `tickets.json`: the ClickUp snapshot (ported format).
- `repos.json`: repos + check commands (ported workspace detection).
- `plan.md`: written by the skill.
- `status.json`: written by the skill after every step: stage, per PR {repo, branch, worktree, stories done/total, checks, review verdict, PR url}, and `waitingFor` (plan approval / ship approval / null).
- `writeback.json`: ClickUp comment + status drafts, posted only by the app, only after your ticks.
- `log.md`: one line per event, for the panel's timeline.

The skill never holds the ClickUp token; the app does all ClickUp calls.

## The skill

- Ships inside the app bundle and is loaded per launch (no install into `~/.claude*`). Phase 0 confirms the mechanism (`--plugin-dir` or `--add-dir` with `.claude/skills`).
- `SKILL.md` = the orchestrator rules. One prompt file per sub-agent role, adapted from DCTerminal's role prompts (planner, plan reviewer, implementer, PR reviewer).
- Worktrees: the orchestrator makes them itself (`git worktree add` in each repo) and gives each sub-agent its path, because a folder of repos is not one git repo.
- Resume: `/eagle-eye resume run=<dir>` continues from `status.json` + `plan.md`. Closing the tab never loses the run.
- Models: Opus for plan/review sub-agents, Sonnet for implementers (configurable).

## Codebase memory (D9)

Goal: a new session in a known repo does not pay to re-explore it, and memory never quietly misleads it.

**Layout** (stored by the app, outside the repo, shared by every account: `<app data>/memory/<repo id>/`)
- `index.md`: always loaded, ~1.5–2k tokens max: what the repo is, top-level map, build/test/check commands, top gotchas, and a list of topic files with one line each.
- `topics/<topic>.md`: loaded on demand only when a task touches that area (e.g. `auth.md`, `sync-engine.md`). Each has a header: the paths it covers and the commit it was last checked at.
- A folder of repos also gets `workspace.md`: how the repos connect (which API the desktop calls on the backend, where the engine sits, shared models and contracts). Sessions load `workspace.md` + the index of each repo in play.
- Setting (off by default): commit `index.md` into the repo as `CLAUDE.md` instead, for team sharing.

**What goes in.** Only what is expensive to rediscover: commands, conventions, non-obvious gotchas, the *why* behind decisions, where things live, cross-repo contracts. Not file listings, code summaries or anything a quick grep finds. Every fact is anchored to a path (e.g. "retry logic: `server/app/sync/retry.go`").

**Loading.** `index.md` (+ `workspace.md`) via `--append-system-prompt-file` at every launch, Eagle-Eye and standalone roles alike (stable prefix → prompt cache). Sub-agents get the memory folder path and open topic files as needed.

**Freshness, per topic.**
- `/dcv2:onboard` creates or rebuilds the memory (seventh role skill).
- The last step of every Eagle-Eye run and the end of every Developer session runs a small "memory update" sub-agent: it reads only `git diff <commit>..HEAD`, finds the topics whose covered paths changed, patches just those, and records the new commit. No diary entries: it changes facts that changed and nothing else (git and run reports keep history).
- On launch, topics whose paths changed since their recorded commit are flagged stale; the session refreshes only those, from the diff. Never a full re-onboard unless asked.

**Trust rule** (in every agent): memory is a hint, the code is the truth. If a remembered file or function isn't where memory says, trust the code, fix the memory entry, continue.

**Separation.** Claude Code's own auto-memory (per account) stays for personal preferences and feedback; codebase facts live only here.

**App-enforced limits.**
- Size caps per file; an over-cap update is rejected and the agent asked to condense.
- Secret scan before every write (tokens, keys, `.env` values) → rejected.
- Pinned facts: lines you mark as pinned (e.g. "never edit migration files by hand") cannot be removed or rewritten by automatic updates.
- The app shows per repo: memory present / N topics stale / missing, with Onboard and Refresh, and lets you read and edit the files.

## Reliability (R1–R10)

| # | What | Phase |
|---|---|---|
| R1 | App re-runs each PR's check command itself before STOP #2 and shows its own ✓/✗ beside the skill's claim. | P3 |
| R2 | Watchdog: no `status.json` change for N min while the session is idle → panel says so, offers Nudge ("continue from status.json") or Resume. | P3 |
| R3 | Claude Code version range pinned; out of range → warning. After a Claude Code update, a 30 s haiku self-test (the Phase 0 spike) runs once. | P3 |
| R4 | Guardrails as a plugin hook (code, not prompt): block force-push, any push before ship approval (approval recorded in status.json by the app), and writes to repos' main checkouts. | P2b |
| R5 | Worktree janitor: when a run ends or is abandoned, list its worktrees/branches, offer to remove the clean ones. | P3 |
| R6 | Concurrency cap: at most 3 Implementers at once by default (setting). | P2b |
| R7 | Golden-run suite: 2–3 recorded toy workspaces (multi-repo, dependency, failing check) on haiku before every release. | P2b |
| R8 | Ticket snapshot frozen at Start; ClickUp edits mid-run show "ticket changed", you decide whether to pull it in. | P4 |
| R9 | Cost/time line per run: tokens + minutes per stage in the panel. | P3 |
| R10 | Run report at the end (`report.md`): planned vs built, review rounds, retries. | P2b |

## Ideas taken from ADE (D10)

Reviewed `~/Documents/Projects/better-agentic-ide` (ADE) on 2026-10-22, read-only. Taken, adapted to keep work repos clean:

| # | Idea | Our version | Phase | ADE reference |
|---|---|---|---|---|
| A1 | Done needs proof | Each PR Reviewer writes `<run>/gates/<pr>.yml` (`gate`: PASS / CONCERNS / FAIL / WAIVED, reason, updated). A PR shows green only when its gate is PASS or WAIVED **and** the app's own check re-run (R1) passes; otherwise "claimed" with the reason. Unreadable gates are shown, never dropped. | P2b (gate files), P3 (panel) | `src/lib/bmadGates.ts`, `src/lib/storyState.ts` |
| A2 | Truth from Claude's own logs | The app tails the session JSONL (and `subagents/agent-<id>.jsonl`) from saved byte offsets: sub-agent spawn / launched / complete / resumed, and `message.usage` (deduped by message id; cache 5m/1h writes + reads) → exact cost and time per stage (R9) and the watchdog signal (R2: no transcript writes for N min). Hook events (SubagentStart/Stop, Stop, PermissionRequest) via a tiny forwarder binary posting to 127.0.0.1 with a per-launch token and a per-tab id from env → working / waiting / needs-you per tab. `status.json` stays the skill's plan of record; logs cross-check it. | P3 | `src-tauri/src/subagent.rs`, `usage.rs`, `agent_status.rs`, `hook_forward.rs` |
| A3 | Guardrail Stop hooks | Plugin Stop/SubagentStop hook runs the repo's lint/check on the worktree diff (30 s timeout); findings → `exit 2` so Claude fixes them; `stop_hook_active` guard so it fires once; env kill switch. The same mechanism carries R4 (no force-push, no push before ship approval, no writes to main checkouts) as PreToolUse blocks. | P2b | `claude-plugin/plugins/ade/hooks/hooks.json`, `scripts/slop-check.sh` |
| A4 | Token savers, per launch | Passed with `claude --settings <json>` at launch, never written into repos: `BASH_MAX_OUTPUT_LENGTH`, sub-agent default model, autocompact threshold, `permissions.deny` for `node_modules/**`, lockfiles, build output. Check output trimmed before it reaches an agent (strip ANSI/progress redraws, keep head + tail + error lines). Context meter warns at 60% (never auto-sends keys). | P2b (settings), P3 (meter, trimming) | `usage.rs` PRESETS / `add_deny_rules`, `src/lib/compactText.ts`, `useContextGuard.ts` |
| A5 | Pinned roles + recorded fixtures | Each role is one markdown body with a `VERSION` (commit sha); standalone skill and pipeline agent are composed from it (D8). Real hook payloads and JSONL samples recorded as test fixtures for a named Claude Code version; contract tests fail on a format change instead of the user's run (makes R3 concrete). | P2b (roles), P3 (fixtures) | `vendor/ade-setup/VERSION`, `src/lib/agentComposition.ts`, `src-tauri/src/fixtures/agent-events/` |
| A6 | Binding vs descriptive memory | Codebase memory splits **contracts** (binding: `workspace.md` + a `contracts` topic every agent must follow) from **notes** (descriptive topic files). | P2b | `.ade/context/` vs `.ade/knowledge/` |

Not taken: writing BMAD, `.claude/agents` or settings into each project (conflicts with clean work repos).

## What to port from DCTerminal

| Area | Source | Notes |
|---|---|---|
| Terminal | `src-tauri/src/pty/` (cut down), xterm components | Claude terminal tabs only |
| Windows | `windows.rs`, window parts of `store/` | multiple windows, each with its account |
| Accounts | `provider/claude*.rs`, account settings | config dir per account, usage line |
| ClickUp | `clickup/`, `commands/clickup*.rs`, intake + updates UI | keychain token per account, snapshot, write-back ticks |
| Repos | `workspace_repos.rs`, `worktree.rs` | detection + checks |
| Basics | `data_dir.rs`, `shell_env.rs`, `paths.rs`, `crash_log.rs`, `process_tree.rs`, `cli_detect.rs`, atomic JSON store | |

Not ported: `acp/`, `bridge/`, `run_actor/`, `pipelines/`, `rounds/`, `roles/`, `template/`, `orchestrator/`, Cursor, GitHub tab, history search. Can return later if missed.

## Build order

0. **Phase 0 (spikes, half a day):** skill loading per launch; sub-agents in parallel making worktrees in sibling repos; `status.json` updates reliably; resume after kill.
1. **P1 shell:** new Tauri app, windows + accounts, one Claude terminal tab, settings. `npm run check` gate from day one.
2. **P2 skill v1:** the full run in a plain terminal against a small test workspace (two toy repos). No app UI yet.
2b. **P2b skill hardening + memory:** `/dcv2:onboard`, memory load/update/staleness, R4, R6, R7, R10, A1 gate files, A3 hooks, A4 settings, A5 roles, A6.
3. **P3 app run:** (adds A1 panel, A2 log tailer + hook forwarder, A4 meter + trimming, A5 fixtures) start screen (folder + request; Eagle-Eye or one of the six role tiles), launch the skill, progress panel from `status.json`, resume button.
4. **P4 ClickUp:** port intake (Fetch) and write-back ticks; personal workspace only for live tests.
5. **P5 dogfood:** one real Koneksi Medium ticket end to end on the work account, then fix what hurts.

Each phase: agents build in worktrees, Claude reviews, fixes MEDIUM+, merges (as with v3).

## Answered (2026-10-22)

1. Old DCTerminal stays installed side by side until v2 is proven; v2 gets its own bundle id.
2. GitHub: private repo `jtfrancisco0121/DCTerminalv2`.
3. The skill stops only at the two stops (D7), not after each story.

Status: approved.

## Phase 0 results (2026-10-22, Claude Code 2.1.287)

- **Skill per launch: works.** `claude --plugin-dir <dir>` loads a plugin for that session only; its skill runs as `/<plugin>:<skill>` and its `agents/*.md` are usable as sub-agent types (`<plugin>:<agent>`, with `model:` and `tools:` in frontmatter). So the app ships one plugin folder: the orchestrator skill + planner / plan-reviewer / implementer / pr-reviewer agents. Nothing is installed into `~/.claude*`.
- **Parallel sub-agents across sibling repos: works.** In a folder of two git repos (not itself a repo), the orchestrator ran `git worktree add` in each repo, launched two sub-agents in one message (both ran at once), each fixed, tested and committed in its own worktree; the repos' main branches were untouched.
- **status.json: works.** The orchestrator wrote it before building and updated each PR as its sub-agent returned.
- **Resume after kill:** moved to P2 (it is skill logic over `status.json` + `plan.md`; `claude --resume` is the fallback).
- Headless result JSON reports sub-agent counts (spawned / completed / failed), useful for tests.
