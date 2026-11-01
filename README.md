# DCTerminalv2

A macOS and Windows terminal app for [Claude Code](https://docs.claude.com/en/docs/claude-code). Open Claude in tabs, one Claude account per window, and let **Eagle-Eye** plan, build, review and ship a change for you, asking before anything leaves your machine.

Built with Tauri 2 (Rust in `src-tauri/`, React + xterm.js in `src/`) and the bundled `dcv2` Claude Code plugin (`plugin/`).

## What you get

| | |
|---|---|
| **Terminal** | Tabs and split panes, search, clickable links and file paths, command palette (Cmd+K), prompt scratchpad |
| **Eagle-Eye runs** | Plan → you approve → parallel builds in git worktrees → review → you approve → PRs opened |
| **Run panel** | Stage, per-PR progress, checks, review gates, cost and time, timeline, rendered plan and reviews |
| **ClickUp** | Paste ticket or epic links to start a run; post comments back with one click |
| **Notifications** | Per-tab "needs you" state, desktop notifications, dock badge (macOS), phone push (ntfy, Pushover, webhook) |
| **Role skills** | `/dcv2:planner`, `/dcv2:developer`, `/dcv2:pr-reviewer`, `/dcv2:codebase-audit` and more, each usable on its own |
| **Safety** | A guard keeps agents inside their worktree, nothing is pushed before your approval, secrets stay in the keychain |

## Requirements

**macOS**
- macOS 10.15 or later (`install.sh` refuses other systems)
- **Claude Code CLI** (`claude`), **git**, **Node.js and npm** (Node 18+ for BMAD planning)
- **Rust** stable, with `clippy` and `rustfmt` (pinned in `rust-toolchain.toml`)
- **python3** 3.9+ (the macOS system one is fine)

**Windows 10 / 11**
- **Git for Windows** (the app and the plugin run their scripts with its Git Bash)
- **Python 3.8+** (`python3`, `python` or `py -3` on the PATH; the Microsoft Store stub does not count)
- **Claude Code CLI** (`claude`), **Node.js and npm**, and **Rust** stable with the MSVC or GNU target to build from source

**Both:** the **GitHub CLI `gh`** is optional, for opening PRs, GitHub review mode, CI status and merged-run cleanup.

## Install

```sh
git clone https://github.com/jtfrancisco0121/DCTerminalv2.git
cd DCTerminalv2
./install.sh            # build, then install
./install.sh --no-build # install the last build
```

What `install.sh` does:

1. Runs `npm ci`, builds the app (`npx tauri build --bundles app`) and the `dcv2-hook` forwarder.
2. Checks the bundle has the plugin and the forwarder, and re-signs it ad hoc if needed.
3. Refuses while DCTerminalv2 is running. Quit it with **Cmd+Q** first.
4. Copies the app to `/Applications` (or `~/Applications`), keeping the old copy until the new one is in place.
5. Removes the quarantine flag (local install, not notarized).

For a DMG, run `npm run release:mac`.

### Windows

```powershell
git clone https://github.com/jtfrancisco0121/DCTerminalv2.git
cd DCTerminalv2
powershell -ExecutionPolicy Bypass -File install.ps1
```

`install.ps1` refuses non-Windows systems and refuses while DCTerminalv2 is running, runs `npm ci` and `npm run release:win` (an NSIS installer that also carries `dcv2-hook.exe`), then runs the installer silently for the current user. The installer is **unsigned**: Windows SmartScreen may say "Windows protected your PC". Choose **More info**, then **Run anyway**.

Known limits on Windows: no dock badge and no native menu bar (the shortcuts are `Ctrl+Shift+<key>`), and the installer is unsigned.

## Using it

1. Open **DCTerminalv2** from Applications.
2. Pick the Claude account for the window. Add accounts in **Settings**.
3. On the start screen, choose a folder, then:
   - **Eagle-Eye** to start a run: describe the change, or paste ClickUp links.
   - A **role tile** (Planner, Developer, PR Reviewer, …) for a single task.
   - A plain **Claude** or **shell** tab.
4. In a run, answer in the run's tab at the two stops: **approve the plan**, then **approve shipping**.
5. Find past and active runs in the **Runs** view, or the status bar's Active runs.

### How a run flows

```mermaid
flowchart LR
    P[Plan] --> R{Plan + risk review}
    R -- findings --> P
    R --> S1([Stop 1: approve plan])
    S1 --> B[Build each PR in its own worktree]
    B --> V{PR review}
    V -- fix --> B
    V --> S2([Stop 2: approve ship])
    S2 --> SH[Push + open PRs]
    SH --> D([Done: report])
```

A stopped run can be resumed. Details are in [docs/run-folder.md](docs/run-folder.md).

## Development

```sh
npm install
cargo build --manifest-path src-tauri/Cargo.toml   # once: builds dcv2-hook
npm run dev:app                                      # tauri dev
```

- Debug builds use their own data folder (`com.jtfrancisco.dcterminalv2.dev`) and the repo's `plugin/`.
- The plugin also works in a plain terminal: `claude --plugin-dir plugin "/dcv2:planner <request>"`.
- **The gate:** `npm run check` runs rustfmt, clippy, cargo tests, Vitest, the build and the plugin test suites. There is no hosted CI. On a Mac, `brew install mingw-w64` adds a step that compiles every Windows-only branch (`cargo clippy --target x86_64-pc-windows-gnu`); without it the gate prints `skip: Windows cross-clippy (brew install mingw-w64)` and carries on. On Windows the gate runs the plugin suites under Git Bash.
- Live plugin tests that call Claude live in `scripts/skill-e2e/` (`run.sh`, `golden.sh`, `standalone.sh`, `bmad_spike.sh`).

## Repository layout

| Path | Contents |
|---|---|
| `src/` | React UI |
| `src-tauri/src/` | Rust backend: terminals, runs, observation, ClickUp, storage |
| `plugin/` | The `dcv2` plugin: skills, agents, roles, scripts, hooks, vendored BMAD |
| `scripts/` | `local-check.mjs` (the gate), plugin tests, `verify-win-bundle.mjs` |
| `install.sh`, `install.ps1` | The macOS and Windows installers |
| `docs/` | App ↔ plugin contracts and dated plans |

## Docs

- [docs/launch.md](docs/launch.md): how the app launches Claude and observes it
- [docs/run-folder.md](docs/run-folder.md): the run folder contract and run rules
- [docs/skills.md](docs/skills.md): the plugin, roles, memory and guard
- [docs/plans/](docs/plans/): dated design plans
