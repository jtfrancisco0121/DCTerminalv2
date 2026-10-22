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
3. **P3 app run:** start screen (folder + request; Eagle-Eye or one of the six role tiles), launch the skill, progress panel from `status.json`, resume button.
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
