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

n0="$(wc -l < "$RUN/log.md")"
bash "$STATE" log "$RUN" <<<'PR9 draft PR opened: https://github.com/o/r/pull/9' >/dev/null
bash "$STATE" log "$RUN" <<<'PR9  draft PR opened: https://github.com/o/r/pull/9 ' >/dev/null
(( $(wc -l < "$RUN/log.md") == n0 + 1 )) && ok "  the same event twice in a row is logged once" || bad "  log: $(tail -3 "$RUN/log.md")"
bash "$STATE" log "$RUN" <<<'PR9 review round 1: APPROVED' >/dev/null
bash "$STATE" log "$RUN" <<<'PR9 draft PR opened: https://github.com/o/r/pull/9' >/dev/null
(( $(wc -l < "$RUN/log.md") == n0 + 3 )) && ok "  not a repeat when something came between" || bad "  log: $(tail -3 "$RUN/log.md")"

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
           {"id": "t2", "custom_id": "T-2", "name": "prefix", "url": "https://x/t2", "list": {"statuses": ["to do", "in review", "done"]}},
           {"id": "t3", "custom_id": null, "name": "nobody", "url": "https://x/t3", "list": {"statuses": ["to do", "done"]}}]}
EOF
expect 0 "writeback" bash "$STATE" writeback "$RUN"
is "  t1 not opened: no status" "$RUN/writeback.json" drafts.0.status null
is "  t2 opened: the plan's ship status (not 'in review')" "$RUN/writeback.json" drafts.1.status '"done"'
python3 -c 'import json,sys; d=json.load(open(sys.argv[1]))["drafts"]; sys.exit(0 if d[0]["comment"] and "S1" in d[0]["comment"] else 1)' "$RUN/writeback.json" && ok "  t1 has its comment" || bad "  t1 comment"
is "  t3 (no story): no comment drafted" "$RUN/writeback.json" drafts.2.comment null
is "  t3: no status" "$RUN/writeback.json" drafts.2.status null
is "  t3: its note" "$RUN/writeback.json" drafts.2.statusNote '"No PR covers this ticket."'
echo '{"app": "edited"}' > "$RUN/writeback.json"
expect 1 "writeback refuses to overwrite" bash "$STATE" writeback "$RUN"
is "  file kept" "$RUN/writeback.json" app '"edited"'
expect 0 "writeback --force" bash "$STATE" writeback "$RUN" --force
expect 1 "put refuses writeback.json" bash "$STATE" put "$RUN/writeback.json" <<<'{}'
expect 1 "put refuses requests/" bash "$STATE" put "$RUN/requests/20261027T100000Z-recheck.result.json" <<<'{"state": "done"}'
for f in tickets.json meta.json writeback-posted.json tickets-pulls.json tickets-update.md app-checks.json app-cleanup.json; do
  expect 1 "put refuses the app's $f" bash "$STATE" put "$RUN/$f" <<<'{}'
done

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

echo "# BMAD mode: size, init, check, and the plan-approval refusal (run folder path has a space)"
B="$d/App Support/runs/r-b"; mkdir -p "$B"
printf 'folder: %s\n' "$WS" > "$B/request.md"
printf '# Plan\n## Stories\n### S1: x\n## Pull requests\n### PR1: x\n- Repo: r\n- Stories: S1\n' > "$B/plan.md"
printf '{"schema":1,"repos":[{"name":"toy \\"quoted\\" repo","path":"%s","check":null}]}\n' "$R" > "$B/repos.json"
# bstatus <stage>: a minimal status.json document for the BMAD run (no PRs yet)
bstatus() {
  python3 - "$B" "$1" <<'PY'
import json, os, sys
run, stage = sys.argv[1:3]
print(json.dumps({"schema": 1, "runId": os.path.basename(run), "stage": stage, "waitingFor": "plan-approval" if stage == "plan-approval" else None,
                  "folder": "/x", "plan": {"path": "plan.md", "reviewRound": 1, "verdict": "APPROVED", "openFindings": 0, "approved": False},
                  "prs": [], "ship": None, "writeback": None, "error": None}))
PY
}
bput() { bash "$STATE" put "$B/status.json"; }
bmeta() { printf '{"schema":1,"bmad":{"mode":"%s","version":"bda3c59 6.13.0-next"}}\n' "$1" > "$B/meta.json"; }
bsize() { local sz="$1"; shift; bash "$STATE" bmad-size "$B" "$sz" "$@" <<<"reason one"; }
# mkdocs [prd] [ux]: valid spec + architecture (and PRD / UX with reports) under the spaced run path
mkdocs() {
  local w="$B/bmad/output/r-b"
  mkdir -p "$w/spec-toy" "$w/architecture-toy"
  printf -- '---\nid: SPEC-toy\n---\n\n# Toy spec\n\n## Capabilities\n\n- **CAP-1** users can add\n' > "$w/spec-toy/spec-toy.md"
  printf -- '---\nname: toy\nstatus: draft\n---\n\n# Spine\n\n## Invariants & Rules\n\n### AD-1 - Layers\n\n- **Binds:** all\n- **Prevents:** drift\n- **Rule:** UI never calls the DB\n' > "$w/architecture-toy/architecture-toy.md"
  local k
  for k in "$@"; do
    mkdir -p "$w/$k-toy"
    if [[ $k == prd ]]; then printf -- '---\nstatus: final\n---\n\n# PRD\n\nUsers add numbers.\n' > "$w/prd-toy/prd-toy.md"; touch -t 202601010000 "$w/prd-toy/prd-toy.md"
    else printf -- '---\nstatus: final\n---\n\n# Design\n\nCalm.\n' > "$w/ux-toy/DESIGN.md"; printf -- '---\nstatus: final\n---\n\n# Experience\n\nOne screen.\n' > "$w/ux-toy/EXPERIENCE.md"; touch -t 202601010000 "$w/ux-toy/DESIGN.md" "$w/ux-toy/EXPERIENCE.md"; fi
    printf '# Validation\nok\n' > "$w/$k-toy/validation-report.md"; printf '<html>ok</html>\n' > "$w/$k-toy/validation-report.html"
  done
}
export DCV2_RUN_DIR="$B"

# an old run: no bmad in meta.json (or no meta.json at all) behaves as before
expect 0 "bmad: put planning (no meta.json)" bput < <(bstatus planning)
expect 0 "bmad: preflight prints bmad=off and a node line" bash "$STATE" preflight "$B"
has "  bmad=off" "bmad=off"; has "  node line" "node="
expect 1 "bmad-size refused when the mode is off" bsize Large
has "  says off" "BMAD mode is off"
expect 1 "bmad-check refused when not applied" bash "$STATE" bmad-check "$B"
expect 0 "bmad: plan-approval puts as today (old run)" bput < <(bstatus plan-approval)
expect 0 "bmad: old run approves" bash "$STATE" approve "$B" plan user
bmeta nonsense
expect 1 "unknown mode reads as off" bsize Large
bmeta off
expect 1 "mode off: bmad-init refused" bash "$STATE" bmad-init "$B"

echo "# BMAD auto: size rules, init, the refusal"
rm -f "$B/status.json" "$B/log.md"; bmeta auto
expect 0 "auto: put planning" bput < <(bstatus planning)
expect 0 "auto: preflight" bash "$STATE" preflight "$B"
has "  bmad=auto" "bmad=auto"
expect 1 "auto, no size: plan-approval put refused" bput < <(bstatus plan-approval)
has "  says to size first" "BMAD auto: run the sizing pass"
expect 1 "auto, no size: approve refused" bash "$STATE" approve "$B" plan user
has "  same message" "state.sh bmad-size first"
expect 1 "bmad-init refused before a size" bash "$STATE" bmad-init "$B"
expect 1 "bmad-size refuses an unknown size" bsize Huge
expect 1 "bmad-size refuses unknown docs" bsize Medium --docs prd,bogus
expect 0 "size Medium" bsize Medium --by planner
is "  size" "$B/status.json" bmad.size '"Medium"'
is "  not applied" "$B/status.json" bmad.applied false
is "  mode recorded" "$B/status.json" bmad.mode '"auto"'
expect 1 "bmad-init refused when not applied" bash "$STATE" bmad-init "$B"
expect 1 "bmad-check refused when not applied" bash "$STATE" bmad-check "$B"
expect 1 "size cannot go down (Small)" bsize Small --by planner
has "  says only up" "BMAD size can only go up (recorded Medium)"
expect 0 "Small in auto skips BMAD: plan-approval put" bput < <(bstatus plan-approval)
expect 0 "Small/Medium in auto approves normally" bash "$STATE" approve "$B" plan user
expect 1 "bmad-size refused outside planning" bsize Large
has "  says planning" "stage planning"
# back to planning: Medium -> Large works, then nothing lowers it
rm -f "$B/status.json"; expect 0 "put planning again" bput < <(bstatus planning)
expect 0 "Medium" bsize Medium --docs prd
expect 0 "Medium -> Large" bsize Large --by plan-reviewer --docs ux
is "  Large" "$B/status.json" bmad.size '"Large"'
is "  applied" "$B/status.json" bmad.applied true
is "  docs only gain" "$B/status.json" bmad.docs '["prd", "ux"]'
for by in planner plan-reviewer none; do
  expect 1 "a recorded Large cannot be lowered (--by $by)" bash "$STATE" bmad-size "$B" Small --by "$by" <<<"x"
  expect 1 "  nor Medium (--by $by)" bash "$STATE" bmad-size "$B" Medium --by "$by" <<<"x"
done
expect 0 "Large again keeps applied" bsize Large --by planner
is "  applied stays true" "$B/status.json" bmad.applied true
# bforge <stage> <bmad json>: the recorded status.json with its bmad replaced (what a model's put could try)
bforge() {
  python3 - "$B" "$1" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1] + "/status.json"))
d["bmad"] = json.loads(sys.argv[3])
d["stage"] = sys.argv[2]
print(json.dumps(d))
PY
}
expect 1 "put cannot lower or overwrite bmad (at plan-approval the recorded Large still gates)" bput < <(bforge plan-approval '{"mode":"auto","size":"Small","applied":false}')
has "  refused by the gate (the recorded Large kept)" "BMAD mode: state.sh bmad-check"
is "  bmad unchanged after a put" "$B/status.json" bmad.size '"Large"'
expect 0 "put planning with a forged bmad is accepted but bmad is kept" bput < <(bforge planning '{"mode":"auto","size":"Small","applied":false}')
has "  note about the kept value" "bmad is set only by"
is "  size still Large" "$B/status.json" bmad.size '"Large"'
is "  applied still true" "$B/status.json" bmad.applied true
expect 1 "applied, no check: plan-approval put refused" bput < <(bstatus plan-approval)
has "  says to run bmad-check" "state.sh bmad-check $B must pass first"
expect 1 "applied, no check: approve refused" bash "$STATE" approve "$B" plan user

echo "# BMAD init and config.toml"
expect 0 "bmad-init" bash "$STATE" bmad-init "$B"
[[ -d "$B/bmad/output" && -f "$B/bmad/_bmad/config.toml" ]] && ok "  config.toml and output/ made" || bad "  bmad-init made nothing"
python3 - "$B" <<'PY' && ok "  config.toml parses back with the spaced path and the quoted repo name" || bad "  config.toml did not parse back"
import sys
try:
    import tomllib
except ImportError:  # python < 3.11: nothing to parse with
    sys.exit(0)
c = tomllib.load(open(sys.argv[1] + "/bmad/_bmad/config.toml", "rb"))["core"]
import os
assert " " in c["output_folder"] and c["output_folder"].endswith("App Support/runs/r-b/bmad/output"), c
assert c["project_name"] == 'toy "quoted" repo', c
assert c["active_initiative"] == "r-b", c
PY
cp "$B/bmad/_bmad/config.toml" "$d/config.before"; echo '# kept' >> "$B/bmad/_bmad/config.toml"
expect 0 "bmad-init is idempotent" bash "$STATE" bmad-init "$B"
grep -q '^# kept' "$B/bmad/_bmad/config.toml" && ok "  an existing config is left alone" || bad "  config was rewritten"
nonode="$d/nonode"; mkdir -p "$nonode"; ln -sf "$(command -v python3)" "$nonode/python3"; ln -sf "$(command -v git)" "$nonode/git"
rm -rf "$B/bmad/_bmad"
expect 1 "bmad-init refuses without node" env PATH="$nonode:/usr/bin:/bin" bash "$STATE" bmad-init "$B"
has "  says Node 18+" "BMAD needs Node 18+"
expect 0 "bmad-init again" bash "$STATE" bmad-init "$B"

echo "# BMAD check"
expect 3 "bmad-check fails with no documents" bash "$STATE" bmad-check "$B"
has "  names the missing spec" "no spec"
has "  names the missing architecture" "no architecture spine"
is "  check.result fail" "$B/status.json" bmad.check.result '"fail"'
mkdocs; : > "$B/bmad/output/r-b/spec-toy/spec-toy.md"
expect 3 "bmad-check fails on an empty spec" bash "$STATE" bmad-check "$B"
has "  says empty" "is empty or unreadable"
mkdocs; printf -- '---\nid: SPEC-toy\n---\n\n# Spec {Spec Title}\n' > "$B/bmad/output/r-b/spec-toy/spec-toy.md"
expect 3 "bmad-check fails on template text" bash "$STATE" bmad-check "$B"
has "  says unfilled" "unfilled template text"
mkdocs; printf -- '---\nname: toy\n---\n\n# Spine\n\n### AD-1 - x\n\n- **Binds:** all\n' > "$B/bmad/output/r-b/architecture-toy/architecture-toy.md"
expect 3 "bmad-check fails on a bad spine (lint_spine)" bash "$STATE" bmad-check "$B"
has "  says lint_spine" "fails lint_spine"
mkdocs; printf -- '# no frontmatter\n\ntext\n' > "$B/bmad/output/r-b/spec-toy/spec-toy.md"
expect 3 "bmad-check fails without id: SPEC-" bash "$STATE" bmad-check "$B"
mkdocs
expect 3 "docs names prd and ux: check fails without them" bash "$STATE" bmad-check "$B"
has "  names prd" "docs names prd"
mkdocs prd ux
rm "$B/bmad/output/r-b/prd-toy/validation-report.html"
expect 3 "PRD without validation-report.html fails" bash "$STATE" bmad-check "$B"
has "  names the report" "validation-report.html is missing or empty"
printf '<html>ok</html>\n' > "$B/bmad/output/r-b/prd-toy/validation-report.html"
touch -t 202501010000 "$B/bmad/output/r-b/prd-toy/validation-report.html"
expect 3 "a report older than the PRD fails" bash "$STATE" bmad-check "$B"
has "  says run Validate again" "run Validate again"
printf -- '---\nstatus: draft\n---\n\n# PRD\n\nx\n' > "$B/bmad/output/r-b/prd-toy/prd-toy.md"; touch -t 202601010000 "$B/bmad/output/r-b/prd-toy/prd-toy.md"
printf '<html>ok</html>\n' > "$B/bmad/output/r-b/prd-toy/validation-report.html"
expect 3 "a PRD that is not final fails" bash "$STATE" bmad-check "$B"
has "  says not final" "status is not final"
mkdocs prd ux
expect 0 "bmad-check passes on fixture documents (PRD + UX with reports)" bash "$STATE" bmad-check "$B"
has "  prints pass" "bmad-check=pass"
is "  check.result" "$B/status.json" bmad.check.result '"pass"'
python3 - "$B" <<'PY' && ok "  check records documents and a digest" || bad "  check record is wrong"
import json, sys
c = json.load(open(sys.argv[1] + "/status.json"))["bmad"]["check"]
assert len(c["digest"]) == 64 and c["reasons"] == [] and c["at"].endswith("Z")
assert "bmad/output/r-b/spec-toy/spec-toy.md" in c["documents"] and "bmad/output/r-b/ux-toy/EXPERIENCE.md" in c["documents"], c["documents"]
PY

echo "# BMAD: plan-approval is allowed with a passing check, refused after a document edit, and recovers"
expect 0 "plan-approval put with a passing check" bput < <(bstatus plan-approval)
echo "A new line." >> "$B/bmad/output/r-b/spec-toy/spec-toy.md"
expect 1 "put plan-approval refused after a document edit (stale digest)" bput < <(bstatus plan-approval)
has "  says documents changed" "documents changed since the last bmad-check"
expect 1 "approve refused after a document edit" bash "$STATE" approve "$B" plan user
expect 0 "bmad-check again" bash "$STATE" bmad-check "$B"
expect 0 "recovered: put plan-approval" bput < <(bstatus plan-approval)
expect 0 "recovered: approve plan" bash "$STATE" approve "$B" plan user
is "  approved" "$B/status.json" plan.approval.by '"user"'
is "  bmad still recorded" "$B/status.json" bmad.check.result '"pass"'
mkdir -p "$B/bmad/output/r-b/prd-late"; printf -- '---\nstatus: final\n---\n\n# late\n\nx\n' > "$B/bmad/output/r-b/prd-late/prd-late.md"
rm -f "$B/status.json"; expect 0 "back to planning" bput < <(bstatus planning)
expect 1 "a document added after the check also makes it stale" bput < <(bstatus plan-approval)

echo "# BMAD on: always applied, even with no size"
rm -rf "$B/bmad" "$B/status.json" "$B/log.md"; bmeta on
expect 0 "on: put planning" bput < <(bstatus planning)
expect 1 "on, no size: plan-approval refused until a check passes" bput < <(bstatus plan-approval)
has "  says bmad-check" "state.sh bmad-check $B must pass first"
expect 0 "on: bmad-init allowed with no size" bash "$STATE" bmad-init "$B"
expect 0 "on: Small is recorded and applied" bsize Small --by planner
is "  applied (mode on)" "$B/status.json" bmad.applied true
mkdocs
expect 0 "on: check passes" bash "$STATE" bmad-check "$B"
expect 0 "on: plan-approval allowed after the check" bput < <(bstatus plan-approval)
# an app mode change cannot be undone by status.json: on stays on
rm -f "$B/status.json"; bash "$STATE" put "$B/status.json" < <(bstatus planning) >/dev/null
bash "$STATE" put "$B/status.json" < <(bforge planning '{"mode":"on","size":"Small","applied":false}') >/dev/null
expect 1 "on: applied=false in status.json does not skip the gate" bput < <(bstatus plan-approval)
rm -rf "$B/bmad" "$B/status.json"; bmeta off
expect 0 "off again: plan-approval puts as before" bput < <(bstatus plan-approval)


echo "# cleanup (0.5.0): narrow, for this run only (gh shim, local bare remote)"
CW="$d/cw"; CR="$CW/cr"; CRUN="$d/runs/r-c"; CO="$d/cl-origin.git"
mkdir -p "$CR" "$CRUN" "$d/cgh"
git -C "$CR" init -q -b main
printf '.env\n*.local\n' > "$CR/.gitignore"; echo "K=1" > "$CR/.env"
git -C "$CR" add .gitignore && git -C "$CR" -c user.name=t -c user.email=t@t commit -q -m init
git init -q --bare -b main "$CO"; git -C "$CR" remote add origin "$CO"; git -C "$CR" push -q origin main
CBASE="$(git -C "$CR" rev-parse HEAD)"
printf 'folder: %s\n' "$CW" > "$CRUN/request.md"
echo '{"schema": 1, "reviewMode": "github"}' > "$CRUN/meta.json"
printf '{"schema": 1, "repos": [{"name": "cr", "path": "%s", "check": null, "copyFiles": [".env"]}]}\n' "$CR" > "$CRUN/repos.json"
(
  export GH_SHIM_DIR="$d/cgh" PATH="$ROOT/scripts/skill-e2e/gh-shim:$PATH"
  for n in 1 2 3 4; do
    bash "$STATE" worktree "$CRUN" "$CR" "PR$n" "$CBASE" >/dev/null
    w="$CW/cr-worktrees/dcv2-r-c-pr$n"
    echo "$n" > "$w/f$n.txt"; git -C "$w" add "f$n.txt"; git -C "$w" -c user.name=t -c user.email=t@t commit -q -m "S$n"
    git -C "$w" push -q "$CO" "dcv2/r-c-pr$n"
  done
)
tip() { git -C "$CW/cr-worktrees/dcv2-r-c-pr$1" rev-parse HEAD; }
T1="$(tip 1)"; T2="$(tip 2)"; T3="$(tip 3)"
[[ -f "$CW/cr-worktrees/dcv2-r-c-pr1/.env" ]] && ok "  PR1 got the copied .env" || bad "  no copied .env"
git -C "$CR" -c user.name=t -c user.email=t@t merge -q --no-edit "dcv2/r-c-pr1"            # PR1's merge pulled into main here
echo wip > "$CW/cr-worktrees/dcv2-r-c-pr3/f3.txt"                                             # PR3: a dirty worktree
echo mine > "$CW/cr-worktrees/dcv2-r-c-pr2/notes.local"                                       # PR2: an ignored file of the user's
python3 - "$d/cgh/prs.json" "$T1" "$T2" "$T3" <<'PY'
import json, sys
f, t1, t2, t3 = sys.argv[1:5]
pr = lambda n, state, oid: {"url": f"https://github.com/shim/repo/pull/{n}", "head": f"dcv2/r-c-pr{n}", "isDraft": False, "state": state, "headRefOid": oid}
json.dump([pr(1, "MERGED", t1), pr(2, "OPEN", t2), pr(3, "MERGED", t3), pr(4, "MERGED", "0" * 40)], open(f, "w"))
PY
cstatus() { python3 - "$CRUN/status.json" "$1" "$CR" "$CW" <<'PY'
import json, sys
f, stage, repo, ws = sys.argv[1:5]
prs = [{"id": f"PR{n}", "repoPath": repo, "branch": f"dcv2/r-c-pr{n}", "worktree": f"{ws}/cr-worktrees/dcv2-r-c-pr{n}",
        "prUrl": f"https://github.com/shim/repo/pull/{n}", "state": "opened"} for n in (1, 2, 3, 4)]
json.dump({"schema": 1, "runId": "r-c", "stage": stage, "prs": prs}, open(f, "w"))
PY
}
cstatus shipping
export GH_SHIM_DIR="$d/cgh"
expect 1 "cleanup refused before done" env PATH="$ROOT/scripts/skill-e2e/gh-shim:$PATH" bash "$STATE" cleanup "$CRUN"
[[ -d "$CW/cr-worktrees/dcv2-r-c-pr1" ]] && ok "  nothing removed" || bad "  PR1 worktree gone"
cstatus done
expect 0 "cleanup at done" env PATH="$ROOT/scripts/skill-e2e/gh-shim:$PATH" bash "$STATE" cleanup "$CRUN"
has "  PR1 worktree removed (the copied .env goes with it)" "PR1: worktree removed"
has "  PR1 branch deleted (merged here)" "branch dcv2/r-c-pr1 deleted"
has "  PR1 remote branch deleted" "PR1: worktree removed; branch dcv2/r-c-pr1 deleted; remote branch deleted"
has "  PR2 (open) worktree kept: the user's ignored file" "PR2: worktree kept: has ignored files (notes.local)"
has "  PR2 remote kept: not merged" "remote branch kept (not merged (open))"
has "  PR3 dirty worktree kept" "PR3: worktree kept: 1 changed files"
has "  PR3 branch kept with its worktree" "branch dcv2/r-c-pr3 kept (its worktree is kept)"
has "  PR4 tip mismatch: remote kept" "remote branch kept (its tip is not the merged PR's head)"
[[ ! -d "$CW/cr-worktrees/dcv2-r-c-pr1" && -d "$CW/cr-worktrees/dcv2-r-c-pr2" && -d "$CW/cr-worktrees/dcv2-r-c-pr3" ]] && ok "  only PR1's and PR4's folders go" || bad "  folders: $(ls "$CW/cr-worktrees")"
[[ ! -d "$CW/cr-worktrees/dcv2-r-c-pr4" ]] && ok "  PR4's clean worktree removed" || bad "  PR4 kept"
heads="$(git -C "$CO" for-each-ref --format='%(refname:short)' refs/heads)"
grep -qx "dcv2/r-c-pr1" <<<"$heads" && bad "  PR1 still on the remote" || ok "  PR1 gone from the remote"
grep -qx "dcv2/r-c-pr3" <<<"$heads" && bad "  PR3 (merged, tip matches) still on the remote" || ok "  PR3 gone from the remote (merged)"
grep -qx "dcv2/r-c-pr2" <<<"$heads" && grep -qx "dcv2/r-c-pr4" <<<"$heads" && grep -qx "main" <<<"$heads" && ok "  PR2, PR4 and main stay on the remote" || bad "  remote heads: $heads"
git -C "$CR" show-ref --verify --quiet refs/heads/dcv2/r-c-pr2 && ok "  PR2 branch kept (git branch -d refused)" || bad "  PR2 branch deleted"
grep -q "pr view https://github.com/shim/repo/pull/1 --json state,mergeCommit,headRefName,headRefOid" "$d/cgh/calls.log" && ok "  asked gh -R about the merge" || bad "  gh calls: $(cat "$d/cgh/calls.log")"
tail -1 "$CRUN/log.md" | grep -q ' done cleanup: PR1: worktree removed' && ok "  logged once" || bad "  log: $(tail -1 "$CRUN/log.md")"
expect 0 "cleanup again: nothing left to do" env PATH="$ROOT/scripts/skill-e2e/gh-shim:$PATH" bash "$STATE" cleanup "$CRUN"
has "  PR1 already gone" "PR1: worktree already gone; remote branch already deleted"
python3 - "$CRUN/.guard.json" "$d/elsewhere" <<'PY'
import json, sys
g = json.load(open(sys.argv[1])); g["worktrees"]["PR2"] = sys.argv[2]; json.dump(g, open(sys.argv[1], "w"))
PY
mkdir -p "$d/elsewhere"
expect 0 "a worktree recorded somewhere else is never removed" env PATH="$ROOT/scripts/skill-e2e/gh-shim:$PATH" bash "$STATE" cleanup "$CRUN"
has "  says why" "PR2: worktree kept: not where this run's worktrees go"
[[ -d "$d/elsewhere" ]] && ok "  the folder stays" || bad "  removed"
unset GH_SHIM_DIR

echo "# requests to the app (0.5.0)"
expect 0 "request tickets" bash "$STATE" request "$CRUN" tickets
rid="${out#request=}"
[[ "$rid" =~ ^[0-9]{8}T[0-9]{6}Z-tickets$ ]] && ok "  id $rid" || bad "  id: $out"
is "  kind" "$CRUN/requests/$rid.json" kind '"tickets"'
is "  schema" "$CRUN/requests/$rid.json" schema 1
expect 0 "a second one in the same second gets its own id" bash "$STATE" request "$CRUN" tickets
[[ "${out#request=}" != "$rid" ]] && ok "  ${out#request=}" || bad "  same id"
expect 1 "unknown kind refused" bash "$STATE" request "$CRUN" "merge"
expect 1 "free text refused" bash "$STATE" request "$CRUN" "recheck; rm -rf /"
expect 1 "not a run folder" bash "$STATE" request "$d" recheck
expect 3 "request-wait: the app has not answered" bash "$STATE" request-wait "$CRUN" "$rid" --timeout 1
has "  pending" "result=pending"
echo '{"schema": 1, "id": "x", "kind": "tickets", "state": "running", "ok": null, "message": "checking"}' > "$CRUN/requests/$rid.result.json"
expect 3 "request-wait: still running" bash "$STATE" request-wait "$CRUN" "$rid" --timeout 1
has "  still running" "result=pending message=checking"
echo '{"schema": 1, "id": "x", "kind": "tickets", "state": "done", "ok": true, "message": "no ticket changed in ClickUp"}' > "$CRUN/requests/$rid.result.json"
expect 0 "request-wait: done" bash "$STATE" request-wait "$CRUN" "$rid"
has "  the result line" "result=done ok=yes message=no ticket changed in ClickUp"
expect 1 "request-wait: not an id" bash "$STATE" request-wait "$CRUN" "../../etc/passwd"
expect 1 "request-wait: no such request" bash "$STATE" request-wait "$CRUN" "20261027T000000Z-recheck"

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
