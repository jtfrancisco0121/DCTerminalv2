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

The guard blocks Edit/Write into `DCV2_MEMORY_DIR` (agents use `memory.sh write`). The app's own memory editor (P3) writes files directly, as the user.

## Run folder additions (P2b)

- `gates/<PR id>.yml` (A1): written by the PR Reviewer step through `state.sh gate`: `gate: PASS|CONCERNS|FAIL|WAIVED`, `reason: <one line>`, `updated: <UTC>`, `review: <review file>`. A PR is green in the app only when the gate is PASS or WAIVED **and** the app's own check re-run (R1) passes.
- `report.md` (R10): written at `done`/`failed`: planned vs built per PR, review rounds, retries, gate per PR, check results.
- `memory-update.md`: what the end-of-run memory update changed (or "no topic touched").

## What the app reads

`status.json`, `log.md`, `gates/*.yml`, `report.md`, `memory.sh status` output, the session JSONL under `<account dir>/projects/...` (A2, P3), and hook-forwarder events.

## App side as built (P3a)

- `--settings`: the app reads `plugin/launch/settings.json` (when present), lays its own keys over a copy (today the usage `statusLine`; the A2 hook-forwarder `hooks` come with A2) and passes the merged JSON inline as one `--settings` value. Nothing is written into the plugin, repos or `~/.claude*`.
- `--append-system-prompt-file`: Eagle-Eye uses `<run dir>/memory-prompt.md`, role tiles `<app data>/launch/<uuid>-memory-prompt.md`; omitted when `memory.sh` is missing or fails.
- `DCV2_TAB_ID` / `DCV2_HOOK_PORT` / `DCV2_HOOK_TOKEN` (A2) are set on every dcv2 tab (as built in P3b, below).
- A tab resumed with `claude --resume` gets the same env and flags but no prompt; an Eagle-Eye tab started fresh again on a run that has a `status.json` sends `/dcv2:eagle-eye resume run=<dir>` instead of `run=`.
- The app drops an inherited `DCV2_TEST_MODE` from every dcv2 tab.
- The plugin folder: `DCTV2_PLUGIN_DIR`, else (debug builds) the repo's `plugin/`, else the bundled resource `<resources>/plugin`.

## Observability as built (P3b)

- **Hook forwarder.** `dcv2-hook` is a second `[[bin]]` of the app crate (`src-tauri/src/bin/dcv2-hook.rs`, std + serde_json only). The app adds `hooks` for `SessionStart`, `UserPromptSubmit`, `PostToolUse`, `SubagentStart`, `SubagentStop`, `Stop`, `Notification`, `PermissionRequest`, `SessionEnd` to the merged `--settings` (appended after the plugin's own hook lists), each running `'<dcv2-hook>'` with a 5 s timeout. The forwarder reads stdin (≤ 1 MiB), keeps only `hook_event_name`, `session_id`, `agent_id`, `agent_type`, `notification_type`, `tool_name`, `source`, `reason` and a count of running `background_tasks` (never the prompt, tool input/output or last message), POSTs it to `127.0.0.1:$DCV2_HOOK_PORT/hook` with `X-Dcv2-Token` / `X-Dcv2-Tab` (300 ms per step), prints nothing and exits 0.
- **Where the binary is.** `DCV2_HOOK_BIN`, else `dcv2-hook` beside the app executable: `Contents/MacOS/` in the bundle (Tauri bundles every `[[bin]]` of the crate there, so no `externalBin`/sidecar triple-named copies are needed), `target/<profile>/` in dev. `tauri dev` builds only the app binary; run `cargo build --manifest-path src-tauri/Cargo.toml` once to get `dcv2-hook` (without it a tab simply has no hook state).
- **Listener.** One std-thread HTTP listener on `127.0.0.1:<random port>` per app run. Each launch of a dcv2 tab gets a fresh token (the previous one stops working); a post with another tab's token, an old token or no token gets 403. States: `working` (prompt submitted, tool done, sub-agent started, or `Stop` with background sub-agents still running), `waiting` (`Stop` with none, `idle_prompt`), `needs-you` (`PermissionRequest`, other notifications).
- **Session log (A2).** The tab's `<config>/projects/*/<session id>.jsonl` and `<session id>/subagents/agent-<id>.jsonl` (+ `.meta.json` with `agentType`) are tailed from byte offsets; a partial last line waits, a shorter file is read again from 0, usage is deduped by `message.id`. Sub-agents: `Agent`/`Task` `tool_use` → `toolUseResult` (`async_launched` or `completed`, `agentId`) → `<task-notification>` (`<tool-use-id>`, `<status>`) for background ones. Read-only.
- **Cost/time (R9).** Per stage (the `log.md` stage at the message's time, else status.json's stage) and per agent: tokens (input, output, cache writes 5m/1h, cache reads) and an **estimate** at API list prices (`observe/price.rs`, the one table). Minutes are active time: gaps over 5 min between log events are left out.
- **Watchdog (R2).** Setting `run.watchdogMinutes` (default 10, 0 off): no status.json change and no write to any of the session's log files for that long, while `waitingFor` is null and the run is not `done`/`failed` → "No progress for N min" with **Nudge** (types `continue from status.json`, then Enter 150 ms later, only on click) and **Resume** (`resume_run`).
- **Context meter (A4).** The session's last main-chain message: input + cache writes + cache reads against the model's window (haiku and 4.x 200K, 5.x 1M, `[1m]` 1M); warns at 60%, never sends keys. The app check log is shown trimmed (ANSI and `\r` redraws stripped, head 40 + error lines + tail 80).
- **Version (R3).** `observe/version.rs`: `CLAUDE_MIN` 2.1.287, `CLAUDE_MAX_TESTED` 2.1.287. Out of range → banner. When no self-test result exists for the detected version, the banner offers a one-click self-test: `claude -p --model haiku --plugin-dir <plugin> --output-format stream-json --verbose --max-turns 4` in a temp folder (`CLAUDE_CODE_SUBAGENT_MODEL=haiku`, `DCV2_STOP_HOOKS=off`, 45 s timeout) asking for one foreground `dcv2:plan-reviewer` agent; passes when the init lists the dcv2 plugin and agent, the agent completes and the result says `SELFTEST-DONE`. Stored in `<app data>/claude-selftest.json`.
- **Fixtures (A5).** `src-tauri/src/fixtures/claude-2.1.287/`: hook payloads, a session log with one sub-agent, and the self-test stream, recorded from one haiku run in a toy folder and sanitised (paths → `/work/toy`, `/config`, `/app/plugin`; attachments other than `model`/`date` dropped; signatures blanked). `Notification` and `PermissionRequest` cannot fire under `-p`; their `*.synthetic.json` copies follow the shape recorded on 2.1.299 by ADE. Contract tests parse all of them.
