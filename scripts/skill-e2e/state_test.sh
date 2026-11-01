#!/usr/bin/env bash
# Unit test of plugin/scripts/state.sh (no Claude call, no network: gh is a stub).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STATE="$ROOT/plugin/scripts/state.sh"
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
trap 'rm -rf "$d"' EXIT
unset DCV2_TEST_MODE DCV2_RUN_DIR DCV2_MEMORY_DIR || true

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
       "plan": {"path": "plan.md", "reviewRound": 1, "verdict": "APPROVED", "openFindings": 0, "approved": True,
                "risk": {"round": 1, "verdict": "NO BLOCKING RISKS", "openFindings": 0, "path": "reviews/risk-round1.md"}},
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
badpy="$d/badpy"; mkdir -p "$badpy"; printf '#!/bin/sh\nexit 1\n' > "$badpy/python3"; cp "$badpy/python3" "$badpy/python"; cp "$badpy/python3" "$badpy/py"; chmod +x "$badpy"/py*
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
echo "# plan.risk (Risk Reviewer)"
expect 0 "plan.risk null" put < <(status planning "" "" 'doc["plan"]["risk"]=None')
expect 0 "plan.risk missing (a run from before the Risk Reviewer)" put < <(status planning "" "" 'del doc["plan"]["risk"]')
expect 0 "plan.risk RISKS FOUND" put < <(status planning "" "" 'doc["plan"]["risk"]={"round":2,"verdict":"RISKS FOUND","openFindings":2,"path":"reviews/risk-round2.md"}')
expect 1 "plan.risk unknown verdict refused" put < <(status planning "" "" 'doc["plan"]["risk"]={"round":1,"verdict":"OK","openFindings":0,"path":None}')
has "  names the verdicts" "plan.risk.verdict"
expect 1 "plan.risk NO BLOCKING RISKS with open findings refused" put < <(status planning "" "" 'doc["plan"]["risk"]={"round":1,"verdict":"NO BLOCKING RISKS","openFindings":1,"path":None}')
expect 1 "plan.risk round 0 refused" put < <(status planning "" "" 'doc["plan"]["risk"]={"round":0,"verdict":"RISKS FOUND","openFindings":1,"path":None}')
expect 1 "plan.risk path outside reviews/ refused" put < <(status planning "" "" 'doc["plan"]["risk"]={"round":1,"verdict":"RISKS FOUND","openFindings":1,"path":"../status.json"}')
expect 1 "plan.risk not an object refused" put < <(status planning "" "" 'doc["plan"]["risk"]="none"')
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
expect 1 "gate PASS: APPROVED but no criteria table" bash "$STATE" gate "$RUN" PR1 PASS "clean"
has "  names the missing table" "Criteria → tests"
printf '## Verdict\nAPPROVED\n\n## Criteria -> tests\n| Story | Criterion | Proving test |\n|---|---|---|\n| S1 | a covered one | a_test.sh::covers |\n| S1 | the rate limit is applied | uncovered |\n' > "$RUN/reviews/PR1-round1.md"
expect 1 "gate PASS: one uncovered row" bash "$STATE" gate "$RUN" PR1 PASS "clean"
has "  names the uncovered criterion" "the rate limit is applied"
hasnt "  does not name the covered one" "a covered one"
printf '## Verdict\nAPPROVED\n\n## Criteria → tests\n| Story | Criterion | Proving test |\n|---|---|---|\n| S1 | bare manual | manual: |\n' > "$RUN/reviews/PR1-round1.md"
expect 1 "gate PASS: manual with no reason" bash "$STATE" gate "$RUN" PR1 PASS "clean"
printf '## Verdict\nAPPROVED\n\n## Criteria → tests\n| Story | Criterion | Proving test |\n|---|---|---|\n' > "$RUN/reviews/PR1-round1.md"
expect 1 "gate PASS: empty table" bash "$STATE" gate "$RUN" PR1 PASS "clean"
printf '## Verdict\nAPPROVED\n\n## Criteria → tests\n| Story | Criterion | Proving test |\n|---|---|---|\n| S1 | multiply adds | x_test.sh::multiply |\n| S1 | docs only | manual: trivial wording change |\n' > "$RUN/reviews/PR1-round1.md"
expect 0 "gate PASS over an APPROVED review" bash "$STATE" gate "$RUN" PR1 PASS "no findings; check passes"
expect 1 "WAIVED over PASS refused" bash "$STATE" gate "$RUN" PR1 WAIVED "x"
printf '## Verdict\nAPPROVED\n\n## Criteria → tests\n| Story | Criterion | Proving test |\n|---|---|---|\n| S1 | multiply adds | scripts/x_test.sh::multiply adds |\n' > "$RUN/reviews/PR2-round1.md"
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
bash "$STATE" log "$RUN" shipping <<<'PR9 draft PR opened: https://github.com/o/r/pull/9' >/dev/null
(( $(wc -l < "$RUN/log.md") == n0 + 4 )) && ok "  another stage is not a repeat" || bad "  log: $(tail -3 "$RUN/log.md")"
bash "$STATE" log "$RUN" <<<'PR9 implementer retried: no JSON block' >/dev/null
bash "$STATE" log "$RUN" <<<'PR9 implementer retried: no JSON block' >/dev/null
(( $(wc -l < "$RUN/log.md") == n0 + 6 )) && ok "  retry lines always count" || bad "  log: $(tail -3 "$RUN/log.md")"
echo "2020-01-01T00:00:00Z building PR9 old event" >> "$RUN/log.md"
bash "$STATE" log "$RUN" building <<<'PR9 old event' >/dev/null
(( $(wc -l < "$RUN/log.md") == n0 + 8 )) && ok "  an old identical line is not a repeat" || bad "  log: $(tail -3 "$RUN/log.md")"

echo "# plan sections (16)"
expect 0 "plan prs" bash "$STATE" plan "$RUN" prs
has "  has PR2" "### PR2: Prefix"; hasnt "  stops before Manual steps" "Manual steps"
expect 0 "plan tickets" bash "$STATE" plan "$RUN" tickets
has "  has t1" "- t1 | T-1"; hasnt "  stops before Risks" "secret risk text"
expect 0 "plan stories" bash "$STATE" plan "$RUN" stories
has "  story headings" "### S2: Prefix results"; hasnt "  headings only" "Tickets: t1"

expect 0 "plan why, none yet" bash "$STATE" plan "$RUN" why PR1
[[ -z "$out" ]] && ok "  no Why line gives an empty result" || bad "  why: $out"
expect 0 "plan decisions, none yet" bash "$STATE" plan "$RUN" decisions
has "  no Decisions section" "no ## Decisions section"
cp "$RUN/plan.md" "$d/plan.bak"
python3 - "$RUN/plan.md" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
s = s.replace("### PR1: Fix multiply\n- Repo: r\n", "### PR1: Fix multiply\n- Repo: r\n- Why:   multiply returns the wrong value without it  \n")
s += "\n## Decisions\n- 2026-10-30 | use x | because y | rules out: z\n"
open(p, "w").write(s)
PY
expect 0 "plan why PR1" bash "$STATE" plan "$RUN" why PR1
[[ "$out" == "multiply returns the wrong value without it" ]] && ok "  Why is trimmed" || bad "  why: $out"
expect 0 "plan why PR2 (no line)" bash "$STATE" plan "$RUN" why PR2
[[ -z "$out" ]] && ok "  PR2 has no Why" || bad "  why PR2: $out"
expect 0 "plan prs with a Why" bash "$STATE" plan "$RUN" prs
has "  Why line is printed" "- Why:   multiply returns the wrong value"
expect 0 "plan decisions" bash "$STATE" plan "$RUN" decisions
has "  decision bullet" "- 2026-10-30 | use x | because y | rules out: z"
cp "$d/plan.bak" "$RUN/plan.md"

echo "# scope (PR body Scope)"
SC="$d/scope-repo"; SRUN="$d/runs/r-scope"; mkdir -p "$SC/src" "$SC/web" "$SRUN"
git -C "$SC" init -q -b main
printf '{"dependencies":{"keep":"1.0.0","gone":"2.0.0"}}\n' > "$SC/package.json"
printf '[package]\nname = "x"\n\n[dependencies]\nserde = "1.0.100"\ntokio = { version = "1.0", features = ["rt"] }\n' > "$SC/Cargo.toml"
printf 'flask==2.0\nrequests>=2\n' > "$SC/requirements.txt"
printf 'module x\n\nrequire (\n\tgithub.com/a/b v1.0.0\n)\n' > "$SC/go.mod"
printf 'lock1\n' > "$SC/package-lock.json"; printf 'src\n' > "$SC/src/a.rs"
git -C "$SC" add -A; git -C "$SC" -c user.name=t -c user.email=t@t commit -q -m base
SBASE="$(git -C "$SC" rev-parse HEAD)"
printf '{"dependencies":{"keep":"1.0.0","left-pad":"1.3.0"}}\n' > "$SC/package.json"
printf '[package]\nname = "x"\n\n[dependencies]\nserde = "1.0.200"\ntokio = { version = "1.0", features = ["rt"] }\n' > "$SC/Cargo.toml"
printf 'flask==2.1\n' > "$SC/requirements.txt"
printf 'module x\n\nrequire (\n\tgithub.com/a/b v1.2.0\n\tgithub.com/c/d v0.1.0\n)\n' > "$SC/go.mod"
printf 'lock2\n' > "$SC/package-lock.json"; printf 'g\n' > "$SC/Gemfile"; printf 'r\n' > "$SC/NOTES.md"
printf 'src2\n' > "$SC/src/b.rs"; printf 'src3\n' > "$SC/src/c.rs"; printf 'w\n' > "$SC/web/i.ts"
git -C "$SC" add -A; git -C "$SC" -c user.name=t -c user.email=t@t commit -q -m change
python3 - "$SRUN" "$SC" "$SBASE" <<'PY'
import json, sys
run, wt, base = sys.argv[1:4]
json.dump({"schema": 1, "runDir": run, "repos": [wt], "worktrees": {"PR1": wt}, "bases": {"PR1": base}}, open(run + "/.guard.json", "w"))
PY
expect 0 "scope" bash "$STATE" scope "$SRUN" PR1
has "  areas with counts" "Areas: (root) (7 files), src (2 files), web (1 file)"
has "  npm add" "package.json: added left-pad 1.3.0"
has "  npm remove" "package.json: removed gone"
has "  cargo bump" "Cargo.toml: bumped serde 1.0.100 → 1.0.200"
hasnt "  unchanged dep not listed" "tokio"
has "  requirements bump" "requirements.txt: bumped flask ==2.0 → ==2.1"
has "  requirements removal" "requirements.txt: removed requests"
has "  go bump" "go.mod: bumped github.com/a/b v1.0.0 → v1.2.0"
has "  go add" "go.mod: added github.com/c/d v0.1.0"
has "  lock file is changed" "package-lock.json: changed"
has "  unknown manifest is changed" "Gemfile: changed"
expect 0 "scope, unknown PR" bash "$STATE" scope "$SRUN" PR9
[[ -z "$out" ]] && ok "  no worktree, no lines" || bad "  scope PR9: $out"

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
# A repo in a mixed-case folder: the printed, stored and compared worktree path keeps its case (ident() only decides).
MIX="$(dirname "$R")/RepoX/MyApp"; mkdir -p "$MIX"; git -C "$MIX" init -q -b main
git -C "$MIX" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init; MIXBASE="$(git -C "$MIX" rev-parse HEAD)"
python3 - "$RUN/status.json" "$MIX" <<'PY'
import json, sys
st = json.load(open(sys.argv[1])); pr = dict(st["prs"][0]); pr.update(id="PR7", repoPath=sys.argv[2], worktree=None, branch=None, commits=0)
st["prs"].append(pr); json.dump(st, open(sys.argv[1], "w"))
PY
expect 0 "worktree of a mixed-case repo" bash "$STATE" worktree "$RUN" "$MIX" PR7 "$MIXBASE"
has "  printed with its original case" "worktree=$(dirname "$R")/RepoX/MyApp-worktrees/dcv2-r-t-pr7"
is "  .guard.json stores it with that case" "$RUN/.guard.json" worktrees.PR7 "\"$(dirname "$R")/RepoX/MyApp-worktrees/dcv2-r-t-pr7\""
expect 0 "  and a second call recognises it" bash "$STATE" worktree "$RUN" "$MIX" PR7 "$MIXBASE"
is "  the run's status keeps the repo path's case" "$RUN/status.json" prs.2.repoPath "\"$MIX\""
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

echo "# worktree only for the run's repos (CAP-17)"
OUT="$d/outside/orepo"; OUT2="$d/outside/orepo2"; mkdir -p "$OUT" "$OUT2"
for repo in "$OUT" "$OUT2"; do git -C "$repo" init -q -b main; git -C "$repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init; done
OBASE="$(git -C "$OUT" rev-parse HEAD)"; OBASE2="$(git -C "$OUT2" rev-parse HEAD)"
cp "$RUN/.guard.json" "$d/guard.before"
expect 1 "a repo outside repos.json and the start folder is refused" bash "$STATE" worktree "$RUN" "$OUT" PR1 "$OBASE"
has "  says why" "not one of this run's repos"
cmp -s "$RUN/.guard.json" "$d/guard.before" && ok "  .guard.json is byte-identical" || bad "  .guard.json changed"
[[ ! -d "$d/outside/orepo-worktrees" ]] && ok "  no worktree made" || bad "  a worktree was made"
git -C "$OUT" branch --list 'dcv2/*' | grep -q . && bad "  a branch was made" || ok "  no branch made"
ln -s "$OUT2" "$d/outside/lnk"
printf '{"schema": 1, "repos": [{"name": "o", "path": "%s", "check": null}]}\n' "$d/outside/lnk" > "$RUN/repos.json"
expect 1 "a repo not in repos.json is still refused when repos.json lists another" bash "$STATE" worktree "$RUN" "$OUT" PR1 "$OBASE"
expect 0 "a repo listed in repos.json by a symlinked path is accepted" bash "$STATE" worktree "$RUN" "$OUT2" PR1 "$OBASE2"
rm -rf "$RUN/repos.json" "$d/outside/orepo2-worktrees"; git -C "$OUT2" worktree prune; git -C "$OUT2" branch -q -D dcv2/r-t-pr1
python3 - "$RUN/.guard.json" <<'PY'
import json, sys
g = json.load(open(sys.argv[1])); g["repos"] = [r for r in g["repos"] if not r.endswith("orepo2")]; g["worktrees"].pop("PR1", None); json.dump(g, open(sys.argv[1], "w"))
PY
expect 0 "a repo inside the start folder is accepted (PR1 again)" bash "$STATE" worktree "$RUN" "$R" PR1 "$BASE"

echo "# ship (3, 4, 8, 10)"
export DCV2_TEST_LOCAL_REMOTES=1   # the remote is a local bare repo (state.sh pushes to local folders only in the suites)
gh="$d/gh"; mkdir -p "$gh"
cat > "$gh/gh" <<EOF
#!/bin/sh
echo "\$*" >> "$d/gh.log"
case "\$1 \$2" in
  "repo view") echo o/rr ;;
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
has "  no remote for r" "PR1 r: no git remote"; has "  base on the recorded remote for rr" "PR2 rr: remote $d/origin.git, base main is on the remote"
expect 1 "approve ship test without the env" bash "$STATE" approve "$RUN" ship test
expect 0 "approve ship user" bash "$STATE" approve "$RUN" ship user
is "  stage shipping" "$RUN/status.json" stage '"shipping"'
is "  shipApproval.by user" "$RUN/status.json" shipApproval.by '"user"'
expect 0 "ship-pr PR1 (no remote)" bash "$STATE" ship-pr "$RUN" PR1
is "  PR1 local-only" "$RUN/status.json" prs.0.state '"local-only"'
# Local-mode ship-pr has the github-mode protections (CAP-14). Each case undoes itself, so PR2 ships normally after them.
pr2() { python3 - "$RUN" "$@" <<'PY'    # pr2 <python on st (status.json) and g (.guard.json)>
import json, sys
run, code = sys.argv[1:3]
st, g = json.load(open(run + "/status.json")), json.load(open(run + "/.guard.json"))
exec(code)
json.dump(st, open(run + "/status.json", "w")); json.dump(g, open(run + "/.guard.json", "w"))
PY
}
pr2reset() { pr2 'st["prs"][1].update(state="reviewed", note=None)'; }
: > "$d/gh.log"
expect 0 "a recorded remote the run may not push (a local folder, outside the suites) is needs-attention" env -u DCV2_TEST_LOCAL_REMOTES bash "$STATE" ship-pr "$RUN" PR2
is "  PR2 needs-attention" "$RUN/status.json" prs.1.state '"needs-attention"'
has "  says why" "is not an owner/name https or ssh URL: nothing pushed"
has "  names the URL" "remote $d/origin.git"
git -C "$d/origin.git" show-ref --verify -q refs/heads/dcv2/r-t-pr2 && bad "  the branch was pushed" || ok "  nothing pushed"
[[ ! -s "$d/gh.log" ]] && ok "  no gh call" || bad "  gh calls: $(cat "$d/gh.log")"
expect 0 "ship-check says the same before approving" env -u DCV2_TEST_LOCAL_REMOTES bash "$STATE" ship-check "$RUN"
has "  cannot be pushed" "PR2 rr: remote $d/origin.git cannot be pushed by the run"
has "  needs attention" ": needs attention"
pr2reset
pr2 'st["prs"][1]["worktree"] = "/tmp/x"'
expect 1 "ship-pr: a status.json worktree that is not canonical is refused" bash "$STATE" ship-pr "$RUN" PR2
has "  says where it must be" "must be on dcv2/r-t-pr2 in its worktree"
pr2 'st["prs"][1]["worktree"] = g["worktrees"]["PR2"]'
pr2 'g["worktrees"].pop("PR2")'
expect 1 "ship-pr: a worktree not recorded in .guard.json is refused" bash "$STATE" ship-pr "$RUN" PR2
has "  says so" "not recorded in .guard.json"
pr2 'g["worktrees"]["PR2"] = st["prs"][1]["worktree"]'
git -C "$d/origin.git" show-ref --verify -q refs/heads/dcv2/r-t-pr2 && bad "  pushed anyway" || ok "  nothing pushed by the refused cases"
pr2 'g.setdefault("copied", {})["PR2"] = [".env"]'
echo "K=1" > "$WT2/.env"; git -C "$WT2" add -f .env; git -C "$WT2" -c user.name=t -c user.email=t@t commit -q -m "oops: .env"
expect 3 "ship-pr: a committed copied .env is needs-attention" bash "$STATE" ship-pr "$RUN" PR2
has "  names the file" "the branch commits .env"
is "  PR2 needs-attention" "$RUN/status.json" prs.1.state '"needs-attention"'
git -C "$d/origin.git" show-ref --verify -q refs/heads/dcv2/r-t-pr2 && bad "  the .env was pushed" || ok "  nothing pushed"
git -C "$WT2" reset -q --hard HEAD~1; rm -f "$WT2/.env"; pr2 'g["copied"].pop("PR2")'; pr2reset
mkdir -p "$(git -C "$RR" rev-parse --path-format=absolute --git-common-dir)/hooks"
printf '#!/bin/sh\ntouch "%s/pre-push-ran"\nexit 0\n' "$d" > "$(git -C "$RR" rev-parse --path-format=absolute --git-common-dir)/hooks/pre-push"
chmod +x "$(git -C "$RR" rev-parse --path-format=absolute --git-common-dir)/hooks/pre-push"
: > "$d/gh.log"
expect 0 "ship-pr PR2 (remote)" bash "$STATE" ship-pr "$RUN" PR2
[[ ! -e "$d/pre-push-ran" ]] && ok "  the worktree's pre-push hook did not run" || bad "  the pre-push hook ran"
grep -q -- "pr view dcv2/r-t-pr2 --json url -q .url -R o/rr" "$d/gh.log" && ok "  gh pr view names -R o/rr" || bad "  gh calls: $(cat "$d/gh.log")"
grep -q -- "pr create .* -R o/rr" "$d/gh.log" && ok "  gh pr create names -R o/rr" || bad "  gh calls: $(cat "$d/gh.log")"
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
echo "# an ssh host alias remote is recognised as owner/name (CAP-14)"
AL="$WS/al"; mkdir -p "$AL"; git -C "$AL" init -q -b main; git -C "$AL" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
git -C "$AL" remote add origin github-work:o/rr
expect 0 "worktree for a repo whose remote is an ssh host alias" bash "$STATE" worktree "$RUN" "$AL" PR3 "$(git -C "$AL" rev-parse HEAD)"
is "  recorded as owner/name" "$RUN/.guard.json" remotes.PR3.repo '"o/rr"'
is "  with its URL" "$RUN/.guard.json" remotes.PR3.url '"github-work:o/rr"'

echo "# PR body sections (one builder; local mode neutralises, caps and scans)"
cp "$RUN/plan.md" "$d/plan.body.bak"; [[ -e "$RUN/tickets.json" ]] && cp "$RUN/tickets.json" "$d/tickets.body.bak" || true
python3 - "$RUN/plan.md" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
s = s.replace("### PR2: Prefix\n- Repo: rr\n", "### PR2: Prefix\n- Repo: rr\n- Why: results have no prefix without it\n")
open(p, "w").write(s)
PY
mkdir -p "$RUN/reviews"
printf 'Eagle-Eye review — round 1: REQUEST CHANGES — 2 MEDIUM+, 0 LOW\n\n## Verdict\nREQUEST CHANGES\n\n## Findings\n1. MEDIUM — `a.py:3` — missing guard, ping @alice about o/r#5 — Fix: add it\n2. HIGH — `b.py:1` — leaks token=ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789 — Fix: remove\n3. LOW — `c.py:1` — nit\n' > "$RUN/reviews/PR2-round1.md"
printf 'Eagle-Eye review — round 2: APPROVED — 0 MEDIUM+, 0 LOW\n\n## Verdict\nAPPROVED\n\n## Findings\nNone.\n' > "$RUN/reviews/PR2-round2.md"
printf '{"tasks":[{"id":"t2","custom_id":"T-2","name":"Prefix for @bob in o/r#9","url":"https://x/t2","description":"SECRET BODY TEXT"}]}\n' > "$RUN/tickets.json"
python3 - "$RUN/status.json" <<'PY'
import json, sys
st = json.load(open(sys.argv[1])); pr = st["prs"][1]; pr["state"] = "pushed"; pr["prUrl"] = None; pr["review"]["round"] = 2
json.dump(st, open(sys.argv[1], "w"))
PY
rm -f "$d/gh-has-pr"; : > "$d/gh.log"
expect 0 "ship-pr builds the sectioned body" bash "$STATE" ship-pr "$RUN" PR2
B2="$RUN/pr-bodies/PR2.md"
python3 - "$B2" <<'PY' && ok "  sections in order: Why, What it adds, Review, Verification, Tickets" || bad "  body: $(cat "$B2")"
import re, sys
t = open(sys.argv[1]).read()
h = re.findall(r"^## (.+)$", t, re.M)
want = ["Why", "What it adds", "Review", "Verification", "Tickets"]
sys.exit(0 if [x for x in h if x in want] == want and h.index("Why") < h.index("What it adds") else 1)
PY
grep -q "results have no prefix without it" "$B2" && ok "  Why from the plan" || bad "  no Why"
grep -q "Round 1 MEDIUM: a.py:3 — missing guard" "$B2" && ok "  a fixed MEDIUM finding is listed" || bad "  body: $(cat "$B2")"
grep -q "ghp_" "$B2" && bad "  a secret-like finding reached the body" || ok "  the secret-like finding line was dropped"
grep -q "nit" "$B2" && bad "  a LOW finding is listed" || ok "  LOW findings are not listed"
grep -q "@alice" "$B2" && bad "  @mention not neutralised" || ok "  finding @mention neutralised"
grep -q "@bob" "$B2" && bad "  ticket @mention not neutralised" || ok "  ticket name neutralised"
grep -q "SECRET BODY TEXT" "$B2" && bad "  a ticket body leaked" || ok "  no ticket bodies"
grep -q "Prefix for" "$B2" && ok "  ticket name listed" || bad "  no ticket line"
grep -qE "\bNone\b|\bnull\b" "$B2" && bad "  None/null in the body: $(cat "$B2")" || ok "  no None or null"
grep -q "^## Scope" "$B2" && bad "  empty Scope section present" || ok "  a section with no data is left out"
cp "$d/plan.body.bak" "$RUN/plan.md"; [[ -e "$d/tickets.body.bak" ]] && cp "$d/tickets.body.bak" "$RUN/tickets.json" || rm -f "$RUN/tickets.json"
rm -rf "$RUN/reviews"
python3 - "$RUN/status.json" <<'PY'
import json, sys
st = json.load(open(sys.argv[1])); pr = st["prs"][1]; pr["prUrl"] = "https://github.com/o/rr/pull/7"; pr["review"]["round"] = 1
json.dump(st, open(sys.argv[1], "w"))
PY

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
for f in tickets.json meta.json writeback-posted.json tickets-pulls.json tickets-update.md app-checks.json app-cleanup.json app-github.json app-usage.json; do
  expect 1 "put refuses the app's $f" bash "$STATE" put "$RUN/$f" <<<'{}'
done

echo "# put writes only <DCV2_RUN_DIR>/status.json (CAP-1, CAP-2)"
PH="$d/home"; mkdir -p "$PH/.claude" "$d/appdata" "$d/runs/r-other"
for tgt in "$RUN/Repos.json" "$RUN/Requests/x.json" "$PH/.claude/settings.json" \
           "$d/appdata/adopted-runs.json" "$d/runs/r-other/status.json" "$RUN/notes.json" "$RUN/sub/status.json"; do
  rm -f "$tgt"
  expect 1 "put refuses ${tgt#$d/}" env HOME="$PH" bash "$STATE" put "$tgt" <<<'{"schema": 1}'
  [[ ! -e "$tgt" ]] && ok "  nothing written" || bad "  $tgt was written"
done
{ cp "$RUN/status.json" "$d/status.before" 2>/dev/null || : > "$d/status.before"; }
for v in Writeback.json .Guard.json Meta.JSON; do   # protected names in another case: refused (and not removed: the volume may ignore case)
  expect 1 "put refuses $v" bash "$STATE" put "$RUN/$v" <<<'{"schema": 1}'
done
for v in Status.json STATUS.JSON; do     # the same file on a case-insensitive volume: refused, whatever the volume
  expect 1 "put refuses $v (not the exact name)" bash "$STATE" put "$RUN/$v" <<<'{"schema": 1}'
  cmp -s "$RUN/status.json" "$d/status.before" && ok "  status.json unchanged" || bad "  status.json changed by $v"
done
expect 1 "put without DCV2_RUN_DIR refused" env -u DCV2_RUN_DIR bash "$STATE" put "$RUN/status.json" <<<'{"schema": 1}'
{ [[ -f "$RUN/status.json" ]] && cmp -s "$RUN/status.json" "$d/status.before" || { [[ ! -f "$RUN/status.json" && ! -s "$d/status.before" ]]; }; } && ok "  status.json unchanged" || bad "  status.json changed"
expect 1 "put of another run's status.json refused" bash "$STATE" put "$d/runs/r-other/status.json" <<<'{"schema": 1}'
ln -s "$RUN" "$d/runlink"
expect 0 "put through a symlinked run path is still the run's status.json" bash "$STATE" put "$d/runlink/status.json" < "$d/status.before"

echo "# schema must be the integer 1 (CAP-16)"
cp "$RUN/status.json" "$d/status.before"
for bad in '1.0' 'True' '"1"' 'None' '2'; do
  expect 1 "put with schema $bad refused" put < <(status planning "" "" "doc[\"schema\"]=$bad")
  cmp -s "$RUN/status.json" "$d/status.before" && ok "  status.json unchanged" || bad "  status.json changed by schema $bad"
done
expect 1 "put with no schema refused" put < <(status planning "" "" 'del doc["schema"]')

echo "# done gate, report (R10)"
bash "$STATE" log "$RUN" <<<'PR2 implementer retried: no JSON block' >/dev/null
echo "Updated topics/sync.md (checked-at abc1234)." > "$RUN/memory-update.md"
expect 0 "done" put < <(status done local-only opened 'doc["ship"]={"remote":"present","pushed":True,"note":None}; doc["prs"][1]["prUrl"]="https://github.com/o/rr/pull/7"')
has "  says it wrote the report" "report.md"
R="$RUN/report.md"
[[ -f "$R" ]] && ok "  report.md written at done" || bad "  no report.md"
out="$(cat "$R" 2>/dev/null)"
has "  stage" "Stage: **done**"
has "  risk review line" "- Risk: 1 review round(s), last verdict NO BLOCKING RISKS, 0 MEDIUM+ open (\`reviews/risk-round1.md\`)"
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
expect 0 "failed run" env DCV2_RUN_DIR="$RUN2" bash "$STATE" put "$RUN2/status.json" < "$d/failed.json"
grep -q "planner failed twice" "$RUN2/report.md" && ok "  report.md at failed, with the error" || bad "  report: $(cat "$RUN2/report.md" 2>/dev/null)"
rm -f "$RUN2/report.md"; mkdir "$RUN2/report.md"
expect 0 "a report that cannot be written does not fail the status write (LOW 9)" env DCV2_RUN_DIR="$RUN2" bash "$STATE" put "$RUN2/status.json" < "$d/failed.json"
has "  says so" "report.md not written"

echo "# BMAD mode: size, init, check, and the plan-approval refusal (run folder path has a space)"
B="$d/App Support/runs/r-b"; mkdir -p "$B"
printf 'folder: %s\n' "$WS" > "$B/request.md"
printf '# Plan\n## Stories\n### S1: x\n- Covers: CAP-1, AD-1\n## Pull requests\n### PR1: x\n- Repo: r\n- Stories: S1\n' > "$B/plan.md"
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

echo "# BMAD placeholders name only real template tokens (CAP-18)"
SPEC="$B/bmad/output/r-b/spec-toy/spec-toy.md"; cp "$SPEC" "$d/spec.keep"
{ cat "$d/spec.keep"; printf '\nThe endpoint is GET /invoices/{invoiceId} and returns {"ok": true}.\n'; } > "$SPEC"
expect 0 "prose with a REST path parameter passes" bash "$STATE" bmad-check "$B"
tok="$(grep -ho '{Product Name}' "$ROOT"/plugin/bmad/skills/*/assets/*template*.md | head -1)"
[[ "$tok" == "{Product Name}" ]] && ok "  {Product Name} is a vendored template token" || bad "  no template token found"
{ cat "$d/spec.keep"; printf '\n# %s\n' "$tok"; } > "$SPEC"
expect 3 "a vendored template token left in fails" bash "$STATE" bmad-check "$B"
has "  names it" "{Product Name}"
{ cat "$d/spec.keep"; printf '\nCall {{base_url}}/x.\n'; } > "$SPEC"
expect 3 "a double-brace token fails" bash "$STATE" bmad-check "$B"
has "  names it" "{{base_url}}"
{ cat "$d/spec.keep"; printf '\nIn code it is fine: `{Product Name}` and\n\n```\n{Product Name}\n```\n'; } > "$SPEC"
expect 0 "a template token inside code passes" bash "$STATE" bmad-check "$B"
cp "$d/spec.keep" "$SPEC"
rm -f "$B/bmad/output/r-b/ux-toy/validation-report.md" "$B/bmad/output/r-b/ux-toy/validation-report.html"
expect 3 "a UX folder without its validation reports fails" bash "$STATE" bmad-check "$B"
[[ "$(grep -o 'ux-toy/validation-report.md is missing' <<<"$out" | wc -l | tr -d ' ')" == 1 ]] && ok "  the missing report is named once, not once per document" || bad "  repeated: $out"
mkdocs prd ux
expect 0 "bmad-check passes again" bash "$STATE" bmad-check "$B"

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

echo "# BMAD: plan.md must trace to the documents"
W="$B/bmad/output/r-b"
# setplan <story blocks...> [-- <extra text>]: plan.md with the given story blocks (each "ID|Covers value", empty value = no Covers: line)
setplan() {
  { printf '# Plan\n## Stories\n'
    local x
    for x in "$@"; do printf '### %s: t\n' "${x%%|*}"; if [[ -n "${x#*|}" ]]; then printf -- '- Covers: %s\n' "${x#*|}"; fi; done
    printf '## Pull requests\n### PR1: x\n- Repo: r\n- Stories: S1\n'
    if [[ -n "${EXTRA:-}" ]]; then printf '\n## Out of scope\n%s\n' "$EXTRA"; fi
  } > "$B/plan.md"
}
bothrefused() { # <label> <must-mention>
  expect 1 "$1: put refused" bput < <(bstatus plan-approval)
  has "  names it" "$2"
  has "  trace message" "plan.md does not trace to the BMAD documents"
  expect 1 "$1: approve refused" bash "$STATE" approve "$B" plan user
  has "  names it (approve)" "$2"
}
bothallowed() {
  expect 0 "$1: put allowed" bput < <(bstatus plan-approval)
  expect 0 "$1: approve allowed" bash "$STATE" approve "$B" plan user
}
setplan "S1|CAP-1" "S2|"
bothrefused "story without Covers:" "S2 has no Covers: line"
setplan "S1|CAP-1, CAP-9"
bothrefused "unknown CAP" "unknown CAP-9"
setplan "S1|none"; bothrefused "Covers: none (no reason)" "S1 has a Covers: line with no ID"
setplan "S1|none:"; bothrefused "Covers: none: (no reason)" "S1 has a Covers: line with no ID"
setplan "S1|n/a"; bothrefused "Covers: n/a" "S1 has a Covers: line with no ID"
# a second CAP in the spec that no story covers
printf -- '- **CAP-2** users can remove\n' >> "$W/spec-toy/spec-toy.md"
printf -- '\n- **SM-C1** counter metric\n- **FR-1** a PRD requirement\n' >> "$W/spec-toy/spec-toy.md"
expect 0 "bmad-check at plan-approval with an untraced plan still records pass" bash "$STATE" bmad-check "$B"
is "  recorded" "$B/status.json" bmad.check.result '"pass"'
setplan "S1|CAP-1"
bothrefused "CAP-2 covered by no story" "CAP-2 is covered by no story"
setplan "S1|CAP-1" "S2|none: refactor needed by S1"
bothrefused "CAP-2 still uncovered with a none story" "CAP-2"
EXTRA="- CAP-2" setplan "S1|CAP-1"
bothrefused "Out of scope line without a reason" "CAP-2"
EXTRA="- CAP-2: not in this release" setplan "S1|CAP-1"
bothallowed "CAP-2 out of scope with a reason"
setplan "S1|CAP-1, CAP-2" "S2|none: refactor needed by S1" "S3|NONE - shared helper"
bothallowed "all covered, none forms (colon, upper-case dash)"
setplan "S1|CAP-1, CAP-2" "S2|SM-C1"
bothallowed "counter-metric SM-C1 alone"
setplan "S1|CAP-1, CAP-2" "S2|FR-1"
bothallowed "a known PRD-style ID"
setplan "S1|CAP-1, CAP-2" "S2|FR-7"
bothrefused "unknown FR-7" "unknown FR-7"
setplan "S1|CAP-1, CAP-2"
# non-BMAD runs: no Covers: needed
printf '# Plan\n## Stories\n### S1: x\n## Pull requests\n### PR1: x\n- Repo: r\n- Stories: S1\n' > "$B/plan.md"
rm -f "$B/status.json"; bmeta off
expect 0 "mode off: put plan-approval without Covers:" bput < <(bstatus plan-approval)
expect 0 "mode off: approve without Covers:" bash "$STATE" approve "$B" plan user
rm -f "$B/meta.json" "$B/status.json"
expect 0 "no meta.json: put plan-approval without Covers:" bput < <(bstatus plan-approval)
expect 0 "no meta.json: approve without Covers:" bash "$STATE" approve "$B" plan user
bmeta on; rm -f "$B/status.json"
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
export GH_SHIM_DIR="$d/cgh" DCV2_RUN_DIR="$CRUN" DCV2_TEST_LOCAL_REMOTES=1
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
echo "# cleanup: local folders only in the suites; the system git config (credential helper) is kept"
expect 0 "cleanup without DCV2_TEST_LOCAL_REMOTES" env -u DCV2_TEST_LOCAL_REMOTES PATH="$ROOT/scripts/skill-e2e/gh-shim:$PATH" bash "$STATE" cleanup "$CRUN"
grep -q 'refused: not a safe remote URL (only https / ssh GitHub-style remotes)' <<<"$out" && ok "  a local folder remote is refused in production" || bad "  out: $out"
export DCV2_TEST_LOCAL_REMOTES=1
mkdir -p "$d/credgit"; REALGIT0="$(command -v git)"
printf '[credential]\n\thelper = dcv2-test-helper\n' > "$d/system.gitconfig"
cat > "$d/credgit/git" <<EOF
#!/usr/bin/env bash
if [[ " \$* " == *" ls-remote "* ]]; then
  echo "nosystem=\${GIT_CONFIG_NOSYSTEM:-} helper=\$("$REALGIT0" config --get credential.helper)" >> "$d/cred.log"
fi
exec "$REALGIT0" "\$@"
EOF
chmod +x "$d/credgit/git"
expect 0 "cleanup with a system credential helper" env PATH="$d/credgit:$ROOT/scripts/skill-e2e/gh-shim:$PATH" GIT_CONFIG_SYSTEM="$d/system.gitconfig" bash "$STATE" cleanup "$CRUN"
grep -q '^nosystem= helper=dcv2-test-helper$' "$d/cred.log" && ok "  network git sees the system credential helper" || bad "  cred.log: $(cat "$d/cred.log" 2>/dev/null)"
python3 - "$CRUN/.guard.json" <<'PY'
import json, sys
g = json.load(open(sys.argv[1])); g["remotes"]["PR4"]["url"] = "http://github.com/o/r.git"; json.dump(g, open(sys.argv[1], "w"))
PY
expect 0 "an http:// remote" env PATH="$ROOT/scripts/skill-e2e/gh-shim:$PATH" bash "$STATE" cleanup "$CRUN"
grep -q 'refused: http:// remotes are not used (use https://)' <<<"$out" && ok "  refused with a clear message" || bad "  out: $out"

echo "# cleanup: a remote URL is never an option, and the delete is a lease"
mkdir -p "$d/fakegit"; REALGIT="$(command -v git)"
cat > "$d/fakegit/git" <<EOF
#!/usr/bin/env bash
if [[ -n "\${FAKE_TIP:-}" && " \$* " == *" ls-remote "* ]]; then echo "\$FAKE_TIP	refs/heads/x"; exit 0; fi
exec "$REALGIT" "\$@"
EOF
chmod +x "$d/fakegit/git"
python3 - "$CRUN/.guard.json" "$d" <<'PY'
import json, sys
g = json.load(open(sys.argv[1])); g["remotes"]["PR4"]["url"] = f"--upload-pack=touch {sys.argv[2]}/PWN;false"; json.dump(g, open(sys.argv[1], "w"))
PY
python3 - "$d/cgh/prs.json" "$T2" <<'PY'
import json, sys
p = json.load(open(sys.argv[1])); p[1]["state"] = "MERGED"; json.dump(p, open(sys.argv[1], "w"))
PY
# PR2's remote branch moves on after GitHub's answer (someone pushed): the lease must refuse the delete.
git -C "$CW/cr-worktrees/dcv2-r-c-pr2" -c user.name=t -c user.email=t@t commit -q --allow-empty -m late
git -C "$CW/cr-worktrees/dcv2-r-c-pr2" push -q "$CO" dcv2/r-c-pr2
expect 0 "cleanup with a forged remote URL and a racing tip" env PATH="$d/fakegit:$ROOT/scripts/skill-e2e/gh-shim:$PATH" FAKE_TIP="$T2" bash "$STATE" cleanup "$CRUN"
[[ ! -e "$d/PWN" ]] && ok "  --upload-pack URL never ran" || bad "  the URL ran a command"
grep -q '^PR4: .*remote branch kept (refused: not a safe remote URL)$' <<<"$out" && ok "  refused as unsafe" || bad "  PR4: $(grep '^PR4' <<<"$out")"
git -C "$CO" show-ref --verify --quiet refs/heads/dcv2/r-c-pr2 && ok "  PR2 kept: the lease failed (ls-remote said its head, the remote has moved on)" || bad "  PR2 deleted without the lease"
grep -q '^PR2: .*remote branch kept: ' <<<"$out" && ok "  says it was kept" || bad "  PR2: $(grep '^PR2' <<<"$out")"
python3 - "$CRUN/.guard.json" <<'PY'
import json, sys
g = json.load(open(sys.argv[1])); g["remotes"]["PR4"]["url"] = "ext::sh -c touch% /tmp/x"; json.dump(g, open(sys.argv[1], "w"))
PY
expect 0 "an ext:: URL is refused too" env PATH="$ROOT/scripts/skill-e2e/gh-shim:$PATH" bash "$STATE" cleanup "$CRUN"
grep -q '^PR4: .*remote branch kept (refused: not a safe remote URL)$' <<<"$out" && ok "  refused" || bad "  PR4: $(grep '^PR4' <<<"$out")"
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
mkdir -p "$d/runs/r-other"; printf 'folder: %s\n' "$CW" > "$d/runs/r-other/request.md"; echo '{"stage": "done"}' > "$d/runs/r-other/status.json"
expect 1 "request for another run than DCV2_RUN_DIR refused" bash "$STATE" request "$d/runs/r-other" selftest
[[ ! -d "$d/runs/r-other/requests" ]] && ok "  nothing written there" || bad "  requests/ made"
expect 1 "cleanup of another run refused" bash "$STATE" cleanup "$d/runs/r-other"
expect 1 "request-wait on another run refused" bash "$STATE" request-wait "$d/runs/r-other" 20261027T000000Z-recheck
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

echo "# worktree identity is the git common dir: a linked-worktree start folder, a --separate-git-dir repo (CAP-15)"
cleanup_layout() { # <tag> <repo path>: the run's own worktree of the repo is made by state.sh worktree, then cleaned up at done
  local tag="$1" repo="$2" lrun="$d/runs/r-$1" base wtp
  mkdir -p "$lrun"; printf 'folder: %s\n' "$(dirname "$repo")" > "$lrun/request.md"; echo '{"schema": 1, "reviewMode": "local"}' > "$lrun/meta.json"
  base="$(git -C "$repo" rev-parse HEAD)"
  expect 0 "[$tag] worktree made" env DCV2_RUN_DIR="$lrun" bash "$STATE" worktree "$lrun" "$repo" PR1 "$base"
  wtp="$(dirname "$repo")/$(basename "$repo")-worktrees/dcv2-r-$tag-pr1"
  python3 - "$lrun/status.json" "$tag" "$repo" "$wtp" <<'PY'
import json, sys
f, tag, repo, wt = sys.argv[1:5]
json.dump({"schema": 1, "runId": "r-" + tag, "stage": "done", "prs": [{"id": "PR1", "repoPath": repo, "branch": f"dcv2/r-{tag}-pr1", "worktree": wt, "state": "reviewed"}]}, open(f, "w"))
PY
  expect 0 "[$tag] cleanup accepts the run's own worktree" env DCV2_RUN_DIR="$lrun" bash "$STATE" cleanup "$lrun"
  has "  [$tag] removed" "PR1: worktree removed"
  [[ ! -d "$wtp" ]] && ok "  [$tag] the folder is gone" || bad "  [$tag] worktree still there"
}
LMAIN="$d/lw/main"; LSTART="$d/lw/start"; mkdir -p "$LMAIN"
git -C "$LMAIN" init -q -b main; git -C "$LMAIN" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
git -C "$LMAIN" worktree add -q -b feature/start "$LSTART"
cleanup_layout lnk "$LSTART"
SWORK="$d/sw/work"; mkdir -p "$SWORK" "$d/sw/gd"
git -C "$SWORK" init -q -b main --separate-git-dir "$d/sw/gd/work.git"; git -C "$SWORK" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
[[ -f "$SWORK/.git" ]] && ok "  the separate-git-dir repo's .git is a file" || bad "  .git is not a file"
cleanup_layout sep "$SWORK"
# a worktree of an unrelated repo is still refused: it sits where the contract puts the recorded repo's worktree, but is not that repo's
URUN="$d/runs/r-unrel"; mkdir -p "$URUN" "$d/lw/unrelated"; printf 'folder: %s\n' "$d/lw" > "$URUN/request.md"
git -C "$d/lw/unrelated" init -q -b main; git -C "$d/lw/unrelated" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
UWT="$d/lw/unrelated-worktrees/dcv2-r-unrel-pr1"; git -C "$LSTART" worktree add -q -b dcv2/r-unrel-pr1 "$UWT"
python3 - "$URUN" "$d/lw/unrelated" "$UWT" <<'PY'
import json, sys
run, repo, wt = sys.argv[1:4]
json.dump({"schema": 1, "runId": "r-unrel", "stage": "done", "prs": [{"id": "PR1", "repoPath": repo, "branch": "dcv2/r-unrel-pr1", "worktree": wt, "state": "reviewed"}]}, open(run + "/status.json", "w"))
json.dump({"schema": 1, "runDir": run, "repos": [repo], "worktrees": {"PR1": wt}}, open(run + "/.guard.json", "w"))
PY
expect 0 "cleanup with a worktree of another repo" env DCV2_RUN_DIR="$URUN" bash "$STATE" cleanup "$URUN"
has "  kept: not a worktree of its recorded repo" "not a worktree of its recorded repo"
[[ -d "$UWT" ]] && ok "  the unrelated worktree is untouched" || bad "  it was removed"

echo "# report: what the run could not do"
bash "$STATE" log "$CRUN" <<<'not done: re-run the checks (the app did not answer: is DCTerminalv2 running?)' >/dev/null
expect 0 "report" bash "$STATE" report "$CRUN"
grep -q '^## Not done by the run' "$CRUN/report.md" && grep -qF -- '- re-run the checks (the app did not answer' "$CRUN/report.md" && ok "  listed in report.md" || bad "  report: $(cat "$CRUN/report.md")"

echo "# epic-plan mode: waves, refusals, and the full sequence (CAP-11)"
B="$d/Epic Dir/runs/r-b"; mkdir -p "$B"; export DCV2_RUN_DIR="$B"
printf 'folder: %s\n' "$WS" > "$B/request.md"
printf '{"schema":1,"repos":[{"name":"r","path":"%s","check":null}]}\n' "$R" > "$B/repos.json"
printf '{"schema":1,"mode":"epic-plan","bmad":{"mode":"auto","version":"x"}}\n' > "$B/meta.json"
python3 -c 'import json, sys
json.dump({"tasks": [{"id": f"t{i}", "custom_id": f"C-{i}", "name": f"Ticket {i}"} for i in range(1, 9)]}, open(sys.argv[1], "w"))' "$B/tickets.json"
wave() { printf '%s\n' "$@" > "$B/waves.md"; }
W1=("## Wave 1: Foundations" "- Tickets: t1, t2, C-3" "- Depends on: none" "- Why: everything else needs these")
W2=("## Wave 2: Features" "- Tickets: t4, t5, t6" "- Depends on: Wave 1" "- Why: built on the foundations")
OOS=("## Out of scope" "- t7: a separate project" "- t8: waits on a vendor")
expect 0 "epic: put planning" bput < <(bstatus planning)
expect 1 "epic: waves refused with no waves.md" bash "$STATE" waves "$B"
wave "${W1[@]}" "${W2[@]}" "## Out of scope" "- t7: a separate project"
expect 1 "waves: a ticket in no wave is refused" bash "$STATE" waves "$B"
has "  names the missing ticket" "ticket t8 is in no wave"
wave "${W1[@]}" "## Wave 2: Features" "- Tickets: t4, t5, t6, t1" "- Depends on: Wave 1" "- Why: x" "${OOS[@]}"
expect 1 "waves: a duplicate ticket is refused" bash "$STATE" waves "$B"
has "  names it" "ticket t1 is in 2 places"
wave "## Wave 1: Too big" "- Tickets: t1, t2, t3, t4, t5, t6, t7" "- Depends on: none" "- Why: x" "## Out of scope" "- t8: later"
expect 1 "waves: a 7-ticket wave is refused" bash "$STATE" waves "$B"
has "  names the size rule" "has 7 ticket(s)"
wave "## Wave 1: Too small" "- Tickets: t1, t2" "- Depends on: none" "- Why: x" "${W2[@]}" "## Out of scope" "- t3: x" "- t7: y" "- t8: z"
expect 1 "waves: a 2-ticket wave is refused when 3 or more are in scope" bash "$STATE" waves "$B"
wave "## Wave 1: First" "- Tickets: t1, t2, t3" "- Depends on: Wave 2" "- Why: x" "## Wave 2: Second" "- Tickets: t4, t5, t6" "- Depends on: none" "- Why: y" "${OOS[@]}"
expect 1 "waves: a forward dependency is refused" bash "$STATE" waves "$B"
has "  says earlier waves only" "dependencies name earlier waves only"
wave "## Wave 1: First" "- Tickets: t1, t2, t3" "- Depends on: none" "- Why: x" "## Wave 2: Second" "- Tickets: t4, t5, nope" "- Depends on: Wave 1" "- Why: y" "${OOS[@]}"
expect 1 "waves: an unknown ticket is refused" bash "$STATE" waves "$B"
[[ ! -e "$B/waves.json" ]] && ok "  no waves.json after a refusal" || bad "  waves.json written"
expect 1 "epic: with no sizing and no waves digest, plan-approval is refused" bput < <(bstatus plan-approval)
echo "# epic-plan mode: refusals"
expect 1 "epic: worktree refused" bash "$STATE" worktree "$B" "$R" PR1 "$BASE"
has "  says epic-plan" "epic-plan run"
for c in draft-pr ship-pr push ready; do expect 1 "epic: $c refused" bash "$STATE" "$c" "$B" PR1; done
expect 1 "epic: stage building refused" bput < <(bstatus building)
bmod() { python3 -c 'import json,sys; doc=json.load(sys.stdin); exec(sys.argv[1]); print(json.dumps(doc))' "$1"; }
expect 1 "epic: a non-empty prs[] refused" bput < <(bstatus planning | bmod 'doc["prs"]=[{"id":"PR1","state":"pending"}]')
echo "# epic-plan mode: the full sequence (BMAD auto, tickets.json, no plan.md)"
expect 0 "epic: bmad-size Large" bsize Large --by planner
expect 0 "epic: bmad-init" bash "$STATE" bmad-init "$B"
mkdocs
expect 0 "epic: bmad-check passes" bash "$STATE" bmad-check "$B"
wave "${W1[@]}" "${W2[@]}" "${OOS[@]}"
expect 0 "epic: waves passes" bash "$STATE" waves "$B"
has "  counts" "waves=pass 2 wave(s), 6 ticket(s) in scope, 2 out of scope"
[[ -f "$B/waves.json" ]] && ok "  waves.json written" || bad "  no waves.json"
is "  wave 1 tickets use the ticket ids" "$B/waves.json" waves.0.tickets '["t1", "t2", "t3"]'
is "  wave 2 depends on 1" "$B/waves.json" waves.1.dependsOn '[1]'
python3 -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))["waves"]["digest"] else 1)' "$B/status.json" && ok "  status.json waves digest recorded" || bad "  no waves digest"
is "  status.json schema stays 1" "$B/status.json" schema 1
echo "x" >> "$B/waves.md"
expect 1 "epic: plan-approval refused on a digest mismatch" bput < <(bstatus plan-approval)
has "  says waves.md changed" "waves.md changed"
expect 1 "epic: approve refused on a digest mismatch" bash "$STATE" approve "$B" plan user
sed '$d' "$B/waves.md" > "$B/waves.tmp" && mv "$B/waves.tmp" "$B/waves.md"
expect 0 "epic: plan-approval put (no plan.md exists)" bput < <(bstatus plan-approval)
expect 0 "epic: put with a forged waves field" bput < <(bstatus plan-approval | bmod 'doc["waves"]={"digest":"forged","at":"x"}')
python3 -c 'import json,sys; sys.exit(1 if json.load(open(sys.argv[1]))["waves"]["digest"]=="forged" else 0)' "$B/status.json" && ok "  the forged waves value was not kept" || bad "  forged digest kept"
expect 0 "epic: approve plan user (no plan.md)" bash "$STATE" approve "$B" plan user
[[ ! -e "$B/plan.md" ]] && ok "  there is no plan.md" || bad "  plan.md exists"
expect 0 "epic: writeback writes nothing" bash "$STATE" writeback "$B"
has "  says so" "epic-plan run: no writeback"
[[ ! -e "$B/writeback.json" ]] && ok "  no writeback.json" || bad "  writeback.json written"
expect 0 "epic: put done with no shipApproval, ship or writeback.json" bput < <(bstatus done)
is "  stage done" "$B/status.json" stage '"done"'
echo "# the same sequence in a normal run still refuses done without shipApproval, ship and writeback.json"
B="$d/Normal Dir/runs/r-b"; mkdir -p "$B"; export DCV2_RUN_DIR="$B"
printf 'folder: %s\n' "$WS" > "$B/request.md"
printf '{"schema":1,"bmad":{"mode":"off"}}\n' > "$B/meta.json"
printf '{"tasks":[{"id":"t1","name":"T"}]}\n' > "$B/tickets.json"
printf '# Plan\n## Stories\n### S1: x\n## Pull requests\n### PR1: x\n- Repo: r\n- Stories: S1\n' > "$B/plan.md"
expect 0 "normal: put planning" bput < <(bstatus planning)
expect 0 "normal: approve plan" bash "$STATE" approve "$B" plan user
expect 1 "normal: done refused without shipApproval" bput < <(bstatus done)
has "  needs the ship approval" "ship approval"
expect 1 "normal: waves refused" bash "$STATE" waves "$B"
expect 0 "normal: writeback is not skipped" bash "$STATE" writeback "$B"
[[ -f "$B/writeback.json" ]] && ok "  writeback.json written in a normal run" || bad "  no writeback.json"

echo "# no usable Python (W2)"
nopy="$d/nopy"; mkdir -p "$nopy"; ln -sf "$(command -v bash)" "$nopy/bash"
expect 1 "state.sh preflight with no Python 3.8+ says so" env PATH="$nopy" "$BASH" "$STATE" preflight "$RUN"
has "  names the three commands" "tried python3, python, py -3"
expect 1 "state.sh (any command but now|log) with no Python 3.8+ says so" env PATH="$nopy" "$BASH" "$STATE" now2 "$RUN"
has "  names the three commands again" "tried python3, python, py -3"
if grep -q "unbound variable" <<<"$out"; then bad "  unbound variable crash"; else ok "  no unbound variable crash"; fi

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
