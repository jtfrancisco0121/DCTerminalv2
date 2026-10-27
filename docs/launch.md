# Launch contract (app ↔ plugin)

How the app (P3) starts a Claude session with the dcv2 plugin, and the files the plugin (P2b) provides for it. Both sides build against this page; change it first when the contract changes.

## Command

```
cd <start folder>
env CLAUDE_CONFIG_DIR=<account dir> \
    DCV2_RUN_DIR=<run dir>                # Eagle-Eye only
    DCV2_MEMORY_DIR=<app data>/memory     # always
    DCV2_MAX_IMPLEMENTERS=<n>             # Eagle-Eye only; default 3 (setting)
    DCV2_STOP_HOOKS=on|off                # A3 kill switch; default on (setting)
    DCV2_TAB_ID=<tab id> DCV2_HOOK_PORT=<port> DCV2_HOOK_TOKEN=<per-launch token>   # A2 forwarder (P3)
  claude --plugin-dir <plugin> \
         --settings <launch settings file> \
         --append-system-prompt-file <memory prompt file> \
         [--model <m>] \
         "/dcv2:eagle-eye run=<run dir>"   |  "/dcv2:eagle-eye resume run=<run dir>"  |  "/dcv2:<role> <request>"
```

`<plugin>` is the plugin folder shipped in the app bundle (dev: `<repo>/plugin`).

## Files the plugin provides (P2b)

| File | Use |
|---|---|
| `plugin/VERSION` | One line: the roles' version (A5). The app shows it and records it in each run's `status.json`-adjacent `meta.json` (P3). |
| `plugin/launch/settings.json` | A4 token savers: `env` (`BASH_MAX_OUTPUT_LENGTH`, `CLAUDE_CODE_SUBAGENT_MODEL` only if the agents don't name one, autocompact threshold), `permissions.deny` (`node_modules/**`, lockfiles, build output). The app merges its own hook-forwarder `hooks` into a copy (per launch, in the run folder or a temp dir) and passes that copy with `--settings`. Never written into repos or `~/.claude*`. |
| `plugin/scripts/memory.sh` | The only code that knows the memory layout (D9). Commands below. |
| `plugin/skills/onboard/SKILL.md` | `/dcv2:onboard` (seventh role tile). |

### memory.sh

All commands read `DCV2_MEMORY_DIR` (or `--root <dir>`), print nothing secret, exit non-zero with one stderr line on error.

| Command | Output |
|---|---|
| `memory.sh id <repo path>` | The repo id: from `origin`'s URL (`host__owner__name`, lowercased, `.git` dropped), else `local__<folder name>-<first 8 hex of sha1(abs path)>`. Stable across accounts and clones. |
| `memory.sh compose <start folder> <out file>` | Writes the system-prompt file for a launch: the trust rule, the memory root path, `workspace.md` (folder of repos) + contracts, each in-play repo's `index.md`, and a list of stale topics. Always succeeds when the folder exists; with no memory it writes a short "no memory yet, `/dcv2:onboard` creates it" note. Bounded size (caps below). |
| `memory.sh status <start folder>` | JSON for the app panel: `{"schema":1,"workspace":{"present":bool,"path":..}|null,"repos":[{"name","path","id","present","head","topics":[{"name","checkedAt","stale":bool}],"staleCount","indexBytes"}]}` |
| `memory.sh write <repo id or "workspace:<id>"> <relative file> < content` | The only write path for agents: enforces size caps, secret scan, pinned lines (`<!-- pinned -->` lines cannot be removed or changed by an automatic write), topic header. Rejects with a reason. |
| `memory.sh scan < text` | The secret scanner alone: `clean` (exit 0), or `looks like a secret on line <n> (<kind>)` (exit 1), never the value. `state.sh` runs it on every GitHub post (fails closed when it cannot run). |

The guard blocks Edit/Write into `DCV2_MEMORY_DIR` (agents use `memory.sh write`). The app's own memory editor (P3) writes files directly, as the user.

## Run folder additions (P2b)

- `meta.json` gains `"reviewMode": "github" | "local"` (app-written at Start from the start-screen toggle "Review on GitHub (draft PRs)"; guard-protected; missing = `local`). The plugin reads it only through `state.sh mode` / `preflight` and branches on it (run-folder.md, Review mode).
- `meta.json` gains `"bmad": {"mode": "auto" | "on" | "off", "version": "<plugin/bmad/VERSION line>"}` (app-written at Start from the start-screen control "BMAD planning (BMAD-METHOD skills)"; guard-protected; missing = `off`). `state.sh preflight` prints `bmad=auto|on|off` and `node=<version>|missing` (BMAD's runtime is Node 18+; auto/off runs never need it). The plugin reads the mode only through `state.sh` (run-folder.md, BMAD mode).
- `repos.json` repos gain `"setup": "<command>" | null` and `"copyFiles": ["relative/path", …]` (app-written; run-folder.md, repos.json): `state.sh worktree` copies the git-ignored files and runs the setup in newly made worktrees, output in `logs/<PR>-setup.txt`.
- status.json PRs gain `draft` (bool) and `comments` (`[{"round", "url"}]`) and the state `drafted` (github mode); `opened` means ready for review. Schema stays 1 (optional fields).
- `pr-comments/<PR>-round<n>.md`: what was posted (github mode).
- Guard (0.3.0): the orchestrator runs no GitHub-writing `gh` at all (`gh api` with a non-GET method or `-f/-F/--field/--raw-field/--input`, `gh <x> create|comment|edit|ready|…`); in github mode it pushes and posts only through `state.sh` (after the plan approval; `ready` after the ship approval); `security dump-keychain` and `security find-*-password … dcterminalv2…` are blocked for everyone in a run.
- `gates/<PR id>.yml` (A1): written by the PR Reviewer step through `state.sh gate`: `gate: PASS|CONCERNS|FAIL|WAIVED`, `reason: <one line>`, `updated: <UTC>`, `review: <review file>`. A PR is green in the app only when the gate is PASS or WAIVED **and** the app's own check re-run (R1) passes.
- `report.md` (R10): written at `done`/`failed`: planned vs built per PR, review rounds, retries, gate per PR, check results.
- `memory-update.md`: what the end-of-run memory update changed (or "no topic touched").
- **AFK runs (0.5.0, plan 2026-10-27).** `requests/<id>.json` (written by `state.sh request <run> recheck|cleanup|tickets|selftest`, `{"schema":1,"id","kind","at"}`, id `<YYYYMMDDTHHMMSSZ>[-n]-<kind>`) asks the app for one of its fixed actions; the app answers in `requests/<id>.result.json` (`{"schema":1,"id","kind","state":"running|done|refused|ignored","ok","message","at"}`) and an `app:` line in `log.md`; `state.sh request-wait <run> <id> [--timeout <s>]` waits for it. `app-cleanup.json` is the app's record of merged-PR cleanups. `state.sh cleanup <run>` (done / failed only) removes the run's clean worktrees, `git branch -d`, and merged PRs' remote branches. repos.json repos may carry `"liveTests": true`. Guard: the orchestrator runs no raw `git worktree …` / changing `git branch` / `git push --delete`; `requests/` and `app-cleanup.json` are guard-protected; a pipeline sub-agent never runs `claude`, and `scripts/skill-e2e/golden.sh` / `bmad_spike.sh` only as the Implementer or PR Reviewer in its worktree of a `liveTests` repo. Details: run-folder.md, "AFK runs".

## What the app reads

`status.json`, `log.md`, `gates/*.yml`, `report.md`, `memory.sh status` output, the session JSONL under `<account dir>/projects/...` (A2, P3), hook-forwarder events, and (0.5.0) `requests/*.json`, of which it uses only the kind.

## App side as built (P3a)

- `--settings`: the app reads `plugin/launch/settings.json` (when present), lays its own keys over a copy (today the usage `statusLine`; the A2 hook-forwarder `hooks` come with A2) and passes the merged JSON inline as one `--settings` value. Nothing is written into the plugin, repos or `~/.claude*`.
- `--append-system-prompt-file`: Eagle-Eye uses `<run dir>/memory-prompt.md`, role tiles `<app data>/launch/<uuid>-memory-prompt.md`; omitted when `memory.sh` is missing or fails.
- `DCV2_TAB_ID` / `DCV2_HOOK_PORT` / `DCV2_HOOK_TOKEN` (A2) are set on every dcv2 tab (as built in P3b, below).
- A tab resumed with `claude --resume` gets the same env and flags but no prompt; an Eagle-Eye tab started fresh again on a run that has a `status.json` sends `/dcv2:eagle-eye resume run=<dir>` instead of `run=`.
- The app drops an inherited `DCV2_TEST_MODE` from every dcv2 tab.
- The plugin folder: `DCTV2_PLUGIN_DIR`, else (debug builds) the repo's `plugin/`, else the bundled resource `<resources>/plugin`.

## Observability as built (P3b)

- **Hook forwarder.** `dcv2-hook` is a second `[[bin]]` of the app crate (`src-tauri/src/bin/dcv2-hook.rs`, std + serde_json only). The app adds `hooks` for `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `SubagentStart`, `SubagentStop`, `Stop`, `Notification`, `PermissionRequest`, `SessionEnd` to the merged `--settings` (appended after the plugin's own hook lists), each running `'<dcv2-hook>'` with a 5 s timeout. The forwarder reads stdin (≤ 1 MiB), keeps only `hook_event_name`, `session_id`, `agent_id`, `agent_type`, `notification_type`, `tool_name`, `source`, `reason` and a count of running `background_tasks` (never the prompt, tool input/output or last message), POSTs it to `127.0.0.1:$DCV2_HOOK_PORT/hook` with `X-Dcv2-Token` / `X-Dcv2-Tab` (300 ms per step), prints nothing and exits 0. A payload cut at the 1 MiB cap still sends the ids read from its start.
- **Where the binary is.** `DCV2_HOOK_BIN`, else `dcv2-hook` beside the app executable: `Contents/MacOS/` in the bundle (Tauri bundles every `[[bin]]` of the crate there, so no `externalBin`/sidecar triple-named copies are needed), `target/<profile>/` in dev. `tauri dev` builds only the app binary; run `cargo build --manifest-path src-tauri/Cargo.toml` once to get `dcv2-hook` (without it a tab simply has no hook state).
- **Which tabs get hooks.** Every Claude tab, not only the tabs dcv2 launches for a run, while the setting `observe.hooksAllTabs` is on (default on; Settings › "Track activity in every Claude tab"). With it off, only launched run tabs get them, as before. A plain tab's hooks only feed its activity dot and usage popover; `with_hooks` merges them into the user's own `--settings`.
- **Listener.** One std-thread HTTP listener on `127.0.0.1:<random port>` per app run. Each launch of a dcv2 tab gets a fresh token (the previous one stops working); a post with another tab's token, an old token or no token gets 403. Connections: at most 32 at once (extras dropped), 2 s for a whole request, 8 KiB of request line + headers. A closed tab's token goes. States: `working` (prompt submitted, tool done, sub-agent started, or `Stop` with background sub-agents still running), `waiting` (`Stop` with none, `idle_prompt`), `needs-you` (`PermissionRequest`, other notifications).
- **Session log (A2).** The session `SessionStart` last named (after `/clear` or a resume), else the tab's saved one: its `<config>/projects/*/<session id>.jsonl` and `<session id>/subagents/agent-<id>.jsonl` (+ `.meta.json` with `agentType`) are tailed from byte offsets; a partial last line waits, a shorter file is read again from 0, usage is deduped by `message.id`. Sub-agents: `Agent`/`Task` `tool_use` → `toolUseResult` (`async_launched` or `completed`, `agentId`) → `<task-notification>` (`<tool-use-id>`, `<status>`) for background ones. Read-only.
- **Cost/time (R9).** Per stage (the `log.md` stage at the message's time, else status.json's stage) and per agent: tokens (input, output, cache writes 5m/1h, cache reads) and an **estimate** at API list prices (`observe/price.rs`, the one table). Minutes are active time: gaps over 5 min between log events are left out.
- **Watchdog (R2).** Setting `run.watchdogMinutes` (default 10, 0 off): no status.json change and no write to any of the session's log files for that long, while `waitingFor` is null, the run is not `done`/`failed`, the tab is not `needs-you`, no tool is running (`PreToolUse` without its `PostToolUse`) and no hook event came in that time → "No progress for N min" with **Nudge** (types `continue from status.json`, then Enter 150 ms later, only on click; refused and hidden while the tab is `needs-you`; with no hook state it asks first) and **Resume** (`resume_run`).
- **Waiting (ticket 14yq6g30vmd).** The same minutes setting: when `waitingFor` is set (counted from the status.json change) or the tab is `needs-you` (counted from the hook state's `activitySinceMs`, set only when the activity changes, so a `SubagentStop` does not restart it) for that long, the panel says "Waiting for you for N min". The verdict is `observe/watchdog.rs` `waiting()`, next to `check()`; it never shows a stall.
- **Context meter (A4).** The session's last main-chain message: input + cache writes + cache reads against the model's window (haiku and 4.x 200K, 5.x 1M, `[1m]` 1M); warns at 60%, never sends keys. The app check log is shown trimmed (ANSI and `\r` redraws stripped, head 40 + error lines + tail 80).
- **Version (R3).** `observe/version.rs`: `CLAUDE_MIN` 2.1.287, `CLAUDE_MAX_TESTED` 2.1.287. Out of range → banner. When no self-test result exists for the detected version, the banner offers a one-click self-test: `claude -p --model haiku --plugin-dir <plugin> --output-format stream-json --verbose --max-turns 4 --no-session-persistence` in a temp folder (own process group, killed whole on timeout) (`CLAUDE_CODE_SUBAGENT_MODEL=haiku`, `DCV2_STOP_HOOKS=off`, 45 s timeout) asking for one foreground `dcv2:plan-reviewer` agent; passes when the init lists the dcv2 plugin and agent, the agent completes and the result says `SELFTEST-DONE`. Stored in `<app data>/claude-selftest.json`.
- **Fixtures (A5).** `src-tauri/src/fixtures/claude-2.1.287/`: hook payloads, a session log with one sub-agent, and the self-test stream, recorded from one haiku run in a toy folder and sanitised (paths → `/work/toy`, `/config`, `/app/plugin`; attachments other than `model`/`date` dropped; signatures blanked). `Notification` and `PermissionRequest` cannot fire under `-p`; their `*.synthetic.json` copies follow the shape recorded on 2.1.299 by ADE. Contract tests parse all of them.

## Attention (tabs that need you)

One backend thread, `observe/attention.rs`, started at app setup (never from a command, so it works with no window open on a panel), decides every notification, in-app notice, badge and reminder.

- **Fast pass, every 2 s**: the open tabs of all windows, their hook state and trackers (the same `observe_tab_now` the run panel uses, so one verdict), and for each run tab the run folder's `status.json`, `app-checks.json`, `app-cleanup.json` and the signed auto-post record.
- **Background pass, every 60 s**: the app's own runs (not adopted) changed within 30 days that have no open run tab: `status.json`, `app-cleanup.json` and the auto-post record only. The cleanup sweep acts on runs hours after their tab was closed, so merged-cleaned (and a late auto-post result) must be seen without a tab.
- **The auto-post record** is read (`clickup::trust::read_autopost`) only when drafts are written and ship is approved (`autopost::may_be_due`), the record file exists and its mtime changed since the last read; the keychain key is read once and cached. A read error is logged once per run and counts as "no change".
- **Events** (edge-triggered, keyed by subject and kind): needs-you, approval-wait, stall, run-done, run-failed, check-failed, autopost-failed (a result that becomes `failed`, per ticket), waiting-reminder (one per episode, after `run.watchdogMinutes`) and merged-cleaned (an `app-cleanup.json` record that becomes final with outcome `cleaned` or `kept`). The first tick after start is silent, a run that first enters a pass is seeded silently, and the same subject and kind is not announced twice within 30 s.
- **Landing.** Desktop notification actions are ignored, so an event notified while no window of the app has focus leaves a target (tab, time). The next app activation within 120 s (a window gaining focus after no window had it for at least 250 ms, or a dock click) selects that tab (`run::focus_tab`). A switch between the app's own windows is faster than 250 ms and never lands. Choosing a tab clears the target.

- **Sinks** (`observe/notify.rs`, `route()`): the in-app notice (an app-wide `attention` event; every window shows it with Go to tab and Dismiss, at most 3, 15 s) is always sent, because macOS shows no banner and ignores the bounce while DCTerminalv2 is the frontmost app; the system notification (tauri-plugin-notification) is always sent; the dock bounce only while no window has focus (critical for needs-you and approval-wait). The dock badge (macOS) is the count of tabs that need you (needs-you, or the run waits at an approval stop), set app-wide on one window and cleared at 0. Settings › Notifications has a master switch (notices, system notifications, badge and bounce) and one switch per kind (`notify` in settings.json, default all on). The plugin reports `granted` on desktop whatever macOS decides, so Settings also shows the "macOS blocks notifications" hint when the test notification is refused. Run events are also logged to the run's `log.md` (`app: attention …`).

## Import from the dev app (release builds)

Debug builds keep their data in `<identifier>.dev`, release builds in `<identifier>`. On first launch (offered once; `dev-import.json` records it) and from File › "Import from dev app…" (hidden in debug builds) the built app imports the dev app's session, reading `.dev` only:

- **Windows and tabs**: each dev window (open or closed) comes in as a closed window on its account (dev accounts matched to this app's by config folder, else added; a window whose account could not be added is skipped), reopened by you from File › Reopen Closed Window; its open tabs and the recently closed ones keep `claudeSessionId`, `cwd` and `claudeConfigDir`, so reopening a tab runs `claude --resume <id>` in the same folder and account. Tabs whose session is already here are skipped (a second import adds only new ones).
- **Launch re-pointed**: `--plugin-dir` → this app's plugin, `--settings` → this plugin's `launch/settings.json`, `--append-system-prompt-file` → a new file from `memory.sh compose` in `<app data>/launch/`, `DCV2_MEMORY_DIR` → this app's memory. `DCV2_RUN_DIR`, `DCV2_MAX_IMPLEMENTERS`, `DCV2_STOP_HOOKS` and the prompt are kept.
- **Runs** are adopted in place (`adopted-runs.json`, see run-folder.md); **memory** is copied when this app has none.
- **Refused while the dev app is live**: its `app.lock` is held (every build now flocks `<app data>/app.lock` for its whole life, pid inside), a `claude` process runs one of its tabs' sessions (`ps`), or a session log was written in the last 2 min → "Quit the dev app first".
- Resuming an adopted run's tab is refused while another `claude` process runs that session.
- Keychain items (ClickUp tokens, the trust key) have the same service names in both builds; macOS asks once to let the built app use them.
