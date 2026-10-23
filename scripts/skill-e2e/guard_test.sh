#!/usr/bin/env bash
# Unit test of plugin/scripts/guard.sh (no Claude call).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GUARD="$ROOT/plugin/scripts/guard.sh"
STATE="$ROOT/plugin/scripts/state.sh"
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
trap 'rm -rf "$d"' EXIT

# A repo, a bound run with PR1's worktree recorded by state.sh, and look-alikes that are not recorded.
RUN="$d/runs/r-x"; R="$d/ws/r"; WT="$d/ws/r-worktrees/dcv2-r-x-pr1"
mkdir -p "$RUN" "$R" "$d/fake"
git -C "$R" init -q -b main && git -C "$R" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
base="$(git -C "$R" rev-parse HEAD)"
printf 'folder: %s/ws\n' "$d" > "$RUN/request.md"; echo '{}' > "$RUN/status.json"
bash "$STATE" worktree "$RUN" "$R" PR1 "$base" >/dev/null
git -C "$R" worktree add -q -b dcv2/r-x-pr2 "$d/ws/r-worktrees/dcv2-r-x-pr2" "$base"        # other PR, not recorded
git -C "$R" worktree add -q -b dcv2/other-pr1 "$d/ws/r-worktrees/dcv2-other-pr1" "$base"     # other run
touch "$d/fake/request.md" "$d/fake/status.json"                                             # fake run folder
ln -s "$R" "$WT/link"                                                                        # symlink out of the worktree
export DCV2_RUN_DIR="$RUN"
unset DCV2_TEST_MODE || true

fails=0
payload() { # <agent> <tool> <key> <value> [cwd]
  python3 -c 'import json,sys; a,t,k,v,c=sys.argv[1:6]; print(json.dumps({"agent_type":a or None,"tool_name":t,"tool_input":{k:v},"cwd":c}))' "$1" "$2" "$3" "$4" "${5:-$d/ws}"
}
expect() { # <expected exit> <label> <payload> [env...]
  local want="$1" label="$2" in="$3" got=0; shift 3
  env "$@" bash "$GUARD" <<<"$in" 2>"$d/err" || got=$?; [[ -z "${VERBOSE:-}" ]] || sed "s/^/       /" "$d/err"
  if [[ $got == "$want" ]]; then echo "ok   $label -> $got"; else echo "FAIL $label -> $got (want $want)"; fails=1; fi
}
w() { expect "$1" "$2 Write ${3#$d}" "$(payload "$2" Write file_path "$3")"; }               # <exit> <agent> <path>
b() { expect "$1" "$2 Bash: ${3//$d/}" "$(payload "$2" Bash command "$3" "${4:-$d/ws}")"; }  # <exit> <agent> <cmd> [cwd]

echo "# Edit/Write"
w 2 dcv2:implementer "$R/a.js"
w 0 dcv2:implementer "$WT/a.js"
w 0 dcv2:implementer "$RUN/reports/PR1-build1.md"
w 2 dcv2:implementer "r/a.js"
w 2 dcv2:implementer "$R/evil-worktrees/dcv2-z/x.js"
w 2 dcv2:implementer "$d/ws/r-worktrees/dcv2-r-x-pr2/a.js"
w 2 dcv2:implementer "$d/ws/r-worktrees/dcv2-other-pr1/a.js"
w 2 dcv2:implementer "$d/fake/plan.md"
w 2 dcv2:implementer "$WT/link/a.js"
w 2 dcv2:implementer "$WT/../../r/a.js"
w 2 dcv2:implementer "$RUN/../../ws/r/a.js"
w 2 implementer "$R/a.js"
w 0 implementer "$WT/a.js"
w 2 pr-reviewer "$WT/a.js"
w 2 dcv2:pr-reviewer "$WT/a.js"
w 0 dcv2:pr-reviewer "$RUN/reviews/PR1-round1.md"
w 2 dcv2:planner "$R/plan.md"
w 0 dcv2:planner "$RUN/plan.md"
w 2 "" "$R/a.js"                                                   # the orchestrator never edits a main checkout (R4)
w 0 "" "$WT/a.js"
w 0 "" "$d/elsewhere/notes.md"
expect 0 "user session (no run) may edit a repo" "$(payload "" Write file_path "$R/a.js")" -u DCV2_RUN_DIR
w 2 "" "$RUN/status.json"
w 2 "" "$RUN/.guard.json"
w 2 dcv2:implementer "$RUN/writeback.json"
w 2 "" "$d/fake/status.json"
expect 2 "implementer with no DCV2_RUN_DIR" "$(payload dcv2:implementer Write file_path "$WT/a.js")" -u DCV2_RUN_DIR
expect 0 "user session with no DCV2_RUN_DIR" "$(payload "" Write file_path "$R/a.js")" -u DCV2_RUN_DIR

echo "# codebase memory: only memory.sh write (D9)"
MEMD="$d/mem"; mkdir -p "$MEMD/x/topics"
expect 2 "orchestrator Write into memory" "$(payload "" Write file_path "$MEMD/x/index.md")" DCV2_MEMORY_DIR="$MEMD"
expect 2 "user session Write into memory" "$(payload "" Edit file_path "$MEMD/x/topics/a.md")" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
expect 2 "implementer Write into memory" "$(payload dcv2:implementer Write file_path "$MEMD/x/index.md")" DCV2_MEMORY_DIR="$MEMD"
expect 2 "user session redirect into memory" "$(payload "" Bash command "echo x > $MEMD/x/index.md")" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
expect 2 "redirect via \$DCV2_MEMORY_DIR" "$(payload "" Bash command 'echo x > $DCV2_MEMORY_DIR/x/index.md')" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
expect 2 "cp into memory" "$(payload dcv2:memory-updater Bash command "cp /tmp/a $MEMD/x/index.md")" DCV2_MEMORY_DIR="$MEMD"
expect 0 "memory.sh write with a heredoc" "$(payload dcv2:memory-updater Bash command "bash /p/scripts/memory.sh write x index.md <<'EOF'
# x
EOF")" DCV2_MEMORY_DIR="$MEMD"
expect 2 "cd into memory via the env var, then a relative write (LOW 6)" "$(payload "" Bash command 'cd "$DCV2_MEMORY_DIR/x" && echo x > index.md')" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
expect 2 "cd into memory by path, then a relative write" "$(payload "" Bash command "cd $MEMD/x && echo x > index.md")" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
expect 2 "cd via another variable, memory mentioned, relative write" "$(payload "" Bash command 'M=$DCV2_MEMORY_DIR; cd $M && echo x > x/index.md')" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
expect 0 "cd into memory to read" "$(payload "" Bash command 'cd "$DCV2_MEMORY_DIR" && cat x/index.md')" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
expect 0 "user session: unknown cd + relative write, memory not mentioned" "$(payload "" Bash command 'cd "$(git rev-parse --show-toplevel)" && echo x > notes.txt')" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
expect 0 "user session with memory set: git push still theirs" "$(payload "" Bash command "git push origin main")" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
expect 0 "user session with memory set: unrelated write" "$(payload "" Write file_path "$R/a.js")" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
echo "# memory-updater"
expect 0 "memory-updater (developer session, no run): read-only git" "$(payload dcv2:memory-updater Bash command "git -C $R diff --name-only HEAD~1 HEAD")" -u DCV2_RUN_DIR
expect 2 "memory-updater (no run): no Write" "$(payload dcv2:memory-updater Write file_path "$d/x.md")" -u DCV2_RUN_DIR
expect 2 "memory-updater (no run): no git commit" "$(payload dcv2:memory-updater Bash command "git -C $R commit -m x")" -u DCV2_RUN_DIR
expect 0 "memory-updater in a run: writes memory-update.md" "$(payload dcv2:memory-updater Write file_path "$RUN/memory-update.md")"
expect 2 "memory-updater in a run: not into a worktree" "$(payload dcv2:memory-updater Write file_path "$WT/a.js")"
expect 2 "memory-updater: no push" "$(payload dcv2:memory-updater Bash command "git -C $WT push")"

echo "# unparsable input / no python3"
expect 2 "unparsable payload from an implementer" '{"agent_type": "dcv2:implementer", "tool_input": {broken'
expect 2 "unparsable payload from a bare planner" '{"agent_type":"planner", oops'
expect 0 "unparsable payload, no agent" '{"tool_input": {broken'
nopy="$d/nopy"; mkdir -p "$nopy"; for c in bash cat grep; do ln -s "$(command -v $c)" "$nopy/$c"; done
expect 2 "no python3: implementer" "$(payload dcv2:implementer Write file_path "$WT/a.js")" PATH="$nopy"
expect 0 "no python3: user session" "$(payload "" Write file_path "$R/a.js")" -u DCV2_RUN_DIR PATH="$nopy"
badpy="$d/badpy"; mkdir -p "$badpy"; printf '#!/bin/sh\nexit 1\n' > "$badpy/python3"; chmod +x "$badpy/python3"
expect 2 "broken python3: implementer" "$(payload dcv2:implementer Write file_path "$WT/a.js")" PATH="$badpy:$PATH"

echo "# Bash: sub-agents"
b 0 dcv2:implementer "git -C $WT add -A && git -C $WT commit -m 'S1: x'"
b 0 dcv2:implementer "cd $WT && git commit -am \"S1: it's\""
b 2 dcv2:implementer "git -C $R commit -m x"
b 2 dcv2:implementer "cd $R && git checkout -b x"
b 2 dcv2:implementer "git -C $d/ws/r-worktrees/dcv2-r-x-pr2 commit -m x"
b 2 dcv2:implementer "git -C $WT push origin HEAD"
b 2 implementer "cd $WT; git push"
b 2 dcv2:implementer "echo \$(git push)"
b 2 dcv2:implementer "echo \`git push\`"
b 2 dcv2:implementer "bash -c 'git -C $R commit -m x'"
b 2 dcv2:implementer "echo x > $R/a.js"
b 2 dcv2:implementer "echo x >> $R/evil-worktrees/dcv2-z/x.js"
b 2 dcv2:implementer "echo x > $d/ws/r-worktrees/dcv2-other-pr1/a.js"
b 2 dcv2:implementer "echo x > $WT/link/a.js"
b 0 dcv2:implementer "echo x > $WT/a.js"
b 0 dcv2:implementer "npm run check > $RUN/logs/PR1-check.txt 2>&1" "$WT"
b 2 dcv2:implementer "sed -i '' 's/a/b/' $R/a.js"
b 0 dcv2:implementer "sed -i '' 's/a/b/' $WT/a.js"
b 2 dcv2:implementer "printf x | tee $R/a.js"
b 2 dcv2:implementer "cp $WT/a.js $R/a.js"
b 0 dcv2:implementer "cp $R/a.js $WT/a.js"
b 2 dcv2:implementer "rm -f $R/a.js"
b 2 dcv2:implementer "cd .. && echo x > r/a.js" "$WT"
b 2 dcv2:implementer "echo x > \$REPO/a.js"
b 0 dcv2:implementer "$(printf 'cat > %s/a.js <<'"'"'EOF'"'"'\nit'"'"'s fine; git push\nEOF' "$WT")"
b 2 dcv2:implementer "echo 'unbalanced"
b 0 dcv2:pr-reviewer "git -C $WT diff main...HEAD && git -C $WT log --oneline -n 5"
b 0 dcv2:pr-reviewer "cd $WT && npm test > $RUN/logs/PR1-review1-check.txt 2>&1"
b 2 dcv2:pr-reviewer "git -C $WT commit -m x"
b 2 dcv2:pr-reviewer "git -C $WT stash"
b 2 dcv2:pr-reviewer "echo x > $WT/a.js"
b 2 dcv2:pr-reviewer "gh pr create --title x"
b 2 dcv2:pr-reviewer "gh api repos/o/r/pulls"
b 2 dcv2:planner "gh issue comment 3 --body x"
b 0 dcv2:planner "git -C $R log --oneline -n 5 && git -C $R branch --list"
b 2 dcv2:planner "git -C $R branch newbranch"
b 2 dcv2:planner "echo x > $R/notes.md"
b 0 dcv2:planner "echo x > $RUN/notes.md"
b 2 dcv2:planner "echo {} > $RUN/status.json"

echo "# Bash: the orchestrator"
b 2 "" "git -C $WT push -u origin dcv2/r-x-pr1"
b 2 "" "gh pr create --base main --head dcv2/r-x-pr1 --title x --body-file /dev/null"
b 2 "" "echo '{}' > $RUN/status.json"
b 2 "" "cp /dev/null $RUN/writeback.json"
b 0 "" "git -C $R status && git -C $R remote"
b 0 "" "bash $STATE log $RUN building <<'EOF'
PR1 built; git push later
EOF"
b 2 "" "gh pr merge 12"
b 2 "" "git -C $R commit -am x"                                     # R4: no changing git in a main checkout
b 2 "" "git -C $R checkout -b x"
b 2 "" "echo x > $R/a.js"
b 2 "" "sed -i '' s/a/b/ $R/a.js"
b 0 "" "git -C $R worktree list && git -C $R log --oneline -n 3"
b 0 "" "echo x > $RUN/notes.md"
expect 0 "user session: git push with no run bound" "$(payload "" Bash command "git push --force")" -u DCV2_RUN_DIR
python3 - "$RUN/status.json" <<'PY'
import json, sys
json.dump({"schema": 1, "stage": "shipping", "shipApproval": {"by": "user", "at": "2026-10-22T00:00:00Z"}}, open(sys.argv[1], "w"))
PY
b 0 "" "git -C $WT push -u origin dcv2/r-x-pr1"
b 0 "" "gh pr create --base main --head dcv2/r-x-pr1 --title x --body-file /dev/null"
b 2 "" "git -C $WT push --force origin dcv2/r-x-pr1"
b 2 "" "git -C $WT push -f origin dcv2/r-x-pr1"
b 2 "" "git -C $WT push --force-with-lease origin dcv2/r-x-pr1"
b 2 "" "git -C $WT push origin +dcv2/r-x-pr1"
b 2 "" "git -C $WT push origin --delete dcv2/r-x-pr1"
b 2 "" "git -C $WT push origin :dcv2/r-x-pr1"
b 2 "" "git -C $WT push origin HEAD:main"                           # R4: only dcv2/ branches
b 2 "" "git -C $WT push origin main"
b 2 "" "git -C $WT push --all origin"
b 2 "" "git -C $WT push --mirror origin"
b 2 "" "git -C $R push -u origin dcv2/r-x-pr1"                      # never from a main checkout
b 0 "" "git -C $WT push -u origin refs/heads/dcv2/r-x-pr1"
b 2 dcv2:implementer "git -C $WT push -u origin dcv2/r-x-pr1"
python3 - "$RUN/status.json" <<'PY'
import json, sys
json.dump({"schema": 1, "stage": "shipping", "shipApproval": None}, open(sys.argv[1], "w"))
PY
b 2 "" "git -C $WT push -u origin dcv2/r-x-pr1"

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
