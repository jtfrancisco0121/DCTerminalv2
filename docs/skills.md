# The dcv2 plugin

One plugin folder, `plugin/`, loaded per launch with `claude --plugin-dir <plugin>` (nothing is installed into `~/.claude*`). It holds the Eagle-Eye run and six roles that also run alone (D8).

```
plugin/
  .claude-plugin/plugin.json   name "dcv2"
  roles/<role>.md              the guidance for each role: the single source of truth
  agents/<role>.md             pipeline sub-agents: model + tools + input/output contract; they read roles/<role>.md
  skills/eagle-eye/SKILL.md    the orchestrator
  skills/<role>/SKILL.md       standalone role sessions; they read roles/<role>.md
  scripts/state.sh             run-folder state: validated atomic JSON, log lines, approvals, base + worktrees, shipping, writeback drafts
  scripts/guard.sh + hooks/    PreToolUse guard (Edit/Write/Bash): sub-agents stay in their worktree / the run folder; no push before the ship approval
```

A role's skill and its sub-agent both start by reading `${CLAUDE_PLUGIN_ROOT}/roles/<role>.md`, so they cannot drift: change the role's behaviour in that one file. The skill or agent file only adds how it is used (standalone: talk to the user in the tab; pipeline: run folder in, short JSON reply out).

| Role | Standalone skill | Pipeline sub-agent | Model (sub-agent) |
|---|---|---|---|
| Planner | `/dcv2:planner` | `dcv2:planner` | opus |
| Plan Reviewer | `/dcv2:plan-reviewer` | `dcv2:plan-reviewer` | opus |
| Developer | `/dcv2:developer` | none (the pipeline uses the Implementer) | (session's) |
| Implementer | none | `dcv2:implementer` | sonnet |
| PR Reviewer | `/dcv2:pr-reviewer` | `dcv2:pr-reviewer` | opus |
| Recommendation | `/dcv2:recommendation` | none | (session's) |
| Codebase Audit | `/dcv2:codebase-audit` | none | (session's) |

Standalone skills run on the session's own model (`claude --model ...`).

## Eagle-Eye

```
DCV2_RUN_DIR=<run dir> claude --plugin-dir <plugin> "/dcv2:eagle-eye run=<run dir>"
DCV2_RUN_DIR=<run dir> claude --plugin-dir <plugin> "/dcv2:eagle-eye resume run=<run dir>"
```

Run it from any folder; the start folder comes from `request.md`. `DCV2_RUN_DIR` is required: the guard hook uses it to know the run (the run refuses to start without it). The run folder contract (inputs, launch env incl. the tests-only `DCV2_TEST_MODE`, `status.json`, `writeback.json`, `log.md`) is in [run-folder.md](run-folder.md).

The flow: Planner → Plan Reviewer loop (fresh reviewer each round, max 3) → **stop #1 "Approve the plan?"** → a worktree per PR (`<repo>-worktrees/dcv2-<runId>-<pr>`, branch `dcv2/<runId>-<pr>`) → Implementers in parallel for PRs with no unmet dependency → a fresh PR Reviewer per PR, fix loop until no MEDIUM+ (max 3 rounds) → dependents start when their dependency is reviewed clean → **stop #2 "Push and open these PRs, and queue the ClickUp updates?"** → push, `gh pr create`, `writeback.json`.

The orchestrator keeps its own context small: it never reads code, diffs, test output or ticket bodies; sub-agents reply in at most 15 lines plus a JSON block and write long output to `reviews/`, `reports/`, `logs/` in the run folder; state lives in `status.json` + `plan.md` and is re-read before every step.

Safety: a plugin hook (`hooks/hooks.json` → `scripts/guard.sh`, on Edit, Write, NotebookEdit, MultiEdit and Bash) guards the run named by `DCV2_RUN_DIR`:
- Edit/Write by a pipeline sub-agent (agent type with or without `dcv2:`): the real path (symlinks and `..` resolved) must be in the run folder, or, for the Implementer only, in one of the run's worktrees as recorded by `state.sh worktree` in `<run>/.guard.json`. (Any of the run's worktrees, not only its own PR's: the hook cannot tell parallel Implementers of one run apart.)
- Bash by a pipeline sub-agent: no `git push`, `gh pr`, `gh api` or other GitHub writes; git commands that change anything only for the Implementer in its run's worktrees (elsewhere read-only git only); redirects, `tee`, `sed -i`, `cp`/`mv`/`rm`/`touch`/`mkdir` and the like into any other git checkout are blocked. Best effort: writes made by other programs (node, python, npm scripts, `xargs` pipelines) are not seen; the agents' rules forbid them.
- The orchestrator may `git push` / `gh pr create` only when status.json is at `shipping` with a `shipApproval` (it normally ships through `state.sh ship-pr`), never force-pushes or deletes branches, never merges, closes or edits PRs.
- Everyone: `status.json`, `writeback.json` and `.guard.json` are written only by `state.sh`.
- Approvals: `state.sh approve <run> ship user` records the yes the orchestrator says the user gave; `… test` works only with `DCV2_TEST_MODE=auto-approve`. The hook enforces the order (no push before a recorded approval), not that the user really typed yes.
- Fail closed: a pipeline sub-agent with no `DCV2_RUN_DIR`, without a working python3, or with a payload that does not parse is blocked.

Standalone skills and the user's own sessions (no `DCV2_RUN_DIR`, no pipeline agent) are not affected. The skill also never touches the repos' main checkouts, never prints secrets, never calls ClickUp. A repo with no git remote is left `local-only` (nothing pushed, no PR) and the run says so.

## Standalone roles

Each opens an interactive session in the current folder (one repo or a folder of repos). The request is the skill's arguments; with none, the session asks for it. No run folder, no status file.

```
claude --plugin-dir <plugin> "/dcv2:<role> <request>"
```

| Skill | What it is for | Changes code? |
|---|---|---|
| `/dcv2:planner <request>` | Investigate the code and write an implementation plan for a feature or bug: findings, stories with acceptance criteria, PRs (one per repo, with dependencies), risks, open questions. Shown in the chat; saved to a file only when asked. | No |
| `/dcv2:plan-reviewer <plan file or text, and the request>` | Independent review of a plan against the request and the code: verdict (APPROVED / APPROVED WITH CHANGES / REQUIRES REVISION) and findings with severities. | No |
| `/dcv2:developer <task>` | Understand the codebase and do the task: implement, fix a bug, build a feature, draw a diagram, review code, investigate CI or production problems; or an onboarding overview. Commits only when asked; never pushes or opens PRs unless asked. | Yes |
| `/dcv2:pr-reviewer <PR URL, branch, or nothing>` | Strict review of a PR or branch against its intent: correctness, regressions, security, data, performance, tests. Posts the review on GitHub only when the user says yes. | No |
| `/dcv2:recommendation <focus, or nothing>` | Study the product and code and recommend features and improvements, each validated against the code, in tiers. Creates GitHub issues only when asked. | No |
| `/dcv2:codebase-audit <scope, or nothing>` | Evidence-based audit for bugs, security, data, performance, reliability and maintainability problems, with severities and confidence. Creates GitHub issues only when asked. | No |

In P3 the app's start screen shows these six as role tiles next to Eagle-Eye; a tile opens a terminal tab running `claude --plugin-dir <bundled plugin> "/dcv2:<role> <what the user typed>"` in the chosen folder.

## Tests

- `scripts/skill-e2e/run.sh`: the Eagle-Eye run end to end, headless, on two toy repos (full run; then halt after PR1 + resume). Cheap models via a test-only plugin copy (`DCV2_E2E_MODEL`, default haiku).
- `scripts/skill-e2e/guard_test.sh`: unit test of the guard hook (no Claude call).
- `scripts/skill-e2e/state_test.sh`: unit test of `state.sh` (no Claude call; `gh` is a stub, the remote a local bare repo).
- `scripts/skill-e2e/standalone.sh [roles...]`: each standalone skill loads, reads its role file and replies; read-only roles leave the repo untouched.

The live tests (`run.sh`, `standalone.sh`) use the current Claude account; none of them touches a real repo.
