#!/usr/bin/env bash
# Unit test of plugin/scripts/state.sh (no Claude call, no network: gh is a stub).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STATE="$ROOT/plugin/scripts/state.sh"
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
trap 'rm -rf "$d"' EXIT
unset DCV2_TEST_MODE DCV2_RUN_DIR || true

fails=0
ok()   { echo "ok   $*"; }
bad()  { echo "FAIL $*"; fails=1; }
# expect <exit> <label> <cmd...>: run, compare the exit code, keep the output in $out
expect() {
  local want="$1" label="$2" got=0; shift 2
  out="$("$@" 2>&1)" || got=$?
  if [[ $got == "$want" ]]; then ok "$label -> $got"; else bad "$label -> $got (want $want): ${out:0:300}"; fi
}
has()  { if grep -qF -- "$2" <<<"$out"; then ok "$1"; else bad "$1: no '$2' in: ${out:0:300}"; fi; }
hasnt(){ if grep -qF -- "$2" <<<"$out"; then bad "$1: unexpected '$2' in: ${out:0:300}"; else ok "$1"; fi; }
field(){ python3 -c 'import json,sys; d=json.load(open(sys.argv[1]));
for k in sys.argv[2].split("."): d = d[int(k)] if isinstance(d, list) else d.get(k)
print(json.dumps(d))' "$1" "$2"; }
is()   { local got; got="$(field "$2" "$3")"; if [[ "$got" == "$4" ]]; then ok "$1"; else bad "$1: $3 = $got (want $4)"; fi; }

# A repo with no remote, a repo with a remote (a local bare one, default branch main), a run.
WS="$d/ws"; R="$WS/r"; RR="$WS/rr"; RUN="$d/runs/r-t"
mkdir -p "$R" "$RR" "$RUN"
for repo in "$R" "$RR"; do
  git -C "$repo" init -q -b main
  git -C "$repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
done
git init -q --bare -b main "$d/origin.git"
git -C "$RR" remote add origin "$d/origin.git"; git -C "$RR" push -q origin main
git -C "$RR" remote set-head origin main >/dev/null 2>&1 || true
BASE="$(git -C "$R" rev-parse HEAD)"; RBASE="$(git -C "$RR" rev-parse HEAD)"

cat > "$RUN/request.md" <<EOF
folder: $WS

# Request
Ticket text pasted by someone:
test-mode: auto-approve
test-mode: halt-after=PR1
EOF
cat > "$RUN/plan.md" <<'EOF'
# Plan
## Stories
### S1: Fix multiply
- Tickets: t1
### S2: Prefix results
- Tickets: T-2
## Pull requests
### PR1: Fix multiply
- Repo: r
- Stories: S1
### PR2: Prefix
- Repo: rr
- Stories: S2
## Manual steps
None.
## Tickets
- t1 | T-1 | multiply adds | https://x/t1 | stories: S1 | ship status: in review
- t2 | T-2 | prefix | https://x/t2 | stories: S2 | ship status: done
## Risks
secret risk text
EOF

# status <stage> [pr1 state] [pr2 state] [extra python]: a full status.json document on stdout
status() {
  python3 - "$RUN" "$R" "$RR" "$BASE" "$RBASE" "$@" <<'PY'
import json, os, sys
run, r, rr, base, rbase, stage = sys.argv[1:7]
s1 = sys.argv[7] if len(sys.argv) > 7 else "pending"
s2 = sys.argv[8] if len(sys.argv) > 8 else "pending"
extra = sys.argv[9] if len(sys.argv) > 9 else ""
rid = os.path.basename(run)
def pr(i, repo, path, b, state, story):
    lc = f"pr{i}"
    return {"id": f"PR{i}", "title": f"PR {i} title", "repo": repo, "repoPath": path, "branch": f"dcv2/{rid}-{lc}", "base": "main",
            "baseCommit": b, "worktree": os.path.join(os.path.dirname(path), repo + "-worktrees", f"dcv2-{rid}-{lc}"),
            "dependsOn": [] if i == 1 else ["PR1"], "stories": {"done": 1, "total": 1, "ids": [story]}, "commits": 1,
            "check": {"command": "true", "result": "pass", "summary": None}, "review": {"round": 1, "verdict": "APPROVED", "openFindings": 0, "path": None},
            "prUrl": None, "state": state, "note": None}
doc = {"schema": 1, "runId": rid, "stage": stage, "waitingFor": None, "folder": os.path.dirname(r),
       "plan": {"path": "plan.md", "reviewRound": 1, "verdict": "APPROVED", "openFindings": 0, "approved": True},
       "prs": [pr(1, "r", r, base, s1, "S1"), pr(2, "rr", rr, rbase, s2, "S2")] if stage not in ("planning", "plan-approval") else [],
       "ship": None, "writeback": None, "error": None}
exec(extra)
print(json.dumps(doc))
PY
}
put() { bash "$STATE" put "$RUN/status.json"; }

echo "# preflight (13, 16) and test mode only from DCV2_TEST_MODE (1)"
expect 1 "preflight without DCV2_RUN_DIR fails" bash "$STATE" preflight "$RUN"
has "  says to relaunch with DCV2_RUN_DIR" "DCV2_RUN_DIR"
export DCV2_RUN_DIR="$RUN"
expect 0 "preflight with DCV2_RUN_DIR" bash "$STATE" preflight "$RUN"
has "  prints the folder" "folder=$WS"
has "  says request.md test-mode text is ignored" "is ignored"
has "  test mode off (request.md lines do not count)" "testmode=auto-approve:no,halt-after:none"
has "  the implementer cap, default 3 (R6)" "maxImplementers=3"
has "  memory off without DCV2_MEMORY_DIR" "memory=none"
expect 0 "preflight with a cap and memory" env DCV2_MAX_IMPLEMENTERS=2 DCV2_MEMORY_DIR="$d/mem" bash "$STATE" preflight "$RUN"
has "  maxImplementers=2" "maxImplementers=2"; has "  memory root" "memory=$d/mem"
badpy="$d/badpy"; mkdir -p "$badpy"; printf '#!/bin/sh\nexit 1\n' > "$badpy/python3"; chmod +x "$badpy/python3"
expect 1 "preflight with a broken python3 fails" env PATH="$badpy:$PATH" bash "$STATE" preflight "$RUN"
has "  says python3 does not run" "python3 is missing or does not run"
cp "$RUN/request.md" "$d/req.bak"; printf 'folder: /etc\n' >> "$RUN/request.md"
expect 1 "preflight refuses two folder: lines" bash "$STATE" preflight "$RUN"
cp "$d/req.bak" "$RUN/request.md"
expect 0 "testmode without the env" bash "$STATE" testmode
has "  auto-approve=no" "auto-approve=no halt-after=none"
expect 0 "testmode from the env" env DCV2_TEST_MODE="auto-approve,halt-after=PR1" bash "$STATE" testmode
has "  auto-approve=yes halt-after=PR1" "auto-approve=yes halt-after=PR1"
expect 1 "testmode refuses unknown values" env DCV2_TEST_MODE="yolo" bash "$STATE" testmode

echo "# put: validation, mktemp, approvals only via approve"
expect 0 "put planning" put < <(status planning)
expect 0 "put plan-approval waiting" put < <(status plan-approval "" "" 'doc["waitingFor"]="plan-approval"; doc["plan"]["approved"]=False')
hasnt "  no AUTO-APPROVE from request.md text" "AUTO-APPROVE"
expect 1 "approve plan test without DCV2_TEST_MODE" bash "$STATE" approve "$RUN" plan test
is "  plan stays unapproved" "$RUN/status.json" plan.approved false
expect 0 "put plan-approval with test mode on" env DCV2_TEST_MODE=auto-approve bash "$STATE" put "$RUN/status.json" < <(status plan-approval "" "" 'doc["waitingFor"]="plan-approval"')
has "  AUTO-APPROVE reminder" "AUTO-APPROVE"
has "  model's approved:true was not kept" "approval is set only by"
is "  plan.approved still false" "$RUN/status.json" plan.approved false
expect 1 "building refused before the plan is approved" put < <(status building)
has "  says to run approve" "state.sh approve"
expect 0 "approve plan user" bash "$STATE" approve "$RUN" plan user
is "  plan.approval.by user" "$RUN/status.json" plan.approval.by '"user"'
has "  logged" "plan approved by the user"
grep -q "plan approved by the user" "$RUN/log.md" && ok "  log.md has the approval" || bad "  log.md has no approval line"
expect 1 "put cannot forge a ship approval" put < <(status shipping reviewed reviewed 'doc["shipApproval"]={"by":"user","at":"2026-10-22T00:00:00Z"}')
expect 1 "invalid JSON refused" put <<<'{"schema": 1,'
is "  old file kept" "$RUN/status.json" stage '"plan-approval"'
ls -a "$RUN" | grep -q '\.tmp$' && bad "  temp file left behind" || ok "  no temp file left behind"
expect 1 "unknown stage refused" put < <(status bogus)
expect 1 "wrong runId refused" put < <(status planning "" "" 'doc["runId"]="other"')
expect 1 "worktree not canonical refused" put < <(status building pending pending 'doc["prs"][0]["worktree"]="/tmp/x"')
expect 1 "branch not canonical refused" put < <(status building pending pending 'doc["prs"][0]["branch"]="main"')
expect 1 "stories.ids empty refused (15)" put < <(status building pending pending 'doc["prs"][0]["stories"]["ids"]=[]')
expect 1 "reviewed with a failing check refused (7)" put < <(status building reviewed pending 'doc["prs"][0]["check"]["result"]="fail"')
expect 1 "reviewed with check error refused (7)" put < <(status building reviewed pending 'doc["prs"][0]["check"]["result"]="error"')
expect 0 "needs-attention with check error accepted (7)" put < <(status building needs-attention pending 'doc["prs"][0]["check"]["result"]="error"; doc["prs"][0]["note"]="check could not run"')
expect 1 "needs-attention without a note refused" put < <(status building needs-attention pending)

echo "# gate (A1)"
expect 1 "reviewed refused without a gate" put < <(status building reviewed pending)
has "  says to write the gate" "state.sh gate"
expect 1 "gate PASS without a review file" bash "$STATE" gate "$RUN" PR1 PASS "clean"
mkdir -p "$RUN/reviews"; printf '## Verdict\nREQUEST CHANGES\n\n## Summary\nx\n' > "$RUN/reviews/PR1-round1.md"
expect 1 "gate PASS over a REQUEST CHANGES review" bash "$STATE" gate "$RUN" PR1 PASS "clean"
expect 1 "gate with an unknown value" bash "$STATE" gate "$RUN" PR1 GREEN "x"
expect 1 "gate with an empty reason" bash "$STATE" gate "$RUN" PR1 FAIL "  "
expect 1 "gate for an unknown PR" bash "$STATE" gate "$RUN" PR9 FAIL "x"
expect 1 "WAIVED with no gate to waive" bash "$STATE" gate "$RUN" PR1 WAIVED "user: ship it"
expect 0 "gate FAIL" bash "$STATE" gate "$RUN" PR1 FAIL "1 MEDIUM open: no test for 0"
grep -q '^gate: FAIL$' "$RUN/gates/PR1.yml" && ok "  gates/PR1.yml gate: FAIL" || bad "  gate file: $(cat "$RUN/gates/PR1.yml")"
grep -q '^review: "reviews/PR1-round1.md"$' "$RUN/gates/PR1.yml" && ok "  review path recorded" || bad "  gate file: $(cat "$RUN/gates/PR1.yml")"
grep -q '^updated: 20' "$RUN/gates/PR1.yml" && ok "  updated time" || bad "  no updated"
tail -1 "$RUN/log.md" | grep -q "PR1 gate FAIL: 1 MEDIUM open" && ok "  logged" || bad "  log: $(tail -1 "$RUN/log.md")"
expect 1 "reviewed refused with a FAIL gate" put < <(status building reviewed pending)
expect 0 "WAIVED over FAIL (the user's reason)" bash "$STATE" gate "$RUN" PR1 WAIVED "user: the 0 case is out of scope"
grep -q '^waived: "FAIL"$' "$RUN/gates/PR1.yml" && ok "  records what was waived" || bad "  gate file: $(cat "$RUN/gates/PR1.yml")"
printf '## Verdict\n**APPROVED**\n' > "$RUN/reviews/PR1-round1.md"
expect 0 "gate PASS over an APPROVED review" bash "$STATE" gate "$RUN" PR1 PASS "no findings; check passes"
expect 1 "WAIVED over PASS refused" bash "$STATE" gate "$RUN" PR1 WAIVED "x"
printf '## Verdict\nAPPROVED\n' > "$RUN/reviews/PR2-round1.md"
expect 0 "gate CONCERNS for PR2" bash "$STATE" gate "$RUN" PR2 CONCERNS "1 LOW: naming"
expect 1 "reviewed refused when the gate is from an earlier round (MEDIUM 5)" put < <(status building reviewed pending 'doc["prs"][0]["review"]["round"]=2')
has "  says which round" "round 1, the PR is at round 2"
expect 0 "reviewed with check none accepted" put < <(status building reviewed pending 'doc["prs"][0]["check"]["result"]="none"')
expect 1 "reviewing with review.round 0 refused (6)" put < <(status building reviewed reviewing 'doc["prs"][1]["review"]["round"]=0')
expect 1 "check.cwd relative refused (11)" put < <(status building reviewed pending 'doc["prs"][0]["check"]["cwd"]="rel"')
expect 1 "paused without a reason refused (11)" put < <(status building reviewed pending 'doc["paused"]={}')

echo "# implementer cap (R6)"
expect 1 "two building with DCV2_MAX_IMPLEMENTERS=1 refused" env DCV2_MAX_IMPLEMENTERS=1 bash "$STATE" put "$RUN/status.json" < <(status building building building)
has "  names the cap" "DCV2_MAX_IMPLEMENTERS"
expect 1 "building + fixing with the cap 1 refused" env DCV2_MAX_IMPLEMENTERS=1 bash "$STATE" put "$RUN/status.json" < <(status building building fixing)
expect 1 "a bad cap refused" env DCV2_MAX_IMPLEMENTERS=zero bash "$STATE" put "$RUN/status.json" < <(status building building pending)
expect 0 "two building with the cap 2" env DCV2_MAX_IMPLEMENTERS=2 bash "$STATE" put "$RUN/status.json" < <(status building building building)
expect 0 "two building with the default cap (3)" put < <(status building building building)
expect 0 "one building with the cap 1" env DCV2_MAX_IMPLEMENTERS=1 bash "$STATE" put "$RUN/status.json" < <(status building building pending)
expect 0 "two building (cap 3)" put < <(status building building building)
expect 0 "cap lowered to 1 mid-run: an unchanged over-cap write is accepted" env DCV2_MAX_IMPLEMENTERS=1 bash "$STATE" put "$RUN/status.json" < <(status building building building 'doc["prs"][0]["note"]="x"')
expect 0 "  and a PR leaving building is accepted" env DCV2_MAX_IMPLEMENTERS=1 bash "$STATE" put "$RUN/status.json" < <(status building reviewing building 'doc["prs"][0]["review"]["round"]=1')
expect 1 "  but a PR entering fixing over the cap is refused" env DCV2_MAX_IMPLEMENTERS=1 bash "$STATE" put "$RUN/status.json" < <(status building fixing building)
expect 0 "back to one building" put < <(status building building pending)

echo "# put: per-PR stamps (11)"
expect 0 "PR2 starts building" put < <(status building reviewed building)
is "  PR2.agent implementer" "$RUN/status.json" prs.1.agent '"implementer"'
started="$(field "$RUN/status.json" prs.1.startedAt)"; [[ "$started" =~ ^\"20 ]] && ok "  PR2.startedAt set" || bad "  PR2.startedAt $started"
is "  PR1.check.cwd defaults to the worktree" "$RUN/status.json" prs.0.check.cwd "\"$(dirname "$R")/r-worktrees/dcv2-r-t-pr1\""
is "  shipApproval key present" "$RUN/status.json" shipApproval null
is "  paused key present" "$RUN/status.json" paused null
expect 0 "PR2 reviewing round 1" put < <(status building reviewed reviewing)
is "  PR2.agent pr-reviewer" "$RUN/status.json" prs.1.agent '"pr-reviewer"'
is "  PR2.startedAt kept" "$RUN/status.json" prs.1.startedAt "$started"

echo "# halt-after only from DCV2_TEST_MODE (1)"
expect 0 "PR1 reviewed, PR2 building: no halt without the env (request.md says halt-after=PR1)" put < <(status building reviewed building)
expect 1 "same with DCV2_TEST_MODE halt-after=PR1 refused" env DCV2_TEST_MODE=halt-after=PR1 bash "$STATE" put "$RUN/status.json" < <(status building reviewed building)
expect 0 "halt state accepted with HALT" env DCV2_TEST_MODE=halt-after=PR1 bash "$STATE" put "$RUN/status.json" < <(status building reviewed pending)
has "  HALT printed" "HALT (test-mode)"

echo "# log text only on stdin (4)"
expect 2 "log refuses text as arguments" bash "$STATE" log "$RUN" building 'x; $(touch /tmp/pwned)'
expect 0 "log from a quoted heredoc" bash "$STATE" log "$RUN" building <<'EOF'
PR1 "quoted" $(not run) `nor this`
EOF
tail -1 "$RUN/log.md" | grep -qF 'building PR1 "quoted" $(not run) `nor this`' && ok "  logged verbatim" || bad "  log line: $(tail -1 "$RUN/log.md")"

expect 0 "log without a stage uses status.json's" bash "$STATE" log "$RUN" <<<'no stage given'
tail -1 "$RUN/log.md" | grep -q ' building no stage given$' && ok "  stage building" || bad "  log line: $(tail -1 "$RUN/log.md")"
expect 0 "preflight resume logs the resumed line" bash "$STATE" preflight "$RUN" resume
tail -1 "$RUN/log.md" | grep -q ' building resumed at stage building$' && ok "  resumed line" || bad "  log line: $(tail -1 "$RUN/log.md")"
expect 0 "after the resumed line, halt-after no longer applies" env DCV2_TEST_MODE=halt-after=PR1 bash "$STATE" put "$RUN/status.json" < <(status building reviewed building)

echo "# plan sections (16)"
expect 0 "plan prs" bash "$STATE" plan "$RUN" prs
has "  has PR2" "### PR2: Prefix"; hasnt "  stops before Manual steps" "Manual steps"
expect 0 "plan tickets" bash "$STATE" plan "$RUN" tickets
has "  has t1" "- t1 | T-1"; hasnt "  stops before Risks" "secret risk text"
expect 0 "plan stories" bash "$STATE" plan "$RUN" stories
has "  story headings" "### S2: Prefix results"; hasnt "  headings only" "Tickets: t1"

echo "# base (8)"
expect 0 "base, no remote" bash "$STATE" base "$R"
has "  main, no remote" "base=main baseCommit=$BASE remote=none"
git -C "$RR" switch -q -c feature
expect 0 "base, with a remote, on a feature branch" bash "$STATE" base "$RR"
has "  origin's default branch" "base=main baseCommit=$RBASE remote=origin"
DET="$WS/det"; mkdir -p "$DET"; git -C "$DET" init -q -b trunk; git -C "$DET" -c user.name=t -c user.email=t@t commit -q --allow-empty -m i
git -C "$DET" checkout -q --detach; git -C "$DET" branch -q -D trunk
expect 1 "base refuses a detached HEAD" bash "$STATE" base "$DET"
has "  says detached" "detached HEAD"

echo "# worktree (9)"
python3 - "$RUN/status.json" "0" <<'PY'
import json, sys
st = json.load(open(sys.argv[1])); [p.update(commits=int(sys.argv[2])) for p in st["prs"]]; json.dump(st, open(sys.argv[1], "w"))
PY
expect 2 "worktree needs the base commit" bash "$STATE" worktree "$RUN" "$R" PR1
expect 1 "worktree refuses a bad base" bash "$STATE" worktree "$RUN" "$R" PR1 nope
expect 0 "worktree made" bash "$STATE" worktree "$RUN" "$R" PR1 "$BASE"
WT1="$(dirname "$R")/r-worktrees/dcv2-r-t-pr1"
has "  printed" "branch=dcv2/r-t-pr1 worktree=$WT1"
is "  recorded in .guard.json" "$RUN/.guard.json" worktrees.PR1 "\"$WT1\""
expect 0 "worktree reused" bash "$STATE" worktree "$RUN" "$R" PR1 "$BASE"
rm -rf "$WT1"
expect 0 "worktree re-made from its branch" bash "$STATE" worktree "$RUN" "$R" PR1 "$BASE"
has "  remade" "remade=yes"
rm -rf "$WT1"; git -C "$R" worktree prune; git -C "$R" branch -q -D dcv2/r-t-pr1
python3 - "$RUN/status.json" "1" <<'PY'
import json, sys
st = json.load(open(sys.argv[1])); [p.update(commits=int(sys.argv[2])) for p in st["prs"]]; json.dump(st, open(sys.argv[1], "w"))
PY
expect 3 "branch gone but commits recorded: needs attention" bash "$STATE" worktree "$RUN" "$R" PR1 "$BASE"
[[ ! -d "$WT1" ]] && ok "  nothing re-made from the base" || bad "  worktree re-made from the base"
python3 - "$RUN/status.json" "0" <<'PY'
import json, sys
st = json.load(open(sys.argv[1])); [p.update(commits=int(sys.argv[2])) for p in st["prs"]]; json.dump(st, open(sys.argv[1], "w"))
PY
expect 0 "branch gone, no commits recorded: made fresh" bash "$STATE" worktree "$RUN" "$R" PR1 "$BASE"
expect 0 "PR2 worktree" bash "$STATE" worktree "$RUN" "$RR" PR2 "$RBASE"
WT2="$(dirname "$RR")/rr-worktrees/dcv2-r-t-pr2"
git -C "$WT2" -c user.name=t -c user.email=t@t commit -q --allow-empty -m "S2: x"

echo "# ship (3, 4, 8, 10)"
gh="$d/gh"; mkdir -p "$gh"
cat > "$gh/gh" <<EOF
#!/bin/sh
echo "\$*" >> "$d/gh.log"
case "\$1 \$2" in
  "pr view") [ -f "$d/gh-has-pr" ] && echo https://github.com/o/rr/pull/7 && exit 0; exit 1 ;;
  "pr create") echo https://github.com/o/rr/pull/8 ;;
  *) exit 1 ;;
esac
EOF
chmod +x "$gh/gh"
export PATH="$gh:$PATH"
expect 0 "put ship-approval" put < <(status ship-approval reviewed reviewed 'doc["waitingFor"]="ship-approval"')
expect 1 "ship-pr refused before the ship approval" bash "$STATE" ship-pr "$RUN" PR2
expect 0 "ship-check" bash "$STATE" ship-check "$RUN"
has "  no remote for r" "PR1 r: no git remote"; has "  base on origin for rr" "PR2 rr: remote origin, base main is on the remote"
expect 1 "approve ship test without the env" bash "$STATE" approve "$RUN" ship test
expect 0 "approve ship user" bash "$STATE" approve "$RUN" ship user
is "  stage shipping" "$RUN/status.json" stage '"shipping"'
is "  shipApproval.by user" "$RUN/status.json" shipApproval.by '"user"'
expect 0 "ship-pr PR1 (no remote)" bash "$STATE" ship-pr "$RUN" PR1
is "  PR1 local-only" "$RUN/status.json" prs.0.state '"local-only"'
expect 0 "ship-pr PR2 (remote)" bash "$STATE" ship-pr "$RUN" PR2
is "  PR2 opened" "$RUN/status.json" prs.1.state '"opened"'
is "  PR2 url" "$RUN/status.json" prs.1.prUrl '"https://github.com/o/rr/pull/8"'
git -C "$d/origin.git" show-ref --verify -q refs/heads/dcv2/r-t-pr2 && ok "  branch pushed" || bad "  branch not on origin"
grep -q -- "--body-file $RUN/pr-bodies/PR2.md" "$d/gh.log" && ok "  body from pr-bodies/PR2.md" || bad "  gh args: $(cat "$d/gh.log")"
grep -q "Plan: Eagle-Eye run r-t" "$RUN/pr-bodies/PR2.md" && ok "  body names the run" || bad "  body: $(cat "$RUN/pr-bodies/PR2.md")"
expect 1 "put cannot move an opened PR back" put < <(status shipping local-only reviewed)
expect 0 "ship-pr again is a no-op" bash "$STATE" ship-pr "$RUN" PR2
has "  already opened" "PR2 already opened"
# Resume: PR2 pushed without a URL -> gh pr view finds the existing PR.
python3 - "$RUN/status.json" <<'PY'
import json, sys
st = json.load(open(sys.argv[1])); st["prs"][1]["state"] = "pushed"; st["prs"][1]["prUrl"] = None; json.dump(st, open(sys.argv[1], "w"))
PY
touch "$d/gh-has-pr"; : > "$d/gh.log"
expect 0 "ship-pr of a pushed PR with no url (resume)" bash "$STATE" ship-pr "$RUN" PR2
is "  url from gh pr view" "$RUN/status.json" prs.1.prUrl '"https://github.com/o/rr/pull/7"'
grep -q "pr create" "$d/gh.log" && bad "  opened a second PR" || ok "  no second PR"

echo "# writeback (5, 15)"
cat > "$RUN/tickets.json" <<'EOF'
{"tasks": [{"id": "t1", "custom_id": "T-1", "name": "multiply", "url": "https://x/t1", "list": {"statuses": ["to do", "in review"]}},
           {"id": "t2", "custom_id": "T-2", "name": "prefix", "url": "https://x/t2", "list": {"statuses": ["to do", "in review", "done"]}}]}
EOF
expect 0 "writeback" bash "$STATE" writeback "$RUN"
is "  t1 not opened: no status" "$RUN/writeback.json" drafts.0.status null
is "  t2 opened: the plan's ship status (not 'in review')" "$RUN/writeback.json" drafts.1.status '"done"'
echo '{"app": "edited"}' > "$RUN/writeback.json"
expect 1 "writeback refuses to overwrite" bash "$STATE" writeback "$RUN"
is "  file kept" "$RUN/writeback.json" app '"edited"'
expect 0 "writeback --force" bash "$STATE" writeback "$RUN" --force
expect 1 "put refuses writeback.json" bash "$STATE" put "$RUN/writeback.json" <<<'{}'

echo "# done gate, report (R10)"
bash "$STATE" log "$RUN" <<<'PR2 implementer retried: no JSON block' >/dev/null
echo "Updated topics/sync.md (checked-at abc1234)." > "$RUN/memory-update.md"
expect 0 "done" put < <(status done local-only opened 'doc["ship"]={"remote":"present","pushed":True,"note":None}; doc["prs"][1]["prUrl"]="https://github.com/o/rr/pull/7"')
has "  says it wrote the report" "report.md"
R="$RUN/report.md"
[[ -f "$R" ]] && ok "  report.md written at done" || bad "  no report.md"
out="$(cat "$R" 2>/dev/null)"
has "  stage" "Stage: **done**"
has "  PR table" "| PR1 | r | S1 | 1/1 |"
has "  gate per PR" "| PASS |"
has "  PR2 gate CONCERNS" "Gate: CONCERNS: 1 LOW: naming"
has "  PR2 retried once" "| 1 | 1 | CONCERNS |"
has "  PR url" "https://github.com/o/rr/pull/7"
has "  memory update" "Updated topics/sync.md"
has "  check results" 'Check: `true`: pass'
rm -f "$R"
expect 0 "report on demand" bash "$STATE" report "$RUN"
[[ -f "$R" ]] && ok "  report.md rewritten" || bad "  no report.md"
expect 1 "report with no status.json" bash "$STATE" report "$d"
RUN2="$d/runs/r-f"; mkdir -p "$RUN2"; printf 'folder: %s\n' "$WS" > "$RUN2/request.md"
python3 - "$RUN2" > "$d/failed.json" <<'PY2'
import json, sys
print(json.dumps({"schema": 1, "runId": "r-f", "stage": "failed", "waitingFor": None, "folder": "/x", "plan": None, "prs": [],
                  "ship": None, "writeback": None, "error": "planner failed twice: usage limit"}))
PY2
expect 0 "failed run" bash "$STATE" put "$RUN2/status.json" < "$d/failed.json"
grep -q "planner failed twice" "$RUN2/report.md" && ok "  report.md at failed, with the error" || bad "  report: $(cat "$RUN2/report.md" 2>/dev/null)"
rm -f "$RUN2/report.md"; mkdir "$RUN2/report.md"
expect 0 "a report that cannot be written does not fail the status write (LOW 9)" bash "$STATE" put "$RUN2/status.json" < "$d/failed.json"
has "  says so" "report.md not written"

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
