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
TRUST="$d/run-trust/r-x"; mkdir -p "$TRUST"                                                   # <app data>/run-trust (RUN is <d>/runs/r-x)
for who in "" dcv2:implementer dcv2:planner dcv2:pr-reviewer; do
  w 2 "$who" "$TRUST/trust.json"
  w 2 "$who" "$TRUST/../r-other/tickets.json"
  b 2 "$who" "echo {} > $TRUST/posted.json"
  b 2 "$who" "cp /dev/null $TRUST/tickets.json.mac"
  b 2 "$who" "rm -rf $d/run-trust"
done
b 2 "" 'echo x > "$HOME/x/run-trust/r-x/trust.json"'
expect 0 "user session (no run) may write run-trust" "$(payload "" Write file_path "$TRUST/trust.json")" -u DCV2_RUN_DIR
for f in tickets.json meta.json writeback-posted.json tickets-pulls.json tickets-update.md app-checks.json; do
  w 2 "" "$RUN/$f"
  w 2 dcv2:planner "$RUN/$f"
  b 2 dcv2:implementer "echo x > $RUN/$f"
done
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
expect 0 "the bare word memory does not count as mentioning it" "$(payload "" Bash command 'cd "$(dirname "$X")" && echo "memory notes" > notes.txt')" -u DCV2_RUN_DIR DCV2_MEMORY_DIR="$MEMD"
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

echo "# the orchestrator runs no GitHub-writing gh (gap closed)"
b 2 "" "gh api repos/o/r/issues/1/comments -f body=x"
b 2 "" "gh api -X POST repos/o/r/issues/1/comments"
b 2 "" "gh api --method=PATCH repos/o/r/pulls/1"
b 2 "" "gh api -XDELETE repos/o/r/git/refs/heads/x"
b 2 "" "gh api graphql -F query=@q.graphql"
b 2 "" "gh api repos/o/r/pulls/1 --input body.json"
b 0 "" "gh api repos/o/r/pulls"
b 0 "" "gh api -X GET repos/o/r/pulls/1/comments"
b 2 "" "gh issue comment 3 --body x"
b 2 "" "gh -R o/r pr comment 1 --body x"
b 2 "" "gh release create v1"
b 2 "" "gh repo delete o/r --yes"
b 2 "" "gh pr ready 1"
b 2 "" "gh pr review 1 --approve"
b 0 "" "gh pr list --head dcv2/r-x-pr1"
b 0 "" "gh pr view 1 --json url"
b 0 "" "gh repo view --json defaultBranchRef"
expect 0 "user session (no run): gh api writes are theirs" "$(payload "" Bash command "gh api -X POST repos/o/r/issues")" -u DCV2_RUN_DIR

echo "# keychain: never read the app's items in a run"
for who in "" dcv2:implementer dcv2:pr-reviewer; do
  b 2 "$who" "security find-generic-password -s com.jtfrancisco.dcterminalv2.trust -w"
  b 2 "$who" "security dump-keychain -d"
  b 2 "$who" "echo \$(security find-generic-password -a x -s DCTerminalV2-clickup -w)"
done
expect 0 "user session (no run): security is theirs" "$(payload "" Bash command "security dump-keychain")" -u DCV2_RUN_DIR

echo "# meta.json (app-written) for everyone"
b 2 "" "echo '{\"reviewMode\":\"github\"}' > $RUN/meta.json"
b 2 "" "cp /tmp/m.json $RUN/meta.json"
w 2 "" "$RUN/meta.json"

echo "# github review mode (meta.json reviewMode github)"
echo '{"schema": 1, "reviewMode": "github"}' > "$RUN/meta.json"
python3 - "$RUN/status.json" <<'PY'
import json, sys
json.dump({"schema": 1, "stage": "building", "plan": {"approval": None}, "shipApproval": None}, open(sys.argv[1], "w"))
PY
b 2 "" "bash $STATE draft-pr $RUN PR1"                                 # before the plan approval
b 2 "" "bash \"\${CLAUDE_PLUGIN_ROOT}/scripts/state.sh\" post-review $RUN PR1 1"
python3 - "$RUN/status.json" <<'PY'
import json, sys
json.dump({"schema": 1, "stage": "building", "plan": {"approval": {"by": "user", "at": "2026-10-24T00:00:00Z"}}, "shipApproval": None}, open(sys.argv[1], "w"))
PY
b 0 "" "bash $STATE draft-pr $RUN PR1"
b 0 "" "bash \"\${CLAUDE_PLUGIN_ROOT}/scripts/state.sh\" post-review $RUN PR1 1"
b 0 "" "bash $STATE push $RUN PR1"
b 0 "" "bash $STATE comment $RUN PR1 fixes 1"
b 2 "" "bash $STATE ready $RUN PR1"                                    # before the ship approval
b 2 "" "git -C $WT push -u origin dcv2/r-x-pr1"                         # only through state.sh
b 2 "" "gh pr create --draft --base main --head dcv2/r-x-pr1 --title x --body-file /dev/null"
b 2 "" "gh pr comment 1 --body x"
b 2 dcv2:implementer "bash $STATE draft-pr $RUN PR1 && gh pr create --draft"
python3 - "$RUN/status.json" <<'PY'
import json, sys
json.dump({"schema": 1, "stage": "shipping", "plan": {"approval": {"by": "user", "at": "2026-10-24T00:00:00Z"}},
           "shipApproval": {"by": "user", "at": "2026-10-24T01:00:00Z"}}, open(sys.argv[1], "w"))
PY
b 0 "" "bash $STATE ready $RUN PR1"
b 2 "" "git -C $WT push -u origin dcv2/r-x-pr1"                         # still only through state.sh
b 2 "" "gh pr create --base main --head dcv2/r-x-pr1 --title x --body-file /dev/null"
b 2 "" "gh pr ready 1"
echo '{"schema": 1}' > "$RUN/meta.json"

echo "# BMAD runtime (run folder and plugin path contain spaces)"
SRUN="$d/App Support/runs/r-s"; PLUG="$d/Plugin Dir/plugin"
mkdir -p "$SRUN/bmad/output" "$SRUN/bmad/_bmad" "$PLUG/bmad"
printf 'folder: %s/ws\n' "$d" > "$SRUN/request.md"; echo '{}' > "$SRUN/status.json"
bx() { local want="$1" who="$2" cmd="$3"                                                      # <exit> <agent> <cmd>, bound to the spaced run
  expect "$want" "$who Bash: ${cmd//$d/}" "$(payload "$who" Bash command "$cmd" "$SRUN")" DCV2_RUN_DIR="$SRUN"; }
bw() { expect "$1" "$2 Write ${3//$d/}" "$(payload "$2" Write file_path "$3" "$SRUN")" DCV2_RUN_DIR="$SRUN"; }  # <exit> <agent> <path>
RT="$PLUG/bmad/runtime.mjs"
CMDQ="DCV2_RUN_DIR=\"$SRUN\" node \"$RT\" resolve_config --project-root \"$SRUN/bmad\" --key core.project_name"
bx 0 dcv2:planner "node \"$RT\" resolve_config --project-root \"$SRUN/bmad\""
bx 0 planner "$CMDQ"
bx 0 dcv2:planner "node '$RT' memlog init --workspace '$SRUN/bmad/output/r-s/spec-x'"
bx 0 dcv2:planner 'node "${CLAUDE_PLUGIN_ROOT}/bmad/runtime.mjs" resolve_config --project-root "$DCV2_RUN_DIR/bmad"'
for who in dcv2:implementer dcv2:pr-reviewer dcv2:plan-reviewer dcv2:memory-updater ""; do
  bx 2 "$who" "node \"$RT\" resolve_config --project-root \"$SRUN/bmad\""
done
bx 2 "" 'node "${CLAUDE_PLUGIN_ROOT}/bmad/runtime.mjs" lint_spine --workspace x'
bx 2 dcv2:implementer "cd \"$PLUG/bmad\" && node runtime.mjs memlog init --workspace x"
bx 2 dcv2:implementer "bash -c 'node \"$RT\" knowledge --root /x'"
bx 2 dcv2:implementer "env DCV2_RUN_DIR=x node \"$RT\" knowledge --root /x"
bx 0 dcv2:implementer "node --version"
bx 0 "" "node script.mjs"
for who in dcv2:planner dcv2:implementer ""; do                                               # the bundle behind the wrapper: nobody
  bx 2 "$who" "node \"$PLUG/bmad/ade-runtime.mjs\" resolve_config --project-root x"
  bx 2 "$who" "cd \"$PLUG/bmad\" && node ade-runtime.mjs memlog init"
  bx 2 "$who" "cat \"$PLUG/bmad/ade-runtime.mjs\""
done
bw 0 dcv2:planner "$SRUN/bmad/output/r-s/spec-x/spec-x.md"
bw 0 dcv2:planner "$SRUN/bmad/output/x.md"
bw 2 dcv2:planner "$R/x.md"                                                                  # a repo is still not writable
bw 2 dcv2:planner "$d/ws/x.md"
bw 2 dcv2:planner "$SRUN/bmad/_bmad/config.toml"                                              # only state.sh bmad-init writes it
bw 2 dcv2:planner "$SRUN/bmad/_bmad/config.user.toml"
bw 2 dcv2:planner "$SRUN/bmad/_bmad/custom/bmad-spec.toml"
bw 2 "" "$SRUN/bmad/_bmad/config.toml"
bw 2 dcv2:implementer "$SRUN/bmad/_bmad/config.toml"
bw 0 dcv2:planner "$SRUN/bmad/_bmad/notes.md"
bx 2 dcv2:planner "echo x > \"$SRUN/bmad/_bmad/config.toml\""
bx 2 dcv2:planner "cp /tmp/x.toml \"$SRUN/bmad/_bmad/config.toml\""
bx 0 dcv2:planner "echo x > \"$SRUN/bmad/output/x.md\""
bx 2 dcv2:planner "echo x > \"$R/x.md\""
bx 0 "" "bash \"$PLUG/scripts/state.sh\" bmad-check \"$SRUN\""

b 2 "" "bash $STATE draft-pr $RUN PR1"                                 # local mode: no drafts
b 0 "" "git -C $WT push -u origin dcv2/r-x-pr1"                         # local mode: today's ship rule

echo "# files copied in from the main checkout are never staged (copyFiles)"
python3 - "$RUN/.guard.json" <<'PY'
import json, sys
g = json.load(open(sys.argv[1])); g["copied"] = {"PR1": [".env", "config/local.json"]}; json.dump(g, open(sys.argv[1], "w"))
PY
b 2 dcv2:implementer "git -C $WT add .env"
b 2 dcv2:implementer "git -C $WT add -f .env"
b 2 dcv2:implementer "git -C $WT add $WT/config/local.json"
b 2 dcv2:implementer "cd $WT && git add -f ."
b 2 dcv2:implementer "cd $WT && git add -f config"
b 2 dcv2:implementer "git -C $WT commit -m x -- .env"
b 0 dcv2:implementer "git -C $WT add src/a.js && git -C $WT commit -m 'S1: x'"
b 0 dcv2:implementer "cd $WT && git add -A"
b 0 dcv2:implementer "git -C $WT add config/other.json"

echo "# review fixes (PR #9)"
echo '{"schema": 1, "reviewMode": "github"}' > "$RUN/meta.json"
python3 - "$RUN/status.json" <<'PY'
import json, sys
json.dump({"schema": 1, "stage": "shipping", "plan": {"approval": {"by": "user", "at": "2026-10-24T00:00:00Z"}},
           "shipApproval": {"by": "user", "at": "2026-10-24T01:00:00Z"}}, open(sys.argv[1], "w"))
PY
for who in dcv2:implementer dcv2:pr-reviewer dcv2:planner implementer; do                      # MED 2
  for sub in draft-pr push "post-review" "comment" ready ship-pr; do
    b 2 "$who" "bash $STATE $sub $RUN PR1 1"
  done
done
b 0 dcv2:implementer "bash $STATE plan $RUN prs"
b 2 dcv2:pr-reviewer "$STATE post-review $RUN PR1 1"                                       # MED 3: by path, no bash
b 2 dcv2:pr-reviewer "/some/other/copy/scripts/state.sh ready $RUN PR1"
b 2 dcv2:implementer "sh ./state.sh draft-pr $RUN PR1" "$ROOT/plugin/scripts"
b 0 "" "$STATE ready $RUN PR1"
python3 - "$RUN/status.json" <<'PY'
import json, sys
json.dump({"schema": 1, "stage": "building", "plan": {"approval": {"by": "user", "at": "2026-10-24T00:00:00Z"}}, "shipApproval": None}, open(sys.argv[1], "w"))
PY
b 2 "" "$STATE ready $RUN PR1"                                                             # by path: still gated
b 0 "" "$STATE draft-pr $RUN PR1"
for c in "gh api -iX PUT repos/o/r/x" "gh api -iXPUT repos/o/r/x" "gh api -XPATCH repos/o/r/x" "gh api --method POST repos/o/r/x" \
         "gh api --method=delete repos/o/r/x" "gh api -X put repos/o/r/x" "gh api -if body=x repos/o/r/x" "gh api --raw-field=a=b x"; do   # MED 4
  b 2 "" "$c"
done
b 0 "" "gh api -iX GET repos/o/r/pulls"
b 0 "" "gh api -i repos/o/r/pulls"
w 2 "" "$RUN/repos.json"                                                                     # MED 5
b 2 "" "echo '{}' > $RUN/repos.json"
b 2 dcv2:implementer "cp /tmp/x $RUN/repos.json"
b 2 dcv2:implementer "git -C $WT remote set-url origin https://evil.example/x.git"          # MED 9
b 2 dcv2:implementer "git -C $WT remote add evil /tmp/evil.git"
b 2 dcv2:implementer "git -C $WT config core.hooksPath /tmp/h"
b 2 dcv2:implementer "cd $WT && git config remote.origin.url /tmp/evil.git"
b 0 dcv2:implementer "git -C $WT remote -v && git -C $WT config --get user.name"
for who in "" dcv2:implementer; do                                                           # LOW 12
  b 2 "$who" "security find-generic-password -s anything -w"
  b 2 "$who" "security find-internet-password -s github.com"
  b 2 "$who" "security export -k login.keychain"
done
echo '{"schema": 1}' > "$RUN/meta.json"

echo "# cleanup (0.5.0): only state.sh removes worktrees and branches"
for c in "git -C $R worktree remove $WT" "git -C $R worktree remove --force $WT" "git worktree prune" "git -C $WT worktree remove $WT" \
         "git -C $R worktree add /tmp/x" "git -C $R branch -d dcv2/r-x-pr1" "git -C $WT branch -D dcv2/r-x-pr1" \
         "git -C $R branch -m dcv2/r-x-pr1 x" "git -C $R push origin --delete dcv2/r-x-pr1" "git -C $WT push origin --delete dcv2/r-x-pr1" \
         "cd $R && git branch -d dcv2/r-x-pr1"; do
  b 2 "" "$c"
done
b 0 "" "git -C $R worktree list && git -C $R branch --list 'dcv2/*' && git -C $WT branch --show-current"
b 0 "" "bash $STATE cleanup $RUN"
b 0 "" "bash $STATE request $RUN recheck && bash $STATE request-wait $RUN 20261027T100000Z-recheck"
for who in dcv2:implementer dcv2:pr-reviewer dcv2:planner dcv2:plan-reviewer; do
  b 2 "$who" "bash $STATE cleanup $RUN"
  b 2 "$who" "bash $STATE request $RUN recheck"
done
for who in "" dcv2:implementer; do                                  # requests/ and the app's cleanup record
  w 2 "$who" "$RUN/requests/20261027T100000Z-recheck.result.json"
  w 2 "$who" "$RUN/requests/x.json"
  w 2 "$who" "$RUN/app-cleanup.json"
  b 2 "$who" "echo '{}' > $RUN/requests/20261027T100000Z-recheck.result.json"
  b 2 "$who" "mkdir -p $RUN/requests/a"
  b 2 "$who" "cp /dev/null $RUN/app-cleanup.json"
done
for who in "" dcv2:implementer; do                                  # the app's run record
  w 2 "$who" "$RUN/app-usage.json"
  b 2 "$who" "echo '{}' > $RUN/app-usage.json"
  b 2 "$who" "cp /dev/null $RUN/app-usage.json"
done
b 0 "" "cat $RUN/requests/20261027T100000Z-recheck.result.json"

echo "# BMAD runtime: path forms are normalised"
bx 2 dcv2:implementer "cd \"$PLUG/bmad\" && node ./runtime.mjs memlog init --workspace x"
bx 2 dcv2:implementer "node \"$PLUG/bmad/sub/../runtime.mjs\" knowledge --root /x"
bx 2 "" "cd \"$PLUG\" && node bmad/./runtime.mjs knowledge"
bx 2 "" "cd \"$PLUG/bmad/skills\" && node ../runtime.mjs knowledge"
bx 2 dcv2:implementer "cd \$HOME && node ./runtime.mjs x"                                 # unknown folder: treated as the runtime
bx 0 dcv2:planner "cd \"$PLUG/bmad\" && node ./runtime.mjs resolve_config --project-root \"$SRUN/bmad\""
bx 0 dcv2:implementer "cd \"$d/ws\" && node ./tools/runtime.mjs"                          # another project's runtime.mjs

echo "# live Claude tests (repos.json liveTests)"
mkdir -p "$WT/scripts/skill-e2e"
for who in dcv2:implementer dcv2:pr-reviewer dcv2:planner dcv2:plan-reviewer dcv2:memory-updater; do
  b 2 "$who" "claude -p hi"
  b 2 "$who" "cd $WT && env X=1 claude --version"
done
b 0 "" "claude --version"                                                                 # the orchestrator: unchanged
b 2 dcv2:implementer "cd $WT && bash scripts/skill-e2e/golden.sh github"                  # off by default
b 2 dcv2:pr-reviewer "bash $WT/scripts/skill-e2e/bmad_spike.sh"
printf '{"schema": 1, "repos": [{"name": "r", "path": "%s", "check": null, "liveTests": true}]}\n' "$R" > "$RUN/repos.json"
b 0 dcv2:implementer "cd $WT && bash scripts/skill-e2e/golden.sh github > $RUN/logs/PR1-golden.txt 2>&1"
b 0 dcv2:pr-reviewer "bash $WT/scripts/skill-e2e/bmad_spike.sh"
b 0 dcv2:implementer "$WT/scripts/skill-e2e/golden.sh fix"
b 2 dcv2:planner "cd $WT && bash scripts/skill-e2e/golden.sh github"                      # only the implementer / pr-reviewer
b 2 dcv2:plan-reviewer "bash $WT/scripts/skill-e2e/golden.sh"
b 2 dcv2:implementer "bash $R/scripts/skill-e2e/golden.sh"                                # the main checkout's copy
b 2 dcv2:implementer "bash $d/ws/r-worktrees/dcv2-r-x-pr2/scripts/skill-e2e/golden.sh"   # another PR's worktree
b 2 dcv2:implementer "bash $WT/golden.sh"                                                  # not the suite's path
b 2 dcv2:implementer 'cd "$X" && bash scripts/skill-e2e/golden.sh'
b 2 dcv2:implementer "cd $WT && claude -p 'run the suite'"                                # still never claude itself
printf '{"schema": 1, "repos": [{"name": "r", "path": "%s", "check": null, "liveTests": "yes"}]}\n' "$R" > "$RUN/repos.json"
b 2 dcv2:implementer "cd $WT && bash scripts/skill-e2e/golden.sh github"                  # only a JSON true counts
rm -f "$RUN/repos.json"

echo "# shared refs (review of #11): nobody changes worktrees, branches or refs with raw git"
O="$d/ws/r-worktrees/dcv2-other-pr1"
for who in dcv2:implementer dcv2:pr-reviewer ""; do
  for c in "git -C $WT worktree remove --force $O" "git -C $WT worktree prune" "git -C $WT worktree add /tmp/z" \
           "git -C $WT branch -D dcv2/other-pr1" "git -C $WT branch -D main" "git -C $WT branch -f main HEAD" \
           "git -C $WT update-ref -d refs/heads/main" "git -C $WT update-ref --stdin" "git -C $WT symbolic-ref HEAD refs/heads/main" \
           "git -C $WT checkout -B main" "git -C $WT switch -C main" "git -C $WT tag -d v1" "git -C $WT gc --prune=now" \
           "cd $WT && git branch -d dcv2/r-x-pr1" "timeout 60 git -C $WT update-ref -d refs/heads/main"; do
    b 2 "$who" "$c"
  done
done
b 0 dcv2:implementer "git -C $WT branch dcv2/r-x-pr1"                          # its own branch only
b 0 dcv2:implementer "git -C $WT checkout -b dcv2/r-x-pr1"
b 0 dcv2:implementer "git -C $WT checkout -- src/a.js && git -C $WT branch --show-current && git -C $WT symbolic-ref --short HEAD"
b 2 dcv2:implementer "git -C $WT checkout -b feature/x"
for c in "git -C $WT config remote.origin.url /tmp/evil.git" "git -C $WT config url.ext::sh.insteadOf https://" \
         "git -C $R config protocol.ext.allow always" "git -C $WT remote set-url origin -- --upload-pack=x"; do
  b 2 "" "$c"                                                                   # the orchestrator writes no git config
done
b 0 "" "git -C $WT config --get remote.origin.url && git -C $WT remote -v"

echo "# other runs' requests (review of #11)"
RB="$d/runs/r-b"; mkdir -p "$RB"; printf 'folder: %s/ws\n' "$d" > "$RB/request.md"
for who in "" dcv2:implementer; do
  w 2 "$who" "$RB/requests/20261027T000000Z-selftest.json"
  w 2 "$who" "$RB/app-cleanup.json"
  b 2 "$who" "echo '{}' > $RB/requests/20261027T000000Z-selftest.result.json"
done
b 2 "" "bash $STATE request $RB selftest"
b 2 "" "bash $STATE cleanup $RB"
b 2 "" "bash $STATE request-wait $RB 20261027T000000Z-selftest"
b 2 "" "bash -- $STATE cleanup $RB"
b 0 "" "bash $STATE request $RUN selftest"

echo "# claude and the live suites, any form (review of #11; liveTests off)"
for c in "bash -x scripts/skill-e2e/golden.sh github" "bash -- scripts/skill-e2e/golden.sh" "timeout 600 bash scripts/skill-e2e/golden.sh" \
         "npx claude -p hi" "npm exec claude -- -p hi" "cl''aude -p hi" "c=claude; \$c -p hi" "\$(command -v claude) -p hi" "timeout 5 claude -p hi" \
         "find . -maxdepth 0 -exec claude -p hi \;" "nice -n 5 claude -p hi" \
         "env -u X claude -p hi" "nohup claude -p hi" "command claude -p hi" "exec claude" "echo hi | xargs claude -p" \
         "source scripts/skill-e2e/golden.sh" ". scripts/skill-e2e/golden.sh" "/bin/bash scripts/skill-e2e/golden.sh" "bash ./scripts/skill-e2e/golden.sh" \
         "bash -o pipefail scripts/skill-e2e/golden.sh"; do
  b 2 dcv2:implementer "$c" "$WT"
done
b 0 dcv2:implementer "grep -rn claude src && cat src/claude.rs && timeout 600 npm run check" "$WT"   # naming claude is fine

echo "# re-review of #11: shell -c clusters, git aliases, fetch into branches, config reads, claude false positives"
for c in "bash -lc 'git -C $O worktree remove --force $O'" "bash -lc 'git push --force origin main'" "sh -ec 'git -C $WT push origin main'" \
         "bash -xc 'echo x > $RUN/status.json'" "zsh -c 'claude -p hi'" "dash -ec 'claude'" "bash -o pipefail -c 'claude -p hi'" \
         "bash -l -c 'claude -p hi'" "bash -lc -- 'claude -p hi'" "sh -lc 'claude -p hi'" \
         "git -c alias.x='update-ref -d refs/heads/main' x" "git -c 'alias.x=!rm -rf $O' x" "git --config-env=alias.x=EVIL x" \
         "git fetch . +HEAD:refs/heads/main" "git fetch . HEAD:refs/heads/dcv2/other-pr1" "git fetch origin main:main" "git pull origin +main:main" \
         "git notes add -m x" "npx --yes @anthropic-ai/claude-code -p hi" "bunx @anthropic-ai/claude-code -p hi" \
         "npm exec @anthropic-ai/claude-code -- -p hi" "/usr/bin/env -S 'claude -p hi'" "env --split-string='claude -p hi'" \
         "git config user.name x" "git config --unset user.name" "git config set user.name x"; do
  b 2 dcv2:implementer "$c" "$WT"
done
b 2 "" "git -C $WT fetch . +HEAD:refs/heads/main"
b 2 "" "sh -ec 'git -C $WT push --delete origin dcv2/r-x-pr1'"
b 2 "" "bash $STATE cleanup \$DCV2_RUN_DIR/../r-b"
b 0 "" "bash $STATE request \$DCV2_RUN_DIR recheck"
for who in "" dcv2:implementer; do
  b 2 "$who" "DCV2_TEST_LOCAL_REMOTES=1 bash $STATE cleanup $RUN"
done
for c in "git fetch origin" "git fetch origin main" "git fetch origin +refs/heads/main:refs/remotes/origin/main" "git fetch . HEAD:refs/heads/dcv2/r-x-pr1" \
         "git config user.name" "git config --get user.email" "git config --list" "git config get user.name" \
         "node scripts/check-claude.mjs" "python3 tools/claude-log.py" "python3 -m json.tool plugin/.claude-plugin/plugin.json" \
         "cat fixtures/claude-2.1.287/hooks.json" "bash -lc 'npm run check'" "bash -c 'git status'"; do
  b 0 dcv2:implementer "$c" "$WT"
done
b 0 dcv2:pr-reviewer "git config user.name && git -C $WT log --oneline -3" "$WT"

echo "# third review of #11: config values that look like options, git -c keys, git env"
for c in "git config alias.x '-c alias.y=!touch /tmp/p y'" "git config core.sshCommand '-oProxyCommand=x'" "git config --global user.name x" \
         "git -c core.sshCommand='rm -rf $O' fetch origin" "git -c core.pager=x -p log" "git -c core.editor=x commit" \
         "git -c core.fsmonitor=x status" "git -c 'Alias.x=!rm -rf $O' x" "git -calias.x='!rm -rf $O' x" "git -c commit.gpgsign=true commit -m x" \
         "git --config-env alias.x=EVIL x" "git --config-env=core.pager=EVIL log" "git --exec-path=/tmp/evil status" \
         "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.x GIT_CONFIG_VALUE_0='!rm -rf $O' git x" \
         "export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.x GIT_CONFIG_VALUE_0=x; git x" "GIT_SSH_COMMAND='rm -rf $O' git fetch" \
         "export GIT_SSH=/tmp/x" "env GIT_EXEC_PATH=/tmp/e git status" "GIT_CONFIG_PARAMETERS=\"'alias.x=!x'\" git x"; do
  b 2 dcv2:implementer "$c" "$WT"
done
b 2 "" "git -C $WT config remote.origin.url '--upload-pack=touch /tmp/p'"
b 2 "" "GIT_SSH_COMMAND=x git -C $WT log"
for c in "git -c user.name=t -c user.email=t@t commit -m x" "git -c color.ui=never log" "git -c advice.detachedHead=false status" \
         "git -c commit.gpgsign=false commit -m x" "git -cuser.name=t commit -m x" "git --exec-path" "git config user.name"; do
  b 0 dcv2:implementer "$c" "$WT"
done

echo "# fourth review of #11: git repo env, the user's git config, --show-origin reads"
for c in "GIT_DIR=$R/.git git reset --hard" "GIT_DIR=$R/.git GIT_WORK_TREE=$R git checkout -- ." "GIT_TEMPLATE_DIR=/tmp/t git init /tmp/x" \
         "export GIT_WORK_TREE=$R; git checkout -- ." "env GIT_INDEX_FILE=/tmp/i git add -A" "declare -x GIT_COMMON_DIR=$R/.git" \
         "typeset -x GIT_OBJECT_DIRECTORY=/tmp/o" "GIT_ALTERNATE_OBJECT_DIRECTORIES=/tmp/o git log" "GIT_NAMESPACE=x git log" \
         "env -S 'GIT_DIR=$R/.git git reset --hard'"; do
  b 2 dcv2:implementer "$c" "$WT"
done
for who in "" dcv2:implementer dcv2:planner; do
  b 2 "$who" "echo '[alias] x = !rm -rf $O' >> ~/.gitconfig"
  b 2 "$who" "echo x >> \$HOME/.gitconfig"
  b 2 "$who" "cp /tmp/x ~/.config/git/config"
  b 2 "$who" "tee -a ~/.config/git/ignore < /dev/null"
  w 2 "$who" "$HOME/.gitconfig"
  w 2 "$who" "$HOME/.config/git/config"
done
b 0 "" "cat ~/.gitconfig"
for c in "git config --show-origin user.name" "git config --show-scope user.name" "git config --list --show-origin"; do
  b 0 dcv2:implementer "$c" "$WT"
done
b 2 dcv2:implementer "git config --show-origin user.name x" "$WT"

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
