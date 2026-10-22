#!/usr/bin/env bash
# End-to-end test of /dcv2:eagle-eye, headless, against a throwaway workspace.
#
#   scripts/skill-e2e/run.sh            # full run, then the halt + resume run
#   scripts/skill-e2e/run.sh full       # only the full run
#   scripts/skill-e2e/run.sh resume     # only the halt + resume run
#
# Env: DCV2_E2E_MODEL (default haiku: orchestrator and, via a test-only plugin copy,
# every sub-agent), DCV2_E2E_TIMEOUT (seconds per claude run, default 1800),
# DCV2_E2E_TMP (parent of the temp dir). Uses the current Claude account.
# Never touches a real repo: the workspace is two toy git repos with no remote,
# so the skill must skip push and PR creation and say so.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

WHICH="${1:-all}"
make_tmp
PLUGIN="$TMP/plugin"
make_test_plugin "$PLUGIN"
status=0

if [[ "$WHICH" == all || "$WHICH" == full ]]; then
  note "TEST 1: full run (auto-approve, no remote)"
  WS="$TMP/ws1"; RUN="$TMP/runs/r-e2e-full"
  make_workspace "$WS"
  write_request "$RUN" "$WS" "test-mode: auto-approve"
  run_claude "$WS" "$PLUGIN" "$TMP/full.jsonl" "/dcv2:eagle-eye run=$RUN"
  python3 "$E2E_DIR/check_run.py" done "$RUN" "$WS" "$TMP/full.jsonl" || status=1
  note "log.md:"; sed 's/^/    /' "$RUN/log.md" 2>/dev/null || true
fi

if [[ "$WHICH" == all || "$WHICH" == resume ]]; then
  note "TEST 2: halt after PR1 (test-mode), delete PR2's worktree, then resume"
  WS="$TMP/ws2"; RUN="$TMP/runs/r-e2e-resume"
  make_workspace "$WS"
  write_request "$RUN" "$WS" "test-mode: auto-approve" "test-mode: halt-after=PR1"
  run_claude "$WS" "$PLUGIN" "$TMP/halt.jsonl" "/dcv2:eagle-eye run=$RUN"
  if python3 "$E2E_DIR/check_run.py" halted "$RUN" "$WS" "$TMP/halt.jsonl" PR1; then
    read -r PR1_REPO PR1_BRANCH PR2_WT < <(python3 -c '
import json,sys; st=json.load(open(sys.argv[1]))
p={x["id"]:x for x in st["prs"]}
print(p["PR1"]["repoPath"], p["PR1"]["branch"], p["PR2"]["worktree"] or "-")' "$RUN/status.json")
    SHA_BEFORE="$(git -C "$PR1_REPO" rev-parse "$PR1_BRANCH")"
    note "PR1 branch $PR1_BRANCH at $SHA_BEFORE"
    if [[ "$PR2_WT" != "-" && -d "$PR2_WT" ]]; then
      note "removing PR2's worktree folder to test the resume re-check: $PR2_WT"
      rm -rf "$PR2_WT"
    fi
    run_claude "$WS" "$PLUGIN" "$TMP/resume.jsonl" "/dcv2:eagle-eye resume run=$RUN"
    python3 "$E2E_DIR/check_run.py" resumed "$RUN" "$WS" "$TMP/resume.jsonl" PR1 "$SHA_BEFORE" || status=1
  else
    status=1
  fi
  note "log.md:"; sed 's/^/    /' "$RUN/log.md" 2>/dev/null || true
fi

if [[ $status == 0 ]]; then note "PASS (artifacts in $TMP)"; else note "FAILED (artifacts in $TMP)"; fi
exit $status
