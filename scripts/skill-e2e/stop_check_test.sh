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
# a link planted at the full log's name is not written through (CAP-8)
rm -f "$RUN/logs/PR1-stop-check.txt"; echo outside > "$d/outside-log.txt"; ln -s "$d/outside-log.txt" "$RUN/logs/PR1-stop-check.txt"
expect 2 "failing check with a link at the log name" "$(payload dcv2:implementer "$reply1")"
okif "  the outside file is unchanged" '[[ "$(cat "$d/outside-log.txt")" == outside ]]'
errhas "  still reports the failure" "PR1 check \`bash check.sh\` fails"
rm -f "$RUN/logs/PR1-stop-check.txt"
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

echo "# slop check (CAP-1)"
F="$d/ws/f"; WT3="$d/ws/f-worktrees/dcv2-r-s-pr3"; mkdir -p "$F"
FBASE="$(sh "$ROOT/fixtures/slop/setup.sh" "$WT3")"
slopset() { # <check command as JSON>
  printf '{"schema": 1, "repos": [{"name": "f", "path": "%s", "check": %s}]}\n' "$F" "$1" > "$RUN/repos.json"
  python3 - "$RUN" "$F" "$WT3" "$FBASE" <<'PY'
import json, sys
run, f, wt, base = sys.argv[1:5]
json.dump({"schema": 1, "runId": "r-s", "stage": "building", "prs": [{"id": "PR3", "repoPath": f, "worktree": wt, "state": "building",
           "base": "main", "check": {"command": "x", "result": "not-run", "cwd": wt}}]}, open(run + "/status.json", "w"))
json.dump({"schema": 1, "runDir": run, "repos": [f], "worktrees": {"PR3": wt}, "bases": {"PR3": base}}, open(run + "/.guard.json", "w"))
PY
  rm -f "$RUN/logs/.stop-check.json"
}
reply3='{"pr": "PR3", "status": "IMPLEMENTED"}'
found() { sed -n 's/^  \([^ :]*\):\([0-9]*\): \[\([a-z-]*\)\].*/\1:\2:\3/p' "$d/err" | sort > "$d/found.txt"; }
slopset '"true"'
expect 2 "a passing check: slop reported" "$(payload dcv2:implementer "$reply3")"
found; okif "  the findings equal fixtures/slop/expected.txt" 'diff -u <(sort "$ROOT/fixtures/slop/expected.txt") "$d/found.txt" >"$d/diff.txt" || { cat "$d/diff.txt"; false; }'
okif "  unchanged lines are never flagged (old.ts line 2)" '! grep -q "^src/old.ts:2:" "$d/found.txt"'
okif "  real output is not flagged (scripts/run.js, println!)" '! grep -q "scripts/run.js\|:5:" "$d/found.txt"'
okif "  excluded paths are not flagged (plugin/bmad, lock files)" '! grep -q "plugin/bmad\|yarn.lock" "$d/found.txt"'
okif "  docs are not matched by the code scope" '! grep -q "^notes.md:3:" "$d/found.txt"'
errhas "  says what to remove" "leftover debugging or filler"
slopset '"sleep 5; false"'
expect 2 "a check slower than the timeout (timed out now): slop still reported" "$(payload dcv2:implementer "$reply3")" DCV2_STOP_CHECK_TIMEOUT=1
errhas "  says the check did not run" "check not run in the hook (slow)"
okif "  remembered as slow" 'grep -q "\"slow\": true" "$RUN/logs/.stop-check.json"'
expect 2 "the cached-slow entry: slop still reported" "$(payload dcv2:implementer "$reply3")"
errhas "  says the check did not run" "check not run in the hook (slow)"
slopset null
expect 2 "no check command: slop reported" "$(payload dcv2:implementer "$reply3")"
slopset '"echo broken; exit 1"'
expect 2 "a failing check is reported alone" "$(payload dcv2:implementer "$reply3")"
errhas "  the check failure" "PR3 check \`echo broken; exit 1\` fails"
okif "  no slop findings with it" '! grep -q "\[console-log\]\|\[dbg-macro\]" "$d/err"'
slopset '"true"'
expect 0 "stop_hook_active: no slop" "$(payload dcv2:implementer "$reply3" true)"
expect 0 "DCV2_STOP_HOOKS=off: no slop" "$(payload dcv2:implementer "$reply3")" DCV2_STOP_HOOKS=off
expect 0 "DCV2_SLOP_CHECK=off: no slop" "$(payload dcv2:implementer "$reply3")" DCV2_SLOP_CHECK=off
# more than 30 findings: capped, exit 2
for i in $(seq 1 40); do printf 'dbg!(%s);\n' "$i" >> "$WT3/lib.rs"; done
expect 2 "more than 30 findings" "$(payload dcv2:implementer "$reply3")"
okif "  at most 30 are listed" '(( $(grep -c "^  [^ ]*:[0-9]*: \[" "$d/err") == 30 ))'
errhas "  says how many more" "more]"
git -C "$WT3" checkout -q -- lib.rs; cp "$ROOT/fixtures/slop/uncommitted/lib.rs" "$WT3/lib.rs"

echo "# every pattern: compiles, and has 0 hits on the repo's tracked files at HEAD"
python3 - "$ROOT" <<'PY' && echo "ok   slop patterns compile and give 0 hits on tracked files" || { echo "FAIL slop patterns"; fails=1; }
import re, subprocess, sys
root = sys.argv[1]
rules = []
for line in open(root + "/plugin/scripts/slop-patterns.tsv", encoding="utf-8"):
    if not line.strip() or line.startswith("#"):
        continue
    rid, scope, pat, msg = line.rstrip("\n").split("\t")[:4]
    rules.append((rid, scope, re.compile(pat)))
assert len(rules) >= 5, "the starter rules"
DOC = (".md", ".mdx", ".txt", ".rst")
LOCKS = {"package-lock.json", "yarn.lock", "pnpm-lock.yaml", "cargo.lock", "go.sum", "poetry.lock"}
def scoped(scope, path):
    low = path.lower()
    if scope == "*": return True
    if scope == "code": return not low.endswith(DOC)
    spec, _, prefix = scope[4:].partition("@")
    return low.endswith(tuple("." + e for e in spec.split(","))) and path.startswith(prefix)
files = subprocess.run(["git", "-C", root, "ls-files"], capture_output=True, text=True).stdout.split("\n")
hits = []
for f in files:
    if not f or f.startswith(("plugin/scripts/slop-patterns.tsv", "fixtures/slop/", "plugin/bmad/")) or f.rsplit("/", 1)[-1].lower() in LOCKS or f.endswith(".lock"):
        continue
    try:
        text = open(root + "/" + f, encoding="utf-8").read()
    except (UnicodeDecodeError, OSError):
        continue
    for n, line in enumerate(text.split("\n"), 1):
        for rid, scope, rx in rules:
            if scoped(scope, f) and rx.search(line):
                hits.append(f"{f}:{n}:{rid}")
assert not hits, hits[:10]
PY

echo "# never blocks on its own errors"
expect 0 "garbage payload" "not json"
badpy="$d/badpy"; mkdir -p "$badpy"; printf '#!/bin/sh\nexit 1\n' > "$badpy/python3"; cp "$badpy/python3" "$badpy/python"; cp "$badpy/python3" "$badpy/py"; chmod +x "$badpy"/py*
expect 0 "broken python3 never blocks" "$(payload dcv2:implementer "$reply1")" PATH="$badpy:$PATH"
status building building; guard; repos '"bash check.sh"' '"true"'
echo '{' > "$RUN/repos.json"
expect 0 "unreadable repos.json" "$(payload dcv2:implementer "$reply1")"

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
