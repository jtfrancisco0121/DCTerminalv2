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
- `DCV2_TAB_ID` / `DCV2_HOOK_PORT` / `DCV2_HOOK_TOKEN` (A2) are not set yet.
- A tab resumed with `claude --resume` gets the same env and flags but no prompt; an Eagle-Eye tab started fresh again on a run that has a `status.json` sends `/dcv2:eagle-eye resume run=<dir>` instead of `run=`.
- The app drops an inherited `DCV2_TEST_MODE` from every dcv2 tab.
- The plugin folder: `DCTV2_PLUGIN_DIR`, else (debug builds) the repo's `plugin/`, else the bundled resource `<resources>/plugin`.
