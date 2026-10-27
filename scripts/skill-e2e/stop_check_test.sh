#!/usr/bin/env bash
# Unit test of plugin/scripts/stop-check.sh (A3), no Claude call.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HOOK="$ROOT/plugin/scripts/stop-check.sh"
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
trap 'rm -rf "$d"' EXIT
unset DCV2_STOP_HOOKS DCV2_TEST_MODE DCV2_RUN_DIR DCV2_MEMORY_DIR || true

RUN="$d/runs/r-s"; A="$d/ws/a"; B="$d/ws/b"
WT1="$d/ws/a-worktrees/dcv2-r-s-pr1"; WT2="$d/ws/b-worktrees/dcv2-r-s-pr2"
mkdir -p "$RUN" "$A" "$B" "$WT1" "$WT2" "$d/elsewhere"
# PR1's check fails (with colour codes and lots of output) until ok.txt exists; PR2's passes.
cat > "$WT1/check.sh" <<'EOF'
for i in $(seq 1 200); do echo "progress line $i"; done
printf '\033[31mAssertionError: expected 6, got 5\033[0m\n'
[ -f ok.txt ] || { echo "1 test failed"; exit 1; }
echo "all passed"
EOF
cp "$WT1/check.sh" "$d/elsewhere/check.sh"
# The worktrees as state.sh worktree records them for the guard.
guard() { printf '{"schema": 1, "runDir": "%s", "repos": ["%s", "%s"], "worktrees": {"PR1": "%s", "PR2": "%s"}}\n' "$RUN" "$A" "$B" "$WT1" "$WT2" > "$RUN/.guard.json"; }
guard
# The check commands come only from repos.json (the app's input), never from status.json.
repos() { # <repo a check as JSON> <repo b check as JSON>
  printf '{"schema": 1, "repos": [{"name": "a", "path": "%s", "check": %s}, {"name": "b", "path": "%s", "check": %s}]}\n' "$A" "$1" "$B" "$2" > "$RUN/repos.json"
}
repos '"bash check.sh"' '"true"'
status() { # <pr1 state> <pr2 state> [pr1 command in status.json] [pr1 worktree] [pr1 check.cwd]
  local wt="${4:-$WT1}"
  python3 - "$RUN" "$WT2" "$1" "$2" "${3:-bash check.sh}" "$wt" "${5:-$wt}" "$A" "$B" <<'PY'
import json, sys
run, w2, s1, s2, c1, wt1, cwd1, a, b = sys.argv[1:10]
pr = lambda i, r, w, s, c, cwd: {"id": f"PR{i}", "repoPath": r, "worktree": w, "state": s,
                                 "check": {"command": c, "result": "not-run", "cwd": cwd}}
json.dump({"schema": 1, "runId": "r-s", "stage": "building",
           "prs": [pr(1, a, wt1, s1, c1, cwd1), pr(2, b, w2, s2, "true", w2)]}, open(run + "/status.json", "w"))
PY
}
payload() { # <agent_type> <last message> [stop_hook_active]
  python3 -c 'import json,sys; a,m,s=sys.argv[1:4]; print(json.dumps({"hook_event_name":"SubagentStop" if a else "Stop","agent_type":a,"agent_id":"x",
"stop_hook_active":s=="true","last_assistant_message":m,"agent_transcript_path":sys.argv[4],"cwd":"/"}))' "$1" "$2" "${3:-false}" "$d/t.jsonl"
}
fails=0
expect() { # <exit> <label> <payload> [env...]
  local want="$1" label="$2" in="$3" got=0; shift 3
  env DCV2_RUN_DIR="$RUN" "$@" bash "$HOOK" <<<"$in" 2>"$d/err" || got=$?
  if [[ $got == "$want" ]]; then echo "ok   $label -> $got"; else echo "FAIL $label -> $got (want $want): $(head -c 300 "$d/err")"; fails=1; fi
}
okif() { if eval "$2"; then echo "ok     $1"; else echo "FAIL   $1"; fails=1; fi; }
errhas() { if grep -qF -- "$2" "$d/err"; then echo "ok   $1"; else echo "FAIL $1: no '$2' in $(head -c 300 "$d/err")"; fails=1; fi; }
reply1='done
```json
{"pr": "PR1", "status": "IMPLEMENTED"}
```'
reply2='{"pr": "PR2", "status": "IMPLEMENTED"}'

status building building
expect 2 "PR1 implementer, failing check" "$(payload dcv2:implementer "$reply1")"
errhas "  names the PR and the command" "PR1 check \`bash check.sh\` fails"
errhas "  keeps the error line, ANSI stripped" "AssertionError: expected 6, got 5"
errhas "  keeps the tail" "1 test failed"
okif "no ANSI" '! grep -q $'"'"'\033'"'"' "$d/err"'
okif "trimmed" '(( $(wc -c < "$d/err") < 4500 ))'
okif "full output in logs/PR1-stop-check.txt" 'grep -q "progress line 200" "$RUN/logs/PR1-stop-check.txt"'
expect 0 "second stop in a row (stop_hook_active)" "$(payload dcv2:implementer "$reply1" true)"
expect 0 "kill switch DCV2_STOP_HOOKS=off" "$(payload dcv2:implementer "$reply1")" DCV2_STOP_HOOKS=off
expect 0 "no run bound (standalone)" "$(payload dcv2:implementer "$reply1")" DCV2_RUN_DIR=
expect 0 "main session Stop" "$(payload "" "$reply1")"
expect 0 "PR reviewer" "$(payload dcv2:pr-reviewer "$reply1")"
expect 0 "PR2 implementer, passing check" "$(payload implementer "$reply2")"
expect 0 "two busy PRs, cannot tell which" "$(payload dcv2:implementer "no json here")"
expect 0 "a worktree path only as a prefix does not pick a PR" "$(payload dcv2:implementer "see ${WT1}-other/x")"
expect 0 "PR1's worktree path inside PR10's does not pick PR1" "$(payload dcv2:implementer "worked in ${WT1}0/a.js")"
expect 2 "the worktree path with a boundary picks PR1" "$(payload dcv2:implementer "worked in ${WT1}/a.js")"
printf '{"type":"user","message":{"content":"run=%s pr=PR1 worktree=%s round=1"}}\n' "$RUN" "$WT1" > "$d/t.jsonl"
expect 2 "PR found from the sub-agent's prompt" "$(payload dcv2:implementer "no json here")"
rm -f "$d/t.jsonl"
status building pending
expect 2 "the one busy PR" "$(payload dcv2:implementer "no json")"
status reviewing pending
expect 0 "PR not building/fixing" "$(payload dcv2:implementer "$reply1")"
status fixing pending
touch "$WT1/ok.txt"
expect 0 "fixed: check passes" "$(payload dcv2:implementer "$reply1")"
rm "$WT1/ok.txt"

echo "# the command and the folder never come from status.json"
status building pending "touch $d/PWNED; false"
expect 2 "status.json's command is ignored; repos.json's runs" "$(payload dcv2:implementer "$reply1")"
errhas "  the repos.json command" "\`bash check.sh\`"
okif "status.json's command did not run" '[[ ! -e "$d/PWNED" ]]'
mv "$RUN/repos.json" "$RUN/repos.json.off"
expect 0 "no repos.json: skipped" "$(payload dcv2:implementer "$reply1")"
okif "status.json's command still did not run" '[[ ! -e "$d/PWNED" ]]'
mv "$RUN/repos.json.off" "$RUN/repos.json"
repos null '"true"'
expect 0 "no check for the repo in repos.json: skipped" "$(payload dcv2:implementer "$reply1")"
okif "status.json's command still did not run" '[[ ! -e "$d/PWNED" ]]'
repos '"bash check.sh"' '"true"'
status building pending "bash check.sh" "$d/elsewhere"
expect 0 "a worktree that is not the canonical one: skipped" "$(payload dcv2:implementer "$reply1")"
status building pending "bash check.sh" "$WT1" "$d/elsewhere"
expect 2 "status.json's check.cwd is ignored: runs in the worktree" "$(payload dcv2:implementer "$reply1")"
errhas "  in the worktree" "fails in $WT1"
printf '{"schema": 1, "runDir": "%s", "repos": [], "worktrees": {}}\n' "$RUN" > "$RUN/.guard.json"
expect 0 "worktree not recorded in .guard.json: skipped" "$(payload dcv2:implementer "$reply1")"
guard
mv "$WT1" "$WT1.real"; ln -s "$d/elsewhere" "$WT1"
expect 0 "the worktree is a symlink elsewhere: skipped" "$(payload dcv2:implementer "$reply1")"
rm "$WT1"; mv "$WT1.real" "$WT1"
status building pending

echo "# slow checks"
repos '"sleep 5; false"' '"true"'
start=$SECONDS
expect 0 "check slower than the timeout: skipped" "$(payload dcv2:implementer "$reply1")" DCV2_STOP_CHECK_TIMEOUT=1
okif "killed at the timeout" '(( SECONDS - start < 4 ))'
okif "remembered as slow" 'grep -q "\"slow\": true" "$RUN/logs/.stop-check.json"'
start=$SECONDS
expect 0 "slow check skipped next time" "$(payload dcv2:implementer "$reply1")"
okif "skipped without running" '(( SECONDS - start < 2 ))'

echo "# never blocks on its own errors"
expect 0 "garbage payload" "not json"
badpy="$d/badpy"; mkdir -p "$badpy"; printf '#!/bin/sh\nexit 1\n' > "$badpy/python3"; chmod +x "$badpy/python3"
expect 0 "broken python3 never blocks" "$(payload dcv2:implementer "$reply1")" PATH="$badpy:$PATH"
echo '{' > "$RUN/repos.json"
expect 0 "unreadable repos.json" "$(payload dcv2:implementer "$reply1")"

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
