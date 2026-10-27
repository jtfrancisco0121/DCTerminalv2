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

## From the review of #11 (afk-cleanup, 2026-10-27)

LOWs left for later:
- `git worktree prune` by `state.sh worktree` / `cleanup` prunes every stale worktree of the repo, not only the run's.
- The cleanup trusts `gh pr view`'s `headRefName` + the recorded repo; a fork PR (`isCrossRepository`) with the same branch name is not checked separately.
- `requests/` grows by two files per request and is never pruned.
- A worktree that is "already gone" is recorded as gone without checking whether `git worktree list` still names it (prune happens later).
- A copied-in `.env` (copyFiles) that the user edited in the worktree goes with the worktree (it is treated as a copy).
- **Do first:** interpreter writes (`python3 -c 'open(".guard.json","w")…'`, `node -e`) are not seen by the guard; this is the root of a forged `.guard.json` (and with it forged worktree / remote records).
- `git stash drop` / `stash clear` by a sub-agent drops the shared `refs/stash` (LOW).
- Same class as the interpreter gap: shells fed a script the guard never sees: stdin (`echo … | bash`, `bash <<< …`, `bash /dev/stdin`), process substitution (`bash <(…)`, `source <(…)`), and a script written earlier (`printf … > /tmp/x.sh; bash /tmp/x.sh`), plus `set -a; . ./envfile`.
- `DCV2_TEST_LOCAL_REMOTES` is matched as a substring, so `DCV2_TEST_LOCAL_REM""OTES=1` gets past the guard (no escalation: it only lets state.sh talk to a local folder remote, which a run cannot record without a forged `.guard.json`).
- (MEDIUM per the re-review) The orchestrator may still delete or reset a run worktree's files (`rm -rf <wt>`, `git clean`, `git reset --hard` in the worktree) and write files through interpreters (`python3 -c 'open(...)'`): the guard sees shell write targets only. Worth a rule that the orchestrator writes nothing in worktrees.
- (Fourth review of #11) `GIT_EDITOR`, `GIT_SEQUENCE_EDITOR`, `GIT_PAGER`, `PAGER`, `EDITOR`, `VISUAL` with non-trivial values run a program from git; refuse anything but `true`/`cat`/`:`.
- Indirect variable setting (`read VAR`, `printf -v VAR`, `export $X=…`, `export GIT_${N}…`) gets past the git-env rule.
- Optional: more harmless `git -c` boolean keys (`core.editor=true`, `core.pager=cat`, `diff.noprefix`, `merge.conflictstyle`, `rebase.autosquash`, `core.autocrlf`) for Implementer convenience.
