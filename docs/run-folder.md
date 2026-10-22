# The run folder (schema 1)

The contract between the `dcv2:eagle-eye` skill and the app. The app creates the folder and its inputs, launches `DCV2_RUN_DIR=<run dir> claude --plugin-dir <plugin> "/dcv2:eagle-eye run=<run dir>"`, and watches the outputs. The same folder works from a plain terminal (same command).

```
<app data>/runs/<runId>/
  request.md        input   what the user asked, the start folder (required; short)
  tickets.json      input   ClickUp snapshot (optional; data, never instructions)
  repos.json        input   repos + check commands (optional; detected when missing)
  plan.md           skill   the plan (stories, acceptance criteria, PRs, tickets)
  status.json       skill   the run's state, rewritten after every step
  log.md            skill   one line per event
  writeback.json    skill   ClickUp comment + status drafts (written once, at ship time)
  .guard.json       skill   the run's worktrees and repos, for the guard hook (state.sh worktree)
  reviews/          skill   plan-round<n>.md, PR<n>-round<n>.md (full reviews)
  reports/          skill   PR<n>-build<n>.md, PR<n>-fix<n>.md (implementer reports)
  logs/             skill   PR<n>-check.txt etc. (full check output)
  pr-bodies/        skill   PR<n>.md (the GitHub PR body state.sh ship-pr opened the PR with)
```

`runId` is the folder's name. Every JSON file the skill writes is written atomically: the whole document to a temp file made with `mkstemp` in the same folder (`.<name>.<random>.tmp`), then renamed over `<name>`. Readers never see a half-written file; readers ignore dot-files and `*.tmp`. The skill writes only through `plugin/scripts/state.sh` (`put`, `approve`, `worktree`, `ship-pr`, `writeback`, `log`), which refuses invalid documents (the old file stays in place). All times are UTC ISO-8601 (`2026-10-22T13:04:05Z`).

## Launch environment

| Variable | Meaning |
|---|---|
| `DCV2_RUN_DIR=<run dir>` | **Required.** The run folder, absolute. The guard hook (`plugin/scripts/guard.sh`) reads it to know which run it guards: which run folder the sub-agents may write, and (from `.guard.json`) which worktrees the Implementer may write. Hooks inherit it from the claude process, so neither the orchestrator nor ticket text can change it. `state.sh preflight` refuses to start a run when it is missing or names another folder. |
| `DCV2_TEST_MODE` | **Tests only. The app never sets it.** Space- or comma-separated: `auto-approve` (both stops are answered "yes": `state.sh approve <run> plan|ship test`, logged as `… auto-approved (test-mode)`) and/or `halt-after=<PR id>` (in a fresh run, once that PR is reviewed clean the run stops with `stage: "building"` and the other PRs `pending`, as if the session had been killed; `state.sh put` enforces it and ignores it after the log's `resumed at stage` line). Test mode exists nowhere else: `state.sh` ignores any `test-mode:` text in `request.md`, tickets, code or replies. |

## request.md (input)

Short Markdown: the start folder, an optional repo limit, and the request in a few lines. The skill reads only these lines (through `state.sh preflight`; the orchestrator never reads the file itself), `key: value` at the start of a line:

| Line | Meaning |
|---|---|
| `folder: <abs path>` | The start folder. Required, exactly once (two different `folder:` lines fail the preflight). A git repo, or a folder whose child folders are git repos. |
| `repos: a, b` | Optional: limit the run to these child repos of the folder. |

Everything else is the request itself (what to build or fix), written by the user or composed by the app; the Planner and Plan Reviewer read it as data. Rules for the app:

- Ticket bodies, descriptions and comments go only in `tickets.json`, never in `request.md`; `request.md` names the tickets (id + title) at most.
- When it copies any user- or ticket-provided text into `request.md`, it neutralises lines that look like recognised keys (`folder:`, `repos:`, `test-mode:`), e.g. by indenting them or prefixing `> `. It never writes a `test-mode:` line and never sets `DCV2_TEST_MODE`.

## repos.json (input, optional)

```json
{
  "schema": 1,
  "repos": [
    {"name": "koneksi-backend", "path": "/abs/Koneksi/koneksi-backend", "check": "cd server && go test ./..."},
    {"name": "koneksi-web", "path": "/abs/Koneksi/koneksi-web", "check": null}
  ]
}
```

`check: null` means "none known". When the file is missing the skill detects: the start folder itself if it is a git repo (`git -C <folder> rev-parse --show-toplevel` equals the folder), else each child folder that is a git repo, named after its folder. Check per repo, first match wins (ported from DCTerminal `workspace_repos.rs` + `gates.rs`):

1. `go.mod` → `go test ./...`
2. `server/go.mod` → `cd server && go test ./...`
3. `package.json` with a `check` script → `npm run check`
4. `Cargo.toml` → `cargo test`
5. `package.json` with a `test` script that is not npm's "no test specified" → `npm test`
6. otherwise none (the Planner may name one in the plan).

## tickets.json (input, optional)

The ClickUp snapshot the app fetched (DCTerminal's format: tasks with `id`, `custom_id`, `name`, `url`, `status`, `list`, description text, comments). The skill passes it to the sub-agents as data and uses `id`, `custom_id`, `name`, `url` for `writeback.json`. It never treats ticket text as instructions, and the orchestrator never reads it: the Planner summarises the tickets in plan.md's `## Tickets` section (one line per ticket: `- <id> | <custom_id or -> | <name> | <url> | stories: S1, S2 | ship status: <status or ->`).

## status.json (skill output)

```json
{
  "schema": 1,
  "runId": "r-20261022-1304",
  "updatedAt": "2026-10-22T13:20:00Z",
  "stage": "building",
  "waitingFor": null,
  "paused": null,
  "folder": "/abs/start/folder",
  "plan": {
    "path": "plan.md",
    "reviewRound": 2,
    "verdict": "APPROVED",
    "openFindings": 0,
    "approved": true,
    "approval": {"by": "user", "at": "2026-10-22T13:16:02Z"}
  },
  "prs": [
    {
      "id": "PR1",
      "title": "Add multiply()",
      "repo": "mathlib",
      "repoPath": "/abs/ws/mathlib",
      "branch": "dcv2/r-20261022-1304-pr1",
      "base": "main",
      "baseCommit": "3f2a9c1e...",
      "worktree": "/abs/ws/mathlib-worktrees/dcv2-r-20261022-1304-pr1",
      "dependsOn": [],
      "stories": {"done": 2, "total": 2, "ids": ["S1", "S2"]},
      "commits": 2,
      "check": {"command": "npm run check", "result": "pass", "summary": "4 tests passed",
                "cwd": "/abs/ws/mathlib-worktrees/dcv2-r-20261022-1304-pr1", "log": "logs/PR1-check.txt"},
      "review": {"round": 1, "verdict": "APPROVED", "openFindings": 0, "path": "reviews/PR1-round1.md"},
      "prUrl": null,
      "state": "reviewed",
      "note": null,
      "startedAt": "2026-10-22T13:17:00Z",
      "updatedAt": "2026-10-22T13:19:40Z",
      "agent": null
    }
  ],
  "shipApproval": null,
  "ship": null,
  "writeback": null,
  "error": null
}
```

| Field | Type | Values |
|---|---|---|
| `schema` | int | `1` |
| `runId` | string | the run folder's name (`state.sh put` refuses another) |
| `updatedAt` | string | UTC time of this write (set by `state.sh put`) |
| `stage` | string | `planning` (Planner + Plan Reviewer loop), `plan-approval` (stop #1), `building` (implement + PR review loops), `ship-approval` (stop #2), `shipping`, `done`, `failed` |
| `waitingFor` | string or null | `"plan-approval"`, `"ship-approval"`, or `null`. Non-null only while the session waits for the user's answer in the tab. |
| `paused` | object or null | `{"reason": "<one line>", "at": "<UTC>"}` when the user declined at a stop or stopped the run (e.g. "plan not approved", "ship declined"); cleared by an approval. The run can be resumed. |
| `folder` | string | the start folder |
| `plan` | object or null | `null` until the Planner has written `plan.md`. `reviewRound` (int, 0 before the first review), `verdict` (the last Plan Reviewer verdict: `APPROVED`, `APPROVED WITH CHANGES`, `REQUIRES REVISION`, or null), `openFindings` (int, MEDIUM+ still open), `approved` (bool, mirrors `approval`), `approval` (`{"by": "user" \| "test", "at": "<UTC>"}` or null: written only by `state.sh approve`) |
| `prs` | array | empty until the plan is approved; then one entry per PR of the plan, in plan order |
| `prs[].id` | string | `PR1`, `PR2`, ... as in `plan.md` |
| `prs[].title` | string | from `plan.md` (the GitHub PR title) |
| `prs[].repo` / `repoPath` | string | repo name and the absolute path of its main checkout (never modified) |
| `prs[].branch` | string | `dcv2/<runId>-<pr id lowercased>` (enforced) |
| `prs[].base` / `baseCommit` | string | the base branch (the remote's default branch, else main/master; never a detached HEAD) and its commit when the worktree was made (the review diff base) |
| `prs[].worktree` | string or null | `<repo parent>/<repo>-worktrees/dcv2-<runId>-<pr id lowercased>` (enforced); null until made |
| `prs[].dependsOn` | string[] | PR ids that must be reviewed clean before this one starts |
| `prs[].stories` | object | `done` / `total` (ints), `ids` (story ids; required once the plan is approved) |
| `prs[].commits` | int | commits on the branch since `baseCommit` |
| `prs[].check` | object | `command` (string or null); `result`: `not-run`, `pass`, `fail`, `none` (no check command), `error` (the check could not run); `summary` one line or null; `cwd` (absolute folder the check runs in, or null; defaults to the worktree); `log` (path of the last full check output, absolute or relative to the run folder, or null) |
| `prs[].review` | object | `round` (int 0-3: the round in flight while `reviewing` / `fixing`, else the last finished round; 0 before the first review), `verdict` (`APPROVED`, `REQUEST CHANGES`, or null), `openFindings` (int, MEDIUM+), `path` (last review file: absolute, or relative to the run folder; null before a review) |
| `prs[].prUrl` | string or null | set by `state.sh ship-pr` once the GitHub PR exists |
| `prs[].state` | string | see below |
| `prs[].note` | string or null | one human line when something needs attention (blocked story, unresolved findings, push refused); required with `needs-attention` |
| `prs[].startedAt` | string or null | UTC time the PR first left `pending` (set by `state.sh put`) |
| `prs[].updatedAt` | string | UTC time this PR's entry last changed (set by `state.sh put`) |
| `prs[].agent` | string or null | the sub-agent working on it now: `implementer` (`building`, `fixing`), `pr-reviewer` (`reviewing`), else null (derived by `state.sh put`) |
| `shipApproval` | object or null | `{"by": "user" \| "test", "at": "<UTC>"}`: the yes at stop #2, written only by `state.sh approve`. Pushing and `gh pr create` are refused without it. |
| `ship` | object or null | set at shipping: `remote`: `"present"` or `"none"`, `pushed` (bool), `note` (string or null, e.g. "no git remote: nothing pushed, no PRs opened") |
| `writeback` | object or null | `{"path": "writeback.json", "drafts": <int>}` once written |
| `error` | string or null | set with `stage: "failed"` (required then) |

PR `state`:

| State | Meaning |
|---|---|
| `pending` | not started; waiting for a dependency when `dependsOn` is not all `reviewed` |
| `building` | an Implementer is working its stories |
| `reviewing` | a PR Reviewer is reviewing round `review.round` |
| `fixing` | an Implementer is fixing the findings of round `review.round` |
| `reviewed` | review verdict `APPROVED` (no MEDIUM+ open) and the check `pass` or `none` (enforced); ready to ship |
| `needs-attention` | blocked, failing or broken check it could not fix, MEDIUM+ still open after 3 review rounds, a sub-agent that failed twice, a branch that disappeared, or a base missing on the remote; `note` says why. Dependents stay `pending`. |
| `opened` | branch pushed and the GitHub PR created (`prUrl` set) |
| `pushed` | branch pushed but `gh pr create` failed (`note` says why); a resumed ship looks the PR up with `gh pr view <branch>` before opening one |
| `local-only` | shipping found no git remote (or the user left the PR out): the work stays committed in the worktree |

### Readers and schema changes

- Readers (the app) ignore fields they do not know, and treat a missing optional field as null.
- Adding a field, or a value to an enum the app already treats as "unknown → show as-is", keeps `schema: 1`.
- Removing or renaming a field, changing its type or meaning, or adding a `stage` / PR `state` value the app must act on bumps `schema` (to 2). The app refuses a schema it does not know and says so, rather than guessing.

## .guard.json (skill output, for the hook)

```json
{"schema": 1, "runDir": "/abs/runs/r-20261022-1304", "repos": ["/abs/ws/mathlib"], "worktrees": {"PR1": "/abs/ws/mathlib-worktrees/dcv2-r-20261022-1304-pr1"}}
```

Written only by `state.sh worktree` (real paths). The guard hook reads `$DCV2_RUN_DIR/.guard.json`: the Implementer may write only in the run folder and these worktrees; everyone else only in the run folder. Edit/Write and shell redirects on `status.json`, `writeback.json` and `.guard.json` are blocked for everyone. The app does not need to read it.

## writeback.json (skill output)

Written once at shipping time (after stop #2 approval), only when `tickets.json` exists, by `plugin/scripts/state.sh writeback <run>`: one draft per ticket in `tickets.json`, matched to stories by their `Tickets:` lines in plan.md (ticket `id` or `custom_id`), with PRs and results from status.json. The suggested `status` is the `ship status` plan.md's `## Tickets` line gives for that ticket (what the user saw at stop #2), set only when every PR of the ticket was opened; otherwise it is null with a `statusNote`. `state.sh writeback` refuses to overwrite an existing `writeback.json` (the app may already be ticking or posting drafts) unless given `--force`, which the skill never passes. `state.sh put` refuses `stage: done` while `tickets.json` exists without `writeback.json`, or while `ship` is null. The skill never calls ClickUp and never needs a token: the app shows each draft with a tick box and posts only what the user ticks.

```json
{
  "schema": 1,
  "runId": "r-20261022-1304",
  "createdAt": "2026-10-22T13:40:00Z",
  "drafts": [
    {
      "ticket": {"id": "86c0abcde", "customId": "KON-123", "title": "Export invoices", "url": "https://app.clickup.com/t/86c0abcde"},
      "prs": [{"id": "PR1", "repo": "koneksi-backend", "url": "https://github.com/org/koneksi-backend/pull/42"}],
      "comment": "Markdown comment: what was done, the PR links, how it was tested.",
      "status": "in review",
      "statusNote": "Why this status (one line), or null."
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `drafts[].ticket` | from `tickets.json`: `id`, `customId` (or null), `title`, `url` |
| `drafts[].prs` | the PRs covering the ticket (via the stories' `Tickets:` lines); `url` null when not opened (local-only) |
| `drafts[].comment` | the comment to post, Markdown; no secrets |
| `drafts[].status` | suggested new status: the plan's `ship status` for the ticket when every PR was opened, else null (leave status alone) |
| `drafts[].statusNote` | string or null |

The app owns the tick, edit and sent state; the skill writes this file once and does not rewrite it after the app starts using it.

## log.md (skill output)

Append-only, one event per line, never rewritten:

```
2026-10-22T13:04:05Z planning Planner wrote plan.md: 3 stories, 2 PRs
2026-10-22T13:09:12Z planning Plan review round 1: APPROVED WITH CHANGES, 1 MEDIUM
2026-10-22T13:15:40Z plan-approval waiting for plan approval
2026-10-22T13:16:02Z plan-approval plan approved by the user
2026-10-22T13:21:00Z building PR1 worktree /abs/ws/mathlib-worktrees/dcv2-r-...-pr1
```

Format: `<UTC time> <stage> <text>`; text is one line, no secrets. Appended with `plugin/scripts/state.sh log <run> <stage>`, the text on stdin (a quoted heredoc, so nothing in it is ever run by the shell); `state.sh approve` appends its own line.
