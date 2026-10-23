# Follow-ups after P1–P4 (2026-10-23)

LOW findings left from the phase reviews, plus checks not yet done.

## Not yet tried
- First GUI launch (`cargo build --manifest-path src-tauri/Cargo.toml` once for `dcv2-hook`, then `npm run dev:app`).
- Release bundle: confirm `dcv2-hook` lands in `Contents/MacOS` and `plugin/` in Resources.
- Real keychain prompts (ClickUp token, trust key).
- P5: one real Koneksi Medium ticket end to end (JT, work account).

## LOWs
- P3b: "tool running" flag can stick (Esc mid-tool, API error): register `StopFailure`, time-limit the hold-off to 2× the watchdog threshold.
- P3b: parallel tool calls share one running flag (errs toward a stall warning).
- P4: replay of an older signed run-trust copy (needs a guard bypass): keep the latest posted-record hash in the keychain.
- P4: guard rule against `security find-generic-password … dcterminalv2`.
- P4: R8 reads comments for every ticket (≈200 calls for 100 tickets, 4 in flight).
- P2b: golden/run.sh suites write no repos.json, so the stop hook skips in live runs; its fix path is unit-tested only.
- P2b: guard shell-write detection is best effort (python/node writes not parsed); WAIVED trusts the orchestrator's word.
- P2b: stop hook skips checks slower than 30 s; memory topics updated in a run show stale on the default branch until the PR merges.
- P3a: Start the same request twice gives two runs; memory files read-only in the app; `repos:` limit not offered in the UI.
- P1: page-supplied window_id trust; settings.json concurrent overwrite.
