#!/usr/bin/env bash
# Unit test of plugin/scripts/stop-check.sh (A3), no Claude call.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HOOK="$ROOT/plugin/scripts/stop-check.sh"
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
trap 'rm -rf "$d"' EXIT
unset DCV2_STOP_HOOKS DCV2_TEST_MODE || true

RUN="$d/runs/r-s"; WT1="$d/ws/a-worktrees/dcv2-r-s-pr1"; WT2="$d/ws/b-worktrees/dcv2-r-s-pr2"
mkdir -p "$RUN" "$WT1" "$WT2"
# PR1's check fails (with colour codes and lots of output) until ok.txt exists; PR2's passes.
cat > "$WT1/check.sh" <<'EOF'
for i in $(seq 1 200); do echo "progress line $i"; done
printf '\033[31mAssertionError: expected 6, got 5\033[0m\n'
[ -f ok.txt ] || { echo "1 test failed"; exit 1; }
echo "all passed"
EOF
status() { # <pr1 state> <pr2 state> [pr1 command]
  python3 - "$RUN" "$WT1" "$WT2" "$1" "$2" "${3:-bash check.sh}" <<'PY'
import json, sys
run, w1, w2, s1, s2, c1 = sys.argv[1:7]
pr = lambda i, w, s, c: {"id": f"PR{i}", "worktree": w, "state": s, "check": {"command": c, "result": "not-run", "cwd": w}}
json.dump({"schema": 1, "runId": "r-s", "stage": "building", "prs": [pr(1, w1, s1, c1), pr(2, w2, s2, "true")]}, open(run + "/status.json", "w"))
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
grep -q $'\033' "$d/err" && { echo "FAIL   ANSI left in"; fails=1; } || echo "ok     no ANSI"
(( $(wc -c < "$d/err") < 4500 )) && echo "ok     trimmed ($(wc -c < "$d/err") bytes)" || { echo "FAIL   not trimmed"; fails=1; }
grep -q "progress line 200" "$RUN/logs/PR1-stop-check.txt" && echo "ok     full output in logs/PR1-stop-check.txt" || { echo "FAIL   no full log"; fails=1; }
expect 0 "second stop in a row (stop_hook_active)" "$(payload dcv2:implementer "$reply1" true)"
expect 0 "kill switch DCV2_STOP_HOOKS=off" "$(payload dcv2:implementer "$reply1")" DCV2_STOP_HOOKS=off
expect 0 "no run bound (standalone)" "$(payload dcv2:implementer "$reply1")" DCV2_RUN_DIR=
expect 0 "main session Stop" "$(payload "" "$reply1")"
expect 0 "PR reviewer" "$(payload dcv2:pr-reviewer "$reply1")"
expect 0 "PR2 implementer, passing check" "$(payload implementer "$reply2")"
expect 0 "two busy PRs, cannot tell which" "$(payload dcv2:implementer "no json here")"
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
status fixing pending "sleep 5; false"
start=$SECONDS
expect 0 "check slower than the timeout: skipped" "$(payload dcv2:implementer "$reply1")" DCV2_STOP_CHECK_TIMEOUT=1
(( SECONDS - start < 4 )) && echo "ok     killed at the timeout" || { echo "FAIL   ran $((SECONDS - start))s"; fails=1; }
grep -q '"slow": true' "$RUN/logs/.stop-check.json" && echo "ok     remembered as slow" || { echo "FAIL   cache: $(cat "$RUN/logs/.stop-check.json")"; fails=1; }
start=$SECONDS
expect 0 "slow check skipped next time" "$(payload dcv2:implementer "$reply1")"
(( SECONDS - start < 2 )) && echo "ok     skipped without running" || { echo "FAIL   ran again"; fails=1; }
status fixing pending "null"
python3 - "$RUN/status.json" <<'PY'
import json, sys; s = json.load(open(sys.argv[1])); s["prs"][0]["check"]["command"] = None; json.dump(s, open(sys.argv[1], "w"))
PY
expect 0 "no check command" "$(payload dcv2:implementer "$reply1")"
status building pending
expect 0 "garbage payload" "not json"
badpy="$d/badpy"; mkdir -p "$badpy"; printf '#!/bin/sh\nexit 1\n' > "$badpy/python3"; chmod +x "$badpy/python3"
expect 0 "broken python3 never blocks" "$(payload dcv2:implementer "$reply1")" PATH="$badpy:$PATH"

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
