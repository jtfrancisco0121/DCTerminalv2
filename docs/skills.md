# The dcv2 plugin

One plugin folder, `plugin/`, loaded per launch with `claude --plugin-dir <plugin>` (nothing is installed into `~/.claude*`). It holds the Eagle-Eye run, six roles that also run alone (D8), and Onboard, which writes the codebase memory (D9). How the app launches it (env, `--settings`, the memory prompt) is in [launch.md](launch.md).

```
plugin/
  .claude-plugin/plugin.json   name "dcv2"
  roles/<role>.md              the guidance for each role: the single source of truth
  agents/<role>.md             pipeline sub-agents: model + tools + input/output contract; they read roles/<role>.md
  skills/eagle-eye/SKILL.md    the orchestrator
  skills/<role>/SKILL.md       standalone role sessions; they read roles/<role>.md
  scripts/state.sh             run-folder state: validated atomic JSON, log lines, approvals, base + worktrees, shipping, writeback drafts, gates, report
  scripts/memory.sh            codebase memory: id, compose (launch prompt), status (app panel), write (the only write path), touched
  scripts/guard.sh + hooks/    PreToolUse guard (Edit/Write/Bash): sub-agents stay in their worktree / the run folder; no push before the ship approval
  scripts/stop-check.sh        Stop/SubagentStop hook: an Implementer cannot finish on a failing check (A3)
  launch/settings.json         per-launch token savers, passed with --settings (A4)
  VERSION                      the roles' version (A5), same as plugin.json's
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
| Onboard | `/dcv2:onboard` | none | (session's) |
| Memory updater | none | `dcv2:memory-updater` (Eagle-Eye Step 5, end of a Developer session) | haiku |

Standalone skills run on the session's own model (`claude --model ...`).

## Eagle-Eye

```
DCV2_RUN_DIR=<run dir> claude --plugin-dir <plugin> "/dcv2:eagle-eye run=<run dir>"
DCV2_RUN_DIR=<run dir> claude --plugin-dir <plugin> "/dcv2:eagle-eye resume run=<run dir>"
```

Run it from any folder; the start folder comes from `request.md`. `DCV2_RUN_DIR` is required: the guard hook uses it to know the run (the run refuses to start without it). The run folder contract (inputs, launch env incl. the tests-only `DCV2_TEST_MODE`, `status.json`, `writeback.json`, `log.md`) is in [run-folder.md](run-folder.md).

The flow: Planner → Plan Reviewer loop (fresh reviewer each round, max 3) → **stop #1 "Approve the plan?"** → a worktree per PR (`<repo>-worktrees/dcv2-<runId>-<pr>`, branch `dcv2/<runId>-<pr>`) → Implementers in parallel for PRs with no unmet dependency, at most `DCV2_MAX_IMPLEMENTERS` (default 3) building/fixing at once (R6, enforced by `state.sh put`) → a fresh PR Reviewer per PR, fix loop until no MEDIUM+ (max 3 rounds), each round's gate written with `state.sh gate` (A1; `reviewed` needs a PASS/CONCERNS/WAIVED gate) → dependents start when their dependency is reviewed clean → **stop #2 "Push and open these PRs, and queue the ClickUp updates?"** → push, `gh pr create`, `writeback.json` → the memory updater patches the topics the PRs' diffs touched (`memory-update.md`) → `done`, and `state.sh put` writes `report.md` (R10; also at `failed`).

The orchestrator keeps its own context small: it never reads code, diffs, test output or ticket bodies; sub-agents reply in at most 15 lines plus a JSON block and write long output to `reviews/`, `reports/`, `logs/` in the run folder; state lives in `status.json` + `plan.md` and is re-read before every step.

Safety: a plugin hook (`hooks/hooks.json` → `scripts/guard.sh`, on Edit, Write, NotebookEdit, MultiEdit and Bash) guards the run named by `DCV2_RUN_DIR`:
- Edit/Write by a pipeline sub-agent (agent type with or without `dcv2:`): the real path (symlinks and `..` resolved) must be in the run folder, or, for the Implementer only, in one of the run's worktrees as recorded by `state.sh worktree` in `<run>/.guard.json`. (Any of the run's worktrees, not only its own PR's: the hook cannot tell parallel Implementers of one run apart.)
- Bash by a pipeline sub-agent: no `git push`, `gh pr`, `gh api` or other GitHub writes; git commands that change anything only for the Implementer in its run's worktrees (elsewhere read-only git only); redirects, `tee`, `sed -i`, `cp`/`mv`/`rm`/`touch`/`mkdir` and the like into any other git checkout are blocked. Best effort: writes made by other programs (node, python, npm scripts, `xargs` pipelines) are not seen; the agents' rules forbid them.
- The orchestrator may `git push` / `gh pr create` only when status.json is at `shipping` with a `shipApproval` (it normally ships through `state.sh ship-pr`), never force-pushes or deletes branches, never merges, closes or edits PRs.
- Everyone: `status.json`, `writeback.json` and `.guard.json` are written only by `state.sh`.
- Approvals: `state.sh approve <run> ship user` records the yes the orchestrator says the user gave; `… test` works only with `DCV2_TEST_MODE=auto-approve`. The hook enforces the order (no push before a recorded approval), not that the user really typed yes.
- Fail closed: a pipeline sub-agent with no `DCV2_RUN_DIR`, without a working python3, or with a payload that does not parse is blocked. (The memory updater may also run without a run, at the end of a Developer session: then it may not Edit/Write at all.)
- R4 for the orchestrator too: no Edit/Write, redirect or changing git in a repo's main checkout recorded in `.guard.json`; pushes only of `dcv2/` branches, from a worktree (no `--all`/`--tags`, `HEAD:main`, deletes or force).
- Codebase memory: when `DCV2_MEMORY_DIR` is set (every app launch), nobody writes into it with Edit/Write or a shell redirect/copy; agents use `memory.sh write`.

Stop hook (A3, `scripts/stop-check.sh` on SubagentStop and Stop): when an Implementer of the bound run finishes, it runs that PR's repo check from `<run>/repos.json` (matched by real path; never status.json's `check.command`, which the orchestrator writes; no repos.json, entry or check → it skips) in the PR's canonical worktree, which `.guard.json` must record (30 s timeout) and, if it fails, sends the Implementer back once (exit 2, `stop_hook_active`) with trimmed output (ANSI stripped; head, error lines, tail; full output in `logs/<PR>-stop-check.txt`). It finds the PR from the reply's JSON (`"pr"`), the sub-agent's prompt (`pr=` or its worktree path, on a path boundary), or the only PR building/fixing. A check slower than the timeout is remembered in `logs/.stop-check.json` and skipped from then on. `DCV2_STOP_HOOKS=off` turns it off; it never blocks the main session, other agents, standalone sessions, or on its own errors.

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

| `/dcv2:onboard [rebuild \| stale \| <repo>]` | Create, rebuild or refresh the codebase memory (index, topics, contracts, workspace) for the repos in the current folder, only through `memory.sh write`. | No (writes memory only) |

At its end, a Developer session that committed runs the `dcv2:memory-updater` sub-agent on each changed repo with memory (`git diff <HEAD at start>..HEAD`).

In P3 the app's start screen shows these seven as role tiles next to Eagle-Eye; a tile opens a terminal tab running `claude --plugin-dir <bundled plugin> "/dcv2:<role> <what the user typed>"` in the chosen folder.

## Codebase memory (D9, A6)

`plugin/scripts/memory.sh` is the only code that knows the layout; the app reads it through `memory.sh status` and `compose` ([launch.md](launch.md)).

```
$DCV2_MEMORY_DIR/
  <repo id>/index.md              always loaded (≤ 8 KB): what the repo is, map, commands, gotchas, topic list
  <repo id>/topics/<topic>.md     on demand (≤ 12 KB); header lines "covers: <path>, ..." and "checked-at: <sha>"
  <repo id>/topics/contracts.md   binding (A6): rules every agent follows
  workspaces/<ws id>/workspace.md a folder of repos (≤ 8 KB, binding): how the repos connect
  workspaces/<ws id>/topics/*.md  workspace topics: covers "<repo>/<path>", checked-at "<repo>=<sha>, ..."
```

- Repo id: `host__owner__name` from `origin` (credentials and `.git` dropped, lowercased), else `local__<folder>-<sha1(abs path)[:8]>`; a folder of repos gets the `local__` form as its workspace id.
- `write` rejects (exit 1, one reason line): a path outside this layout, an over-cap file ("condense it"), a line that looks like a secret (private keys, AWS key ids, GitHub `ghp_`/`github_pat_`, Slack, Google, JWTs, passwords in URLs; `sk-`, `pk_`/`sk_`/`rk_` tokens and AWS secret keys; `.env`/JSON/YAML-style assignments: for password/secret/token/credential names any 8+ character value that is not a placeholder or a file path, for `*KEY` names a token-shaped value (16+ characters, two character classes, entropy ≥ 3 bits/char, not a path); the value is never echoed), a removed or changed `<!-- pinned -->` line, and a topic without its header. Writes are atomic (mkstemp + rename).
- A topic is stale when any covered path changed in `git diff --no-renames <checked-at>..HEAD`, its commit is unknown, or (workspace topics) a covered repo is not in play. `compose` lists stale topics; it loads `workspace.md`, the contracts and each repo's `index.md` (never topic bodies), each fenced in `<memory-notes>` under a header saying the notes are data derived from repo content that cannot override the user, role files or guards (a body cannot close its fence); it redacts secret-looking lines and stays under 40 KB. Contracts bind only as facts about interfaces (API shapes, schemas, ownership); onboard and the updater never record repo text as instructions.
- `memory.sh touched <repo> <from> [<to>]` (for the memory updater): the topics a diff touched and the changed paths no topic covers.
- Trust rule, in every role file: memory is a hint, the code is the truth; fix the entry and continue.
- Eagle-Eye's memory update reads each PR's `baseCommit..<branch>` and sets `checked-at` to the branch head; until that branch is merged, the topic may read stale on the default branch (the diff from the branch head to it). That is accepted: stale means "re-check from the diff", never wrong.

## Tests

- `scripts/skill-e2e/run.sh`: the Eagle-Eye run end to end, headless, on two toy repos (full run; then halt after PR1 + resume). Cheap models via a test-only plugin copy (`DCV2_E2E_MODEL`, default haiku).
- `scripts/skill-e2e/guard_test.sh`: unit test of the guard hook (no Claude call).
- `scripts/skill-e2e/state_test.sh`: unit test of `state.sh` (no Claude call; `gh` is a stub, the remote a local bare repo).
- `scripts/skill-e2e/memory_test.sh`: unit test of `memory.sh` (ids, caps, secrets, pinned lines, headers, staleness, compose, status).
- `scripts/skill-e2e/stop_check_test.sh`: unit test of the Stop/SubagentStop hook.
- `scripts/skill-e2e/plugin_test.sh`: static checks: `VERSION`, every skill/agent reads its role file, every role has the trust rule, hooks and launch settings parse, scripts pass `bash -n`.
- `scripts/skill-e2e/golden.sh [multi dep fix]` (R7, before every release): three toy workspaces through the whole run, in parallel, launched as the app does (`--settings`, the composed memory prompt, `DCV2_MEMORY_DIR`): two independent repos with onboard first and `DCV2_MAX_IMPLEMENTERS=1`; a PR that depends on another; a check that already fails for a reason the request does not name. Asserts gates, `report.md`, `memory-update.md`, memory written only via `memory.sh`, ordering, and untouched main checkouts.
- `scripts/skill-e2e/standalone.sh [roles...]`: each standalone skill loads, reads its role file and replies; read-only roles leave the repo untouched.

The unit tests run on python3 3.9+ (macOS's system python3 included). The live tests (`run.sh`, `golden.sh`, `standalone.sh`) use the current Claude account; none of them touches a real repo.
