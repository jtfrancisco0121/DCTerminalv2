# Contributing

Thanks for helping. DCTerminalv2 is a Tauri 2 app (Rust in `src-tauri/`, React in `src/`) with a bundled Claude Code plugin in `plugin/`. The layout is in the [README](README.md#repository-layout).

## Build from source

```sh
npm install
cargo build --manifest-path src-tauri/Cargo.toml --bin dcv2-hook   # once: the hook forwarder the app looks for
npm run dev:app                                                     # tauri dev
```

Debug builds keep their data in their own folder (`com.jtfrancisco.dcterminalv2.dev`) and use the repo's `plugin/`. To install a build on your Mac, run `./install.sh`.

## The gate: `npm run check`

Every change passes `npm run check` before it is committed: rustfmt, clippy, cargo tests, Vitest, the production build, and the plugin's hermetic bash suites. On a Mac it also runs the release script's test. **There is no hosted CI and none will be added** (`.github/` holds only issue templates, and a test fails if `.github/workflows/` appears), so this local run is the only gate. If you add a bash suite, list it in `scripts/local-check.mjs`.

- Rust is formatted with `rustfmt` and linted with `clippy -D warnings`.
- The page has no formatter configured: no prettier. Match the style of the file you are in; `tsc` and Vitest are the checks.
- Live suites that call Claude (`scripts/skill-e2e/run.sh`, `golden.sh`, `standalone.sh`, `bmad_spike.sh`) are never part of the gate. Run one by hand only when your change needs it.

## Changes and commits

- Keep each change small and focused: one story or one fix, with its tests. No drive-by refactors or formatting churn.
- Subjects follow `git log`: `S<n>: <short title>` for a story (for example `S4: dcterminalv2:// trace link`), or `<PR id> review fixes: <summary>`. Then a blank line and one short paragraph on why the change was needed.
- Contracts first: the files the app and the plugin share are documented in `docs/run-folder.md` and `docs/launch.md`. Change the doc in the same PR, and bump `plugin/VERSION` and `plugin/.claude-plugin/plugin.json` together when the plugin's interface changes.
- Never commit secrets, `.env` files or personal paths.

## Releases (maintainers)

`scripts/release-mac.sh` builds the DMG on your Mac, signs it ad hoc, verifies the seal on the app inside the mounted DMG and prints the SHA-256. It refuses a dirty tree and mismatched versions, and never publishes unless you pass `--publish`.

```sh
scripts/release-mac.sh              # DCTerminalv2_<version>_aarch64.dmg (Apple silicon)
scripts/release-mac.sh --universal  # DCTerminalv2_<version>_universal.dmg; needs: rustup target add aarch64-apple-darwin x86_64-apple-darwin
scripts/release-mac.sh --publish    # also creates the GitHub release v<version> at the built commit
```

`--publish` refuses unless HEAD is the tip of the remote default branch and the tag does not exist yet.

**Release QA comes before `--publish`.** The installed DMG is tried by a separate macOS user and the results are checked, as described in [docs/release-qa.md](docs/release-qa.md):

1. `scripts/release-qa/prepare.sh --test-user <name>` (you), then `scripts/release-qa/seed.sh <version>` and the computer-use session (the test user).
2. `scripts/release-qa/check.sh <version>` (you) prints `release QA pass`.
3. Only then `scripts/release-mac.sh --publish`. It re-runs the check and refuses without a passing result for this version and HEAD, or when `release.json` or `webqa-target.json` is not yours or is group/world-writable.

## Security

Report vulnerabilities privately, as described in [SECURITY.md](SECURITY.md), not in a public issue.
