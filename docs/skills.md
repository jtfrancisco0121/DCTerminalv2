# The dcv2 plugin

One plugin folder, `plugin/`, loaded per launch with `claude --plugin-dir <plugin>` (nothing is installed into `~/.claude*`). It holds the Eagle-Eye run, six roles that also run alone (D8), and Onboard, which writes the codebase memory (D9). How the app launches it (env, `--settings`, the memory prompt) is in [launch.md](launch.md).

```
plugin/
  .claude-plugin/plugin.json   name "dcv2"
  roles/<role>.md              the guidance for each role: the single source of truth
  agents/<role>.md             pipeline sub-agents: model + tools + input/output contract; they read roles/<role>.md
  skills/eagle-eye/SKILL.md    the orchestrator
  skills/<role>/SKILL.md       standalone role sessions; they read roles/<role>.md
  scripts/state.sh             run-folder state: validated atomic JSON, log lines, approvals, base + worktrees (copyFiles + setup), shipping, GitHub review mode (draft PRs, review comments, ready), writeback drafts, gates, report, cleanup (done / failed), app requests (recheck, cleanup, tickets, selftest)
  scripts/memory.sh            codebase memory: id, compose (launch prompt), status (app panel), write (the only write path), touched, scan (the secret scanner, also used before GitHub posts)
  scripts/guard.sh + hooks/    PreToolUse guard (Edit/Write/Bash): sub-agents stay in their worktree / the run folder; no push before the ship approval
  scripts/stop-check.sh        Stop/SubagentStop hook: an Implementer cannot finish on a failing check (A3) or on slop in the lines it added (`slop-patterns.tsv`)
  launch/settings.json         per-launch token savers, passed with --settings (A4)
  VERSION                      the roles' version (A5), same as plugin.json's
```

A role's skill and its sub-agent both start by reading `${CLAUDE_PLUGIN_ROOT}/roles/<role>.md`, so they cannot drift: change the role's behaviour in that one file. The skill or agent file only adds how it is used (standalone: talk to the user in the tab; pipeline: run folder in, short JSON reply out).

| Role | Standalone skill | Pipeline sub-agent | Model (sub-agent) |
|---|---|---|---|
| Planner | `/dcv2:planner` | `dcv2:planner` | opus |
| Plan Reviewer | `/dcv2:plan-reviewer` | `dcv2:plan-reviewer` | opus |
| Risk Reviewer | `/dcv2:risk-reviewer` | `dcv2:risk-reviewer` (Eagle-Eye beside the Plan Reviewer; the standalone Planner, Codebase Audit and Recommendation) | opus |
| Developer | `/dcv2:developer` | none (the pipeline uses the Implementer) | (session's) |
| Implementer | none | `dcv2:implementer` | sonnet |
| PR Reviewer | `/dcv2:pr-reviewer` | `dcv2:pr-reviewer` | opus |
| Recommendation | `/dcv2:recommendation` | none | (session's) |
| Codebase Audit | `/dcv2:codebase-audit` | none | (session's) |
| QA Engineer | `/dcv2:qa-engineer` | none | (session's) |
| Product Manager | `/dcv2:product-manager` | none | (session's) |
| Business Analyst | `/dcv2:analyst` | none | (session's) |
| Senior Engineer | `/dcv2:senior-engineer` | none | (session's) |
| Principal Engineer | `/dcv2:principal-engineer` | none | (session's) |
| Onboard | `/dcv2:onboard` | none | (session's) |
| QA | `/dcv2:qa` | `dcv2:qa` (Eagle-Eye Step 3, after a clean review round of a QA-on PR) | sonnet |
| Memory updater | none | `dcv2:memory-updater` (Eagle-Eye Step 5, end of a Developer session) | haiku |

Standalone skills run on the session's own model (`claude --model ...`).

## Eagle-Eye

```
DCV2_RUN_DIR=<run dir> claude --plugin-dir <plugin> "/dcv2:eagle-eye run=<run dir>"
DCV2_RUN_DIR=<run dir> claude --plugin-dir <plugin> "/dcv2:eagle-eye resume run=<run dir>"
```

Run it from any folder; the start folder comes from `request.md`. `DCV2_RUN_DIR` is required: the guard hook uses it to know the run (the run refuses to start without it). The run folder contract (inputs, launch env incl. the tests-only `DCV2_TEST_MODE`, `status.json`, `writeback.json`, `log.md`) is in [run-folder.md](run-folder.md).

The flow: Planner → plan review loop (each round a fresh Plan Reviewer and a fresh Risk Reviewer in parallel; a MEDIUM+ finding or risk from either sends the plan back to the Planner, LOW risks are only listed; max 5 rounds) → **stop #1 "Approve the plan?"** → a worktree per PR (`<repo>-worktrees/dcv2-<runId>-<pr>`, branch `dcv2/<runId>-<pr>`) → Implementers in parallel for PRs with no unmet dependency, at most `DCV2_MAX_IMPLEMENTERS` (default 3) building/fixing at once (R6, enforced by `state.sh put`) → a fresh PR Reviewer per PR, fix loop until no MEDIUM+ (max 3 rounds), each round's gate written with `state.sh gate` (A1; `reviewed` needs a PASS/CONCERNS/WAIVED gate) → dependents start when their dependency is reviewed clean → **stop #2 "Push and open these PRs, and queue the ClickUp updates?"** → push, `gh pr create`, `writeback.json` (GitHub review mode, `meta.json` `reviewMode: "github"`: a draft PR opens after each PR's first build, every review round is posted as one PR comment and every fix round is pushed with a "Round n fixes" comment, all through `state.sh`; stop #2 asks "Mark these N draft PRs ready for review, and queue the ClickUp updates?" and `state.sh ready` marks them ready) → the memory updater patches the topics the PRs' diffs touched (`memory-update.md`) → `done`, and `state.sh put` writes `report.md` (R10; also at `failed`).

The PR Reviewer's review carries a `## Criteria → tests` table (`| Story | Criterion | Proving test |`, `path::test name` or `manual: <reason>`); an uncovered criterion is a MEDIUM finding and `state.sh gate` refuses PASS/CONCERNS for a review with no table, an empty one or an unmapped row (FAIL and WAIVED are unchanged). The Implementer's report names the proving test per criterion. Commits (Implementer and developer) keep the `S<n>: <title>` / `<PR> review fixes: <summary>` subject and add a blank line and a why paragraph; release version files are never changed unless a story says so, and an unplanned change to one is a MEDIUM PR Reviewer finding.

BMAD planning (meta.json `bmad` auto/on, run-folder.md "BMAD mode"): before the normal plan the Planner runs `pass=size` and, when BMAD applies (Large, or mode on), `pass=bmad`, which follows the vendored BMAD-METHOD spec, architecture (and PRD, UX) skills headless and writes only under `<run>/bmad/`. `state.sh bmad-size` / `bmad-init` / `bmad-check` record it, and `state.sh put` / `approve` refuse plan-approval until the check passes on the current documents. The default plan pass then reads `<run>/bmad/output/` and every story cites the spec items it covers, and `state.sh` refuses plan approval otherwise. The Plan Reviewer and the PR Reviewer read the same documents and check for drift. Runs with `bmad=off` (and all runs without the field) are unchanged.

The orchestrator keeps its own context small: it never reads code, diffs, test output or ticket bodies; sub-agents reply in at most 15 lines plus a JSON block and write long output to `reviews/`, `reports/`, `logs/` in the run folder; state lives in `status.json` + `plan.md` and is re-read before every step.

Safety: a plugin hook (`hooks/hooks.json` → `scripts/guard.sh`, on Edit, Write, NotebookEdit, MultiEdit, Bash, Monitor, PowerShell and `mcp__*` tools) guards the run named by `DCV2_RUN_DIR`:
- Edit/Write by a pipeline sub-agent (agent type with or without `dcv2:`): the real path (symlinks and `..` resolved) must be in the run folder, or, for the Implementer only, in one of the run's worktrees as recorded by `state.sh worktree` in `<run>/.guard.json`. (Any of the run's worktrees, not only its own PR's: the hook cannot tell parallel Implementers of one run apart.)
- Bash by a pipeline sub-agent: no `git push`, `gh pr`, `gh api` or other GitHub writes; git commands that change anything only for the Implementer in its run's worktrees (elsewhere read-only git only); redirects, `tee`, `sed -i`, `cp`/`mv`/`rm`/`touch`/`mkdir` and the like into any other git checkout are blocked. Best effort: writes made by other programs (node, python, npm scripts, `xargs` pipelines) are not seen; the agents' rules forbid them.
- The orchestrator, local review mode: `git push` / `gh pr create` only when status.json is at `shipping` with a `shipApproval` (it normally ships through `state.sh ship-pr`). GitHub review mode: no direct push or `gh pr create` at all; `state.sh draft-pr|push|post-review|comment` only after the plan approval, `state.sh ready` only after the ship approval. Either mode: no GitHub-writing `gh` (`gh api` writes, `gh pr comment|edit|ready|merge|close|…`, `gh issue|release|repo <write>`), never force-pushes or deletes branches, never merges, closes or edits PRs.
- Everyone in a run: no `security dump-keychain|find-generic-password|find-internet-password|export`. `repos.json` (app-written) is protected like `meta.json`.
- Sub-agents never run `state.sh draft-pr|push|post-review|comment|ready|ship-pr|cleanup|request` (matched by the script's name, with or without `bash`).
- The orchestrator never runs `git worktree <anything but list>`, a changing `git branch` or `git push --delete` in a bound run: `state.sh cleanup` (done / failed) and the app's merged-run cleanup remove worktrees and branches (0.5.0). `<run>/requests/` is written only by `state.sh request` and the app.
- A pipeline sub-agent never runs `claude`; `scripts/skill-e2e/golden.sh` / `bmad_spike.sh` only the Implementer or PR Reviewer runs, from its own worktree, when repos.json gives the repo `liveTests: true` (0.5.0). `gh api` is read-only for the orchestrator: no method flag or GET only (any short-flag cluster with `X` sets the method), no `-f/-F/--field/--raw-field/--input`.
- The Implementer may not `git add` / `git commit` a file `state.sh worktree` copied in from the main checkout (repos.json `copyFiles`, recorded in `.guard.json`), and may not change git remotes or config.
- Everyone: `status.json`, `writeback.json` and `.guard.json` are written only by `state.sh`.
- Approvals: `state.sh approve <run> ship user` records the yes the orchestrator says the user gave; `… test` works only with `DCV2_TEST_MODE=auto-approve`. The hook enforces the order (no push before a recorded approval), not that the user really typed yes.
- Fail closed: a pipeline sub-agent with no `DCV2_RUN_DIR`, without a working python3, or with a payload that does not parse is blocked. (The memory updater may also run without a run, at the end of a Developer session: then it may not Edit/Write at all.)
- R4 for the orchestrator too: no Edit/Write, redirect or changing git in a repo's main checkout recorded in `.guard.json`; pushes only of `dcv2/` branches, from a worktree (no `--all`/`--tags`, `HEAD:main`, deletes or force).
- Codebase memory: when `DCV2_MEMORY_DIR` is set (every app launch), nobody writes into it with Edit/Write or a shell redirect/copy; agents use `memory.sh write`.

Stop hook (A3, `scripts/stop-check.sh` on SubagentStop and Stop): when an Implementer of the bound run finishes, it runs that PR's repo check from `<run>/repos.json` (matched by real path; never status.json's `check.command`, which the orchestrator writes; no repos.json, entry or check → it skips) in the PR's canonical worktree, which `.guard.json` must record (30 s timeout) and, if it fails, sends the Implementer back once (exit 2, `stop_hook_active`) with trimmed output (ANSI stripped; head, error lines, tail; full output in `logs/<PR>-stop-check.txt`). It finds the PR from the reply's JSON (`"pr"`), the sub-agent's prompt (`pr=` or its worktree path, on a path boundary), or the only PR building/fixing. A check slower than the timeout is remembered in `logs/.stop-check.json` and skipped from then on. `DCV2_STOP_HOOKS=off` turns it off; it never blocks the main session, other agents, standalone sessions, or on its own errors.

Epic-plan mode (run-folder.md "Epic-plan mode"): the Planner's `pass=epic` writes `waves.md` (waves of 3 to 6 tickets with `Depends on` earlier waves only), `state.sh waves` checks it and writes `waves.json`, the reviewers check for too-big waves, hidden dependencies and waves that cannot ship alone, and the run ends at `done` after the plan approval with no worktree, PR, ship step or writeback.

Decision log: the repo's `topics/decisions.md` (bullets `- YYYY-MM-DD | decision | reason | rules out: what`) is loaded after the contracts. A plan may add a `## Decisions` section after `## Risks` in the same form; a plan that contradicts an entry must state why its reason no longer holds (the Plan Reviewer raises it as MEDIUM otherwise), and the memory updater records the approved Decisions at the end of the run (`state.sh plan <run> decisions`).

Learned lines: an Implementer or PR Reviewer reply may carry up to 3 `Learned: <one line>` lines (durable, repo-specific, no secrets) before its JSON block. The orchestrator logs them (`learned: <line>`) and hands only those lines to the memory updater at Step 5, which stores them with `memory.sh learn` (duplicates dropped, secrets refused, caps enforced); the developer role does the same at its memory step. Only memory.sh writes memory.

Slop check (same hook): the check has one outcome, `pass`, `absent` (no check command), `slow` (cached slow or timed out now) or `fail`. For every outcome but `fail` the hook then matches `plugin/scripts/slop-patterns.tsv` (id, scope, pattern, message; narrow starter rules: `dbg!(` in Rust, new TODO/FIXME/XXX comment markers, the assistant-disclaimer phrase, filler comments, `console.log`/`console.debug` under `src/**/*.ts(x)`) against the lines the PR added since its base commit (`git diff --unified=0 <base>...HEAD` plus uncommitted tracked changes; base from `.guard.json` `bases`). Excluded: the TSV, `fixtures/slop/`, `plugin/bmad/`, lock files. At most 30 findings, exit 2, and the message says "check not run in the hook (slow)" when the check did not run. A failing check is reported alone. `DCV2_SLOP_CHECK=off` (setting `run.slopCheck`) turns slop off. The app's `run/slop.rs` and the run panel's "Slop check" action give the same verdict; `fixtures/slop/` is the shared fixture.

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
| `/dcv2:risk-reviewer <plan, branch / PR, or report>` | Independent review of what could go wrong if a plan is built, a change ships or a report's advice is followed (data, security, compatibility, deployment and rollback, concurrency, performance, blast radius): verdict (NO BLOCKING RISKS / RISKS FOUND) and risks with severities and mitigations. | No |
| `/dcv2:developer <task>` | Understand the codebase and do the task: implement, fix a bug, build a feature, draw a diagram, review code, investigate CI or production problems; or an onboarding overview. Commits only when asked; never pushes or opens PRs unless asked. | Yes |
| `/dcv2:pr-reviewer <PR URL, branch, or nothing>` | Strict review of a PR or branch against its intent: correctness, regressions, security, data, performance, tests. Posts the review on GitHub only when the user says yes. | No |
| `/dcv2:recommendation <focus, or nothing>` | Study the product and code and recommend features and improvements, each validated against the code, in tiers. Creates GitHub issues only when asked. | No |
| `/dcv2:codebase-audit <scope, or nothing>` | Evidence-based audit for bugs, security, data, performance, reliability and maintainability problems, with severities and confidence. Creates GitHub issues only when asked. | No |
| `/dcv2:qa-engineer <request>` | A QA Engineer persona with the playbooks test cases, bug report and regression scope; can turn a drafted bug list into ClickUp tickets through the ClickUp helper (below). | No |
| `/dcv2:product-manager <request>` | A Product Manager persona with the playbooks PRD, epic breakdown (an `Epic: <title>` parent plus subtasks), release notes and status update; can create the epic's tickets through the ClickUp helper (below). | No |
| `/dcv2:analyst <request>` | A Business Analyst persona with the playbooks requirements analysis (business rules, edge cases, gaps, Given/When/Then stories), data analysis (on a copy of the file, never the user's; redacted, aggregated output) and process map (Mermaid); can create drafted tickets through the ClickUp helper (below). | No |
| `/dcv2:senior-engineer <task>` | A Senior Engineer persona with the playbooks write tests, safe refactor and performance investigation; follows the Developer's rules for every change, commits only when asked, never pushes, and runs the memory update for repos it committed to. | Yes |
| `/dcv2:principal-engineer <request>` | A Principal Engineer persona with the playbooks RFC, impact analysis and tech roadmap; reads the repo's decision log first, offers `/dcv2:risk-reviewer` on a finished RFC and `/dcv2:product-manager` for a roadmap's first quarter. | No |
| `/dcv2:onboard [rebuild \| stale \| <repo>]` | Create, rebuild or refresh the codebase memory (index, topics, contracts, workspace) for the repos in the current folder, only through `memory.sh write`. | No (writes memory only) |

The Planner, Codebase Audit and Recommendation sessions have their draft checked by a fresh `dcv2:risk-reviewer` sub-agent before they show it: a MEDIUM+ risk means the session revises the draft and runs a fresh reviewer, until a round reports none (at most 5 rounds); LOW risks are listed in a closing `## Risk review` section, and risks still open after round 5 are listed there for the user to decide. With no run bound, the guard lets this sub-agent run read-only (it writes nothing with Edit/Write and only read-only git), like the memory updater.

At its end, a Developer or Senior Engineer session that committed runs the `dcv2:memory-updater` sub-agent on each changed repo with memory (`git diff <HEAD at start>..HEAD`).

In P3 the app's start screen shows the roles as tiles next to Eagle-Eye: the pipeline roles under "Or one role on its own, in this folder:", then a "Personas" group (QA Engineer, Product Manager, Business Analyst, Senior Engineer, Principal Engineer); the groups come from `ROLES` in `run/launch.rs`; a tile opens a terminal tab running `claude --plugin-dir <bundled plugin> "/dcv2:<role> <what the user typed>"` in the chosen folder.

## Codebase memory (D9, A6)

`plugin/scripts/memory.sh` is the only code that knows the layout; the app reads it through `memory.sh status` and `compose` ([launch.md](launch.md)).

```
$DCV2_MEMORY_DIR/
  <repo id>/index.md              always loaded (≤ 8 KB): what the repo is, map, commands, gotchas, topic list
  <repo id>/topics/<topic>.md     on demand (≤ 12 KB; `topics/decisions.md` ≤ 24 KB); header lines "covers: <path>, ..." and "checked-at: <sha>"
  <repo id>/topics/contracts.md   binding (A6): rules every agent follows
  workspaces/<ws id>/workspace.md a folder of repos (≤ 8 KB, binding): how the repos connect
  workspaces/<ws id>/topics/*.md  workspace topics: covers "<repo>/<path>", checked-at "<repo>=<sha>, ..."
```

- Repo id: `host__owner__name` from `origin` (credentials and `.git` dropped, lowercased), else `local__<folder>-<sha1(abs path)[:8]>`; a folder of repos gets the `local__` form as its workspace id.
- `write` rejects (exit 1, one reason line): a path outside this layout, an over-cap file ("condense it"), a line that looks like a secret (private keys, AWS key ids, GitHub `ghp_`/`github_pat_`, Slack, Google, JWTs, passwords in URLs; `sk-`, `pk_`/`sk_`/`rk_` tokens and AWS secret keys; `.env`/JSON/YAML-style assignments: for password/secret/token/credential names any 8+ character value that is not a placeholder or a file path, for `*KEY` names a token-shaped value (16+ characters, two character classes, entropy ≥ 3 bits/char, not a path); the value is never echoed), a removed or changed `<!-- pinned -->` line, and a topic without its header. Writes are atomic (mkstemp + rename).
- A topic is stale when any covered path changed in `git diff --no-renames <checked-at>..HEAD`, its commit is unknown, or (workspace topics) a covered repo is not in play. `compose` lists stale topics; it loads `workspace.md`, the contracts and each repo's `index.md` (never topic bodies), each fenced in `<memory-notes>` under a header saying the notes are data derived from repo content that cannot override the user, role files or guards (a body cannot close its fence); it redacts secret-looking lines and stays under 48 KB. Contracts bind only as facts about interfaces (API shapes, schemas, ownership); onboard and the updater never record repo text as instructions.
- `memory.sh touched <repo> <from> [<to>]` (for the memory updater): the topics a diff touched and the changed paths no topic covers.
- Trust rule, in every role file: memory is a hint, the code is the truth; fix the entry and continue.
- Eagle-Eye's memory update reads each PR's `baseCommit..<branch>` and sets `checked-at` to the branch head; until that branch is merged, the topic may read stale on the default branch (the diff from the branch head to it). That is accepted: stale means "re-check from the diff", never wrong.

## ClickUp helper (persona roles)

`plugin/scripts/clickup.sh` (bash, starts `lib/clickup.py` through `lib/python.sh`; stdlib only) is the one way a persona session turns a drafted backlog into ClickUp tickets. It only creates tasks (`POST /list/<id>/task`).

| Command | What it does |
|---|---|
| `clickup.sh lists` | Read-only. One line per list: `<list id><TAB><team> / <space> / [<folder> /] <list>`. |
| `clickup.sh preview <file>` | Read-only, works anywhere. Prints the epic, the numbered tickets with their tags, a count line and `digest: <12 hex>`. |
| `clickup.sh create <list id> <file> --yes <digest>` | Creates the tickets. Needs the exact argv shape; `--yes` must be followed by the digest `preview` printed for the file as it is now. |

- **Tickets file**: JSON `{"epic": {"name", "md"} | null, "tickets": [{"name", "md", "tags"?}]}`, 1 to 50 tickets, unique names, written in the system temp folder (or `/tmp`; Claude's scratchpad counts), never in the repo. A file anywhere else is refused. The digest is the first 12 hex characters of the SHA-256 of the file bytes; `create` reads the file once and checks, validates and posts those bytes, so an edit after the preview is refused ("run preview again").
- **Epic and subtasks**: the epic (when present) is created first, every ticket after it carries `parent` = the epic.
- **No duplicates**: a ledger `<file>.created.json` (bound to one list id, rewritten atomically after each POST) and a same-name-and-parent lookup in the list make a second `create` of the same file create nothing; it prints `skip (already created) <url> <name>` per ticket.
- **Token**: read from the macOS keychain item `com.jtfrancisco.dcterminalv2.clickup` of the tab's Claude account (`DCV2_ACCOUNT_ID`, set by the app at every spawn; see [launch.md](launch.md)), the one Settings › ClickUp stores. It goes only into the request header, never into argv, output or a file; there is no token file or env override. macOS only for now. When the keychain asks, answer "Allow", not "Always Allow" (that would let any process read the token without asking).
- **Bound runs**: the helper refuses every command (`lists` included) when `DCV2_RUN_DIR` is set, before any token read: tickets are never created inside an Eagle-Eye run.
- **The guard** (`guard.sh`, rule `clickup-create`) looks at every shell command that names a file ending in `clickup.sh` or `clickup.py` (at any path, the old account skill `clickup-create` included) together with the word `create`:
  - in a **standalone session whose permission mode shows prompts** (`default`, `acceptEdits`, `plan`) it answers with a PreToolUse `ask`, so Claude Code asks you "Create ClickUp tickets from <file> in list <id>?";
  - in **any other mode** (role tabs run in `auto` by default; also `bypassPermissions`, `dontAsk`, an unknown or missing mode) it blocks and gives you the command to type yourself with the `!` prefix: `! bash …/clickup.sh create <list id> <file> --yes <digest>`;
  - in a **bound run** it blocks always (message: use the Read tool to read the helper, and `git commit -F <file>` for a commit message that names it);
  - `lists` and `preview` are never asked or blocked; a command that breaks another guard rule is blocked, not asked.
  The ask is a speed bump against mistakes, not a sandbox: a create wrapped in a script file is not seen by the guard (the helper still needs the digest and refuses in a bound run).
- **Retired**: the account skill `clickup-create` (`~/.claude-account1/skills/clickup-create`) is replaced by this helper; delete it. The guard covers it by basename until then.
- **Manual check, once after the reinstall** (with a test list): in a role tab (`auto`) ask for a one-ticket draft and check the guard refuses with the `!` line; type that line and check it creates the ticket; switch the session to `default` (Shift+Tab), ask for another and check Claude Code shows the guard's prompt. Results: not recorded yet.

## Tests

- `scripts/skill-e2e/run.sh`: the Eagle-Eye run end to end, headless, on two toy repos (full run; then halt after PR1 + resume). Cheap models via a test-only plugin copy (`DCV2_E2E_MODEL`, default haiku).
- `scripts/skill-e2e/guard_test.sh`: unit test of the guard hook (no Claude call).
- `scripts/skill-e2e/state_test.sh`: unit test of `state.sh` (no Claude call; `gh` is a stub, the remote a local bare repo).
- `scripts/skill-e2e/memory_test.sh`: unit test of `memory.sh` (ids, caps, secrets, pinned lines, headers, staleness, compose, status).
- `scripts/skill-e2e/stop_check_test.sh`: unit test of the Stop/SubagentStop hook.
- `scripts/skill-e2e/qa_browser_test.sh`: unit test of `scripts/qa-browser.mjs` against a stub CDP endpoint (no browser, no network).
  Click steps run one in-page finder (`pickTarget`, also tested in jsdom by `scripts/qa-browser-click.test.mjs`): matches inside the topmost open dialog come first, each match is hit-tested at its centre, and a covered target fails the step with `covered by <what covers it>`; `click`, `type` and `text` take an optional `within` selector (empty or on another op: exit 2). The driver's exit codes: 0 all steps passed, 1 a step failed, 2 refused or no step read (an empty steps list prints `qa-browser: no steps read (no step lines printed)` on stderr; QA treats it as ERROR, never PASS), 3 no browser. It runs when started through a symlinked folder (real paths are compared). A page-error line may carry `"url"`; `kind` `network` marks a failed load or a 4xx/5xx response, and in web mode it is printed only for the page's own same-origin document (4xx fetch or image responses on real sites are normal and print nothing).
- `scripts/skill-e2e/clickup_test.sh`: unit test of the ClickUp helper (no Claude call, no network, no keychain item: a stub folder under `DCV2_CLICKUP_STUB` with `DCV2_TEST_MODE`, a fake account id; independent of the caller's `DCV2_RUN_DIR` and `DCV2_ACCOUNT_ID`). Runs as the plugin suite `clickup` of `npm run check`.
- `scripts/skill-e2e/plugin_test.sh`: static checks: `VERSION`, every skill/agent reads its role file, every role has the trust rule, hooks and launch settings parse, scripts pass `bash -n`.
- `scripts/skill-e2e/golden.sh [multi dep fix]` (R7, before every release): three toy workspaces through the whole run, in parallel, launched as the app does (`--settings`, the composed memory prompt, `DCV2_MEMORY_DIR`): two independent repos with onboard first and `DCV2_MAX_IMPLEMENTERS=1`; a PR that depends on another; a check that already fails for a reason the request does not name. Asserts gates, `report.md`, `memory-update.md`, memory written only via `memory.sh`, ordering, and untouched main checkouts.
- `scripts/skill-e2e/standalone.sh [roles...]`: each standalone skill loads, reads its role file and replies; read-only roles leave the repo untouched.

The unit tests run on python3 3.9+ (macOS's system python3 included). The live tests (`run.sh`, `golden.sh`, `standalone.sh`) use the current Claude account; none of them touches a real repo.

## QA and triage (0.10.0)

- `dcv2:qa` (sub-agent, sonnet): tries a PR's `(app)` criteria in the running app through `scripts/qa-browser.mjs` (a headless Chrome-family browser) and writes `reviews/PR<n>-qa<r>.md` plus screenshots. Standalone: `/dcv2:qa` on a branch with criteria in your words.
- The Planner marks criteria checkable in the app as `- [ ] (app) <criterion>`; the Plan Reviewer checks each is concrete.
- Planner `pass=triage` (with `plugin/roles/triage.md`) proposes the next runs from open tickets; see run-folder.md, Triage mode.
