#!/usr/bin/env bash
# Unit test of state.sh's github review mode (draft-pr, push, post-review, comment, ready, mode) and the
# worktree setup / copyFiles step. No Claude call, no network: gh is scripts/skill-e2e/gh-shim/gh and
# the remote is a local bare repo.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STATE="$ROOT/plugin/scripts/state.sh"
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
trap 'rm -rf "$d"' EXIT
unset DCV2_TEST_MODE DCV2_MEMORY_DIR DCV2_RUN_DIR || true
export DCV2_TEST_LOCAL_REMOTES=1   # the remote is a local bare repo (state.sh accepts local folders only in the suites)

fails=0
ok()   { echo "ok   $*"; }
bad()  { echo "FAIL $*"; fails=1; }
expect() {
  local want="$1" label="$2" got=0; shift 2
  out="$("$@" 2>&1)" || got=$?
  if [[ $got == "$want" ]]; then ok "$label -> $got"; else bad "$label -> $got (want $want): ${out:0:400}"; fi
}
has()  { if grep -qF -- "$2" <<<"$out"; then ok "$1"; else bad "$1: no '$2' in: ${out:0:400}"; fi; }
hasnt(){ if grep -qF -- "$2" <<<"$out"; then bad "$1: unexpected '$2' in: ${out:0:400}"; else ok "$1"; fi; }
field(){ python3 -c 'import json,sys; d=json.load(open(sys.argv[1]));
for k in sys.argv[2].split("."): d = (d[int(k)] if int(k) < len(d) else None) if isinstance(d, list) else (d or {}).get(k)
print(json.dumps(d))' "$1" "$2"; }
is()   { local got; got="$(field "$2" "$3")"; if [[ "$got" == "$4" ]]; then ok "$1"; else bad "$1: $3 = $got (want $4)"; fi; }
S() { bash "$STATE" "$@"; }

export GH_SHIM_DIR="$d/gh"; mkdir -p "$GH_SHIM_DIR"
fakebin="$d/bin"; mkdir -p "$fakebin"
cat > "$fakebin/npm" <<'EOF'
#!/bin/sh
echo "fake npm $*"
EOF
chmod +x "$fakebin/npm"
export PATH="$ROOT/scripts/skill-e2e/gh-shim:$fakebin:$PATH"
[[ "$(command -v gh)" == "$ROOT/scripts/skill-e2e/gh-shim/gh" ]] || { echo "FAIL the gh shim is not first on PATH"; exit 1; }

# R: no remote, has a package-lock.json. RR: a local bare origin, a git-ignored .env and a tracked file.
WS="$d/ws"; R="$WS/r"; RR="$WS/rr"; RUN="$d/runs/r-g"
mkdir -p "$R" "$RR" "$RUN" "$d/outside"
echo '{}' > "$R/package-lock.json"
printf '.env\n.env.*\n.setup-marker\nsecret-link.env\n' > "$RR/.gitignore"; echo tracked > "$RR/tracked.txt"
for repo in "$R" "$RR"; do
  git -C "$repo" init -q -b main; git -C "$repo" config user.name t; git -C "$repo" config user.email t@t
  git -C "$repo" add -A; git -C "$repo" commit -q -m init
done
echo 'API_URL=http://localhost' > "$RR/.env"; echo 'outside' > "$d/outside/x.env"
ln -s "$d/outside/x.env" "$RR/secret-link.env"
git init -q --bare -b main "$d/origin.git"
git -C "$RR" remote add origin "$d/origin.git"; git -C "$RR" push -q origin main
git -C "$RR" remote set-head origin main >/dev/null 2>&1 || true
BASE="$(git -C "$R" rev-parse HEAD)"; RBASE="$(git -C "$RR" rev-parse HEAD)"
printf 'folder: %s\n\n# Request\nx\n' "$WS" > "$RUN/request.md"
cat > "$RUN/plan.md" <<'EOF'
# Plan
## Stories
### S1: Fix multiply
### S2: Prefix results
## Pull requests
### PR1: Fix multiply
- Stories: S1
### PR2: Prefix
- Stories: S2
EOF
python3 - "$RUN/repos.json" "$R" "$RR" <<'PY'
import json, sys
json.dump({"schema": 1, "repos": [
  {"name": "r", "path": sys.argv[2], "check": "true", "setup": None, "copyFiles": []},
  {"name": "rr", "path": sys.argv[3], "check": "true", "setup": "echo ran > .setup-marker && echo setup-done",
   "copyFiles": [".env", ".env.local", "tracked.txt", "../outside/x.env", "/etc/hosts", "secret-link.env"]}]}, open(sys.argv[1], "w"))
PY
export DCV2_RUN_DIR="$RUN"

# st <stage> <pr1 state> <pr2 state> [python]: write status.json directly (as state.sh would have).
st() {
  python3 - "$RUN" "$R" "$RR" "$BASE" "$RBASE" "$@" <<'PY'
import json, os, sys
run, r, rr, base, rbase, stage, s1, s2 = sys.argv[1:9]
extra = sys.argv[9] if len(sys.argv) > 9 else ""
rid = os.path.basename(run)
old = {}
try:
    old = {p["id"]: p for p in json.load(open(os.path.join(run, "status.json")))["prs"]}
except Exception:
    pass
def pr(i, repo, path, b, state, story):
    lc = f"pr{i}"
    p = {"id": f"PR{i}", "title": f"PR {i} title", "repo": repo, "repoPath": path, "branch": f"dcv2/{rid}-{lc}", "base": "main",
         "baseCommit": b, "worktree": os.path.join(os.path.dirname(path), repo + "-worktrees", f"dcv2-{rid}-{lc}"),
         "dependsOn": [], "stories": {"done": 1, "total": 1, "ids": [story]}, "commits": 1,
         "check": {"command": "true", "result": "pass", "summary": None}, "review": {"round": 1, "verdict": "APPROVED", "openFindings": 0, "path": None},
         "prUrl": None, "state": state, "note": None}
    for k in ("prUrl", "draft", "comments"):
        if k in old.get(p["id"], {}):
            p[k] = old[p["id"]][k]
    return p
doc = {"schema": 1, "runId": rid, "stage": stage, "waitingFor": None, "folder": os.path.dirname(r),
       "plan": {"path": "plan.md", "reviewRound": 1, "verdict": "APPROVED", "openFindings": 0, "approved": True,
                "approval": {"by": "user", "at": "2026-10-24T00:00:00Z"}},
       "prs": [pr(1, "r", r, base, s1, "S1"), pr(2, "rr", rr, rbase, s2, "S2")],
       "shipApproval": None, "ship": None, "writeback": None, "error": None}
exec(extra)
json.dump(doc, open(os.path.join(run, "status.json"), "w"))
PY
}

echo "# mode (meta.json reviewMode; missing = local)"
expect 0 "mode with no meta.json" S mode "$RUN"; has "  local" "reviewMode=local"
echo '{"schema": 1, "reviewMode": "local"}' > "$RUN/meta.json"
expect 0 "mode local" S mode "$RUN"; has "  local" "reviewMode=local"
echo '{"schema": 1, "reviewMode": "github"}' > "$RUN/meta.json"
expect 0 "mode github" S mode "$RUN"; has "  github" "reviewMode=github"
expect 0 "preflight prints the mode" S preflight "$RUN"; has "  reviewMode=github" "reviewMode=github"

echo "# worktree: copyFiles + setup (new worktrees only)"
st building pending pending 'for p in doc["prs"]: p["commits"] = 0'
expect 0 "PR2 worktree made" S worktree "$RUN" "$RR" PR2 "$RBASE"
WT2="$WS/rr-worktrees/dcv2-r-g-pr2"; WT1="$WS/r-worktrees/dcv2-r-g-pr1"
has "  copied one file" "copied=1"
has "  refused the tracked file" "tracked.txt: tracked by git"
has "  refused a path out of the repo" "../outside/x.env: not a plain relative path"
has "  refused an absolute path" "/etc/hosts: not a plain relative path"
has "  refused a symlink" "secret-link.env: a symlink"
hasnt "  a missing optional file is not mentioned" ".env.local"
has "  setup ok" "setup=ok log=$RUN/logs/PR2-setup.txt"
[[ "$(cat "$WT2/.env" 2>/dev/null)" == "API_URL=http://localhost" ]] && ok "  .env copied" || bad "  .env not copied"
[[ ! -e "$WT2/secret-link.env" ]] && ok "  symlink not copied" || bad "  symlink copied"
[[ -f "$WT2/.setup-marker" ]] && ok "  setup ran in the worktree" || bad "  no .setup-marker"
grep -q "setup-done" "$RUN/logs/PR2-setup.txt" && ok "  setup output in the log" || bad "  log: $(cat "$RUN/logs/PR2-setup.txt")"
is "  .guard.json records the copied file" "$RUN/.guard.json" copied.PR2 '[".env"]'
[[ -z "$(git -C "$WT2" status --porcelain)" ]] && ok "  worktree still clean (copied + setup files are ignored)" || bad "  worktree dirty: $(git -C "$WT2" status --porcelain)"
expect 0 "PR2 worktree reused" S worktree "$RUN" "$RR" PR2 "$RBASE"
hasnt "  no setup on reuse" "setup="
expect 0 "PR1 worktree: setup detected from package-lock.json" S worktree "$RUN" "$R" PR1 "$BASE"
has "  setup ok" "setup=ok"
grep -q "fake npm ci" "$RUN/logs/PR1-setup.txt" && ok "  ran npm ci" || bad "  log: $(cat "$RUN/logs/PR1-setup.txt")"
python3 - "$RUN/repos.json" <<'PY'
import json, sys
j = json.load(open(sys.argv[1])); j["repos"][0]["setup"] = "sleep 30"; json.dump(j, open(sys.argv[1], "w"))
PY
rm -rf "$WT1"; git -C "$R" worktree prune
t0=$SECONDS
expect 0 "a setup that times out does not fail the worktree" env DCV2_SETUP_TIMEOUT=1 bash "$STATE" worktree "$RUN" "$R" PR1 "$BASE"
has "  setup failed" "setup=failed log="; has "  worktree printed" "worktree=$WT1"
(( SECONDS - t0 < 15 )) && ok "  killed at the timeout" || bad "  took $((SECONDS - t0)) s"
grep -q "timed out" "$RUN/logs/PR1-setup.txt" && ok "  the log says it timed out" || bad "  log: $(cat "$RUN/logs/PR1-setup.txt")"
python3 - "$RUN/repos.json" <<'PY'
import json, sys
j = json.load(open(sys.argv[1])); j["repos"][0]["setup"] = "exit 4"; json.dump(j, open(sys.argv[1], "w"))
PY
rm -rf "$WT1"; git -C "$R" worktree prune
expect 0 "a failing setup does not fail the worktree" S worktree "$RUN" "$R" PR1 "$BASE"
has "  setup failed" "setup=failed"
# a link planted at the setup log's name is not written through (CAP-8): the setup does not run, the outside file stays as it was
python3 - "$RUN/repos.json" <<'PY'
import json, sys
j = json.load(open(sys.argv[1])); j["repos"][0]["setup"] = "echo ran > .setup-marker"; json.dump(j, open(sys.argv[1], "w"))
PY
echo outside > "$d/outside-setup.txt"; rm -f "$RUN/logs/PR1-setup.txt"; ln -s "$d/outside-setup.txt" "$RUN/logs/PR1-setup.txt"
rm -rf "$WT1"; git -C "$R" worktree prune
expect 0 "a setup log that is a link: the worktree still comes" S worktree "$RUN" "$R" PR1 "$BASE"
has "  setup failed" "setup=failed"
[[ "$(cat "$d/outside-setup.txt")" == outside ]] && ok "  the outside file is unchanged" || bad "  outside file: $(cat "$d/outside-setup.txt")"
[[ ! -e "$WT1/.setup-marker" ]] && ok "  the setup did not run" || bad "  the setup ran"
rm -f "$RUN/logs/PR1-setup.txt"
python3 - "$RUN/repos.json" <<'PY'
import json, sys
j = json.load(open(sys.argv[1])); j["repos"][0]["setup"] = ""; json.dump(j, open(sys.argv[1], "w"))
PY
rm -rf "$WT1"; git -C "$R" worktree prune
expect 0 "an empty setup means none" S worktree "$RUN" "$R" PR1 "$BASE"; has "  none" "setup=none"
git -C "$WT2" commit -q --allow-empty -m "S2: prefix"
git -C "$WT1" commit -q --allow-empty -m "S1: fix"

echo "# draft-pr"
st building building building 'doc["plan"]["approval"] = None; doc["stage"] = "plan-approval"'
expect 1 "refused before the plan approval" S draft-pr "$RUN" PR2
has "  says so" "plan approval"
echo '{"schema": 1}' > "$RUN/meta.json"
st building building building
expect 1 "refused in local mode" S draft-pr "$RUN" PR2
has "  says local" "review mode is local"
echo '{"schema": 1, "reviewMode": "github"}' > "$RUN/meta.json"
st building building building 'doc["prs"][1]["check"]["result"] = "fail"'
expect 1 "refused when the check fails" S draft-pr "$RUN" PR2
st building building building
expect 0 "PR1 has no remote: no draft" S draft-pr "$RUN" PR1
has "  no-remote" "draft=no-remote"; is "  PR1 still building" "$RUN/status.json" prs.0.state '"building"'
expect 0 "PR2 draft opened" S draft-pr "$RUN" PR2
has "  opened" "draft=opened url=https://github.com/shim/repo/pull/1"
is "  prUrl" "$RUN/status.json" prs.1.prUrl '"https://github.com/shim/repo/pull/1"'
is "  draft true" "$RUN/status.json" prs.1.draft true
is "  state drafted" "$RUN/status.json" prs.1.state '"drafted"'
is "  comments []" "$RUN/status.json" prs.1.comments '[]'
git -C "$d/origin.git" show-ref -q --verify refs/heads/dcv2/r-g-pr2 && ok "  branch pushed" || bad "  branch not on origin"
grep -q "pr create --draft --base main --head dcv2/r-g-pr2 --title PR 2 title" "$GH_SHIM_DIR/calls.log" && ok "  gh pr create --draft" || bad "  calls: $(cat "$GH_SHIM_DIR/calls.log")"
grep -q "review in progress" "$RUN/pr-bodies/PR2.md" && ok "  body says review in progress" || bad "  body: $(cat "$RUN/pr-bodies/PR2.md")"
grep -q "S2: Prefix results" "$RUN/pr-bodies/PR2.md" && ok "  body lists the stories" || bad "  body: $(cat "$RUN/pr-bodies/PR2.md")"
grep -q "draft PR opened" "$RUN/log.md" && ok "  logged" || bad "  not logged"
expect 0 "draft-pr again" S draft-pr "$RUN" PR2; has "  exists" "draft=exists"
python3 - "$RUN/status.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1])); p = s["prs"][1]; p["prUrl"] = None; p.pop("draft"); p["state"] = "building"; json.dump(s, open(sys.argv[1], "w"))
PY
expect 0 "resume: the branch's PR is found, not opened twice" S draft-pr "$RUN" PR2
has "  found" "draft=found url=https://github.com/shim/repo/pull/1"
[[ "$(grep -c 'pr create' "$GH_SHIM_DIR/calls.log")" == 1 ]] && ok "  one gh pr create in all" || bad "  creates: $(grep -c 'pr create' "$GH_SHIM_DIR/calls.log")"

echo "# put keeps what only state.sh writes"
st building building reviewing 'doc["prs"][1]["prUrl"] = None; doc["prs"][1].pop("draft", None); doc["prs"][1].pop("comments", None)'
python3 - "$RUN/status.json" > "$d/doc.json" <<'PY'
import json, sys
print(json.dumps(json.load(open(sys.argv[1]))))
PY
# (st wrote the file directly; restore the recorded fields first, then let the orchestrator put a doc without them)
python3 - "$RUN/status.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1])); p = s["prs"][1]; p.update(prUrl="https://github.com/shim/repo/pull/1", draft=True, comments=[]); json.dump(s, open(sys.argv[1], "w"))
PY
expect 0 "orchestrator put without prUrl/draft/comments" S put "$RUN/status.json" < "$d/doc.json"
is "  prUrl kept" "$RUN/status.json" prs.1.prUrl '"https://github.com/shim/repo/pull/1"'
is "  draft kept" "$RUN/status.json" prs.1.draft true
is "  comments kept" "$RUN/status.json" prs.1.comments '[]'
python3 -c 'import json,sys; s=json.load(open(sys.argv[1])); s["prs"][1]["comments"]=[{"round":1,"url":"https://evil"}]; s["prs"][1]["draft"]=False; print(json.dumps(s))' "$RUN/status.json" > "$d/doc.json"
expect 0 "orchestrator put with forged comments/draft" S put "$RUN/status.json" < "$d/doc.json"
is "  forged comments dropped" "$RUN/status.json" prs.1.comments '[]'
is "  forged draft dropped" "$RUN/status.json" prs.1.draft true
python3 -c 'import json,sys; s=json.load(open(sys.argv[1])); s["prs"][0]["state"]="drafted"; print(json.dumps(s))' "$RUN/status.json" > "$d/doc.json"
expect 1 "drafted without a prUrl refused" S put "$RUN/status.json" < "$d/doc.json"

echo "# post-review"
expect 1 "post-review with no review file" S post-review "$RUN" PR2 1
expect 1 "post-review round 4 refused" S post-review "$RUN" PR2 4
mkdir -p "$RUN/reviews"
cat > "$RUN/reviews/PR2-round1.md" <<'EOF'
Eagle-Eye review — round 1: REQUEST CHANGES — 1 MEDIUM+, 0 LOW

## Verdict
REQUEST CHANGES

## Findings
1. MEDIUM — format.js:2 — formatResult(0) returns "0" — prefix it with "Result: "
EOF
expect 0 "post-review round 1" S post-review "$RUN" PR2 1
has "  posted" "posted=ok url=https://github.com/shim/repo/pull/1#issuecomment-1001"
is "  comment recorded" "$RUN/status.json" prs.1.comments.0.url '"https://github.com/shim/repo/pull/1#issuecomment-1001"'
is "  round recorded" "$RUN/status.json" prs.1.comments.0.round 1
grep -q "Eagle-Eye review — round 1: REQUEST CHANGES" "$GH_SHIM_DIR/comments/1-1.md" && ok "  the review is the comment" || bad "  comment: $(cat "$GH_SHIM_DIR/comments/1-1.md")"
[[ -f "$RUN/pr-comments/PR2-round1.md" ]] && ok "  pr-comments/PR2-round1.md kept" || bad "  no pr-comments file"
expect 0 "post-review round 1 again" S post-review "$RUN" PR2 1
has "  already" "posted=already"
[[ "$(ls "$GH_SHIM_DIR/comments" | wc -l | tr -d ' ')" == 1 ]] && ok "  not re-posted" || bad "  comments: $(ls "$GH_SHIM_DIR/comments")"
printf 'Eagle-Eye review — round 2: APPROVED — 0 MEDIUM+, 0 LOW\n' > "$RUN/reviews/PR2-round2.md"
expect 0 "post-review with gh failing" env GH_SHIM_FAIL=comment bash "$STATE" post-review "$RUN" PR2 2
has "  failed, nothing recorded" "posted=failed"
is "  still one comment" "$RUN/status.json" prs.1.comments.1 null
python3 -c 'print("Eagle-Eye review — round 2: APPROVED — 0 MEDIUM+, 1 LOW\n\n" + "LOW — a.js:1 — long text — none\n" * 3000)' > "$RUN/reviews/PR2-round2.md"
expect 0 "post-review of a review over the cap" S post-review "$RUN" PR2 2
n="$(python3 -c 'import sys; print(len(open(sys.argv[1]).read()))' "$GH_SHIM_DIR/comments/1-2.md")"
(( n <= 60200 )) && ok "  capped ($n chars)" || bad "  $n chars"
grep -q "The full review is in the run folder: reviews/PR2-round2.md" "$GH_SHIM_DIR/comments/1-2.md" && ok "  says where the full review is" || bad "  no tail"
printf 'Eagle-Eye review — round 3: APPROVED — 0 MEDIUM+, 0 LOW\n\nThe env had ANTHROPIC_API_KEY=sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWx0123456789\n' > "$RUN/reviews/PR2-round3.md"
expect 3 "post-review with a secret in it is refused" S post-review "$RUN" PR2 3
has "  says why, not the value" "looks like a secret"; hasnt "  never prints the value" "AbCdEfGh"
is "  PR2 needs attention" "$RUN/status.json" prs.1.state '"needs-attention"'
[[ "$(ls "$GH_SHIM_DIR/comments" | wc -l | tr -d ' ')" == 2 ]] && ok "  nothing sent" || bad "  comments: $(ls "$GH_SHIM_DIR/comments")"
grep -rq "AbCdEfGh" "$GH_SHIM_DIR" "$RUN/pr-comments" && bad "  the secret reached the shim or pr-comments" || ok "  the secret went nowhere"

echo "# push + comment fixes"
st building building fixing 'doc["prs"][1]["review"]["round"] = 1'
expect 1 "comment fixes with no new commit" S comment "$RUN" PR2 fixes 1
expect 1 "comment with another template refused" S comment "$RUN" PR2 hello 1
git -C "$WT2" commit -q --allow-empty -m "PR2 review fixes: prefix zero"
expect 1 "comment fixes before the push" S comment "$RUN" PR2 fixes 1
has "  push first" "push the fixes first"
expect 0 "push" S push "$RUN" PR2; has "  ok" "push=ok"
[[ "$(git -C "$d/origin.git" rev-parse dcv2/r-g-pr2)" == "$(git -C "$WT2" rev-parse HEAD)" ]] && ok "  origin has the fix" || bad "  origin behind"
expect 0 "push again" S push "$RUN" PR2; has "  up to date" "push=up-to-date"
expect 0 "comment fixes 1" S comment "$RUN" PR2 fixes 1
has "  posted" "posted=ok"
c="$(ls -t "$GH_SHIM_DIR/comments" | head -1)"
grep -q "^Round 1 fixes: 1 commit(s)" "$GH_SHIM_DIR/comments/$c" && grep -q "PR2 review fixes: prefix zero" "$GH_SHIM_DIR/comments/$c" && ok "  fixed template with the commit" || bad "  comment: $(cat "$GH_SHIM_DIR/comments/$c")"
is "  fixesUrl recorded" "$RUN/status.json" prs.1.comments.0.fixesUrl "\"https://github.com/shim/repo/pull/1#issuecomment-1003\""
expect 0 "comment fixes 1 again" S comment "$RUN" PR2 fixes 1; has "  already" "posted=already"
git -C "$WT2" commit -q --amend --allow-empty -m "PR2 review fixes: rewritten"
expect 0 "push of rewritten history never forces" S push "$RUN" PR2; has "  failed" "push=failed"
git -C "$WT2" reset -q --hard "$(git -C "$d/origin.git" rev-parse dcv2/r-g-pr2)"
expect 0 "PR1 (no draft): push skipped" S push "$RUN" PR1; has "  skipped" "push=skipped"
expect 0 "PR1 (no draft): post-review skipped" S post-review "$RUN" PR1 1; has "  no" "posted=no"

echo "# ready (stop #2)"
st building reviewed reviewed
expect 1 "ready before the ship approval" S ready "$RUN" PR2
has "  says so" "ship approval"
expect 1 "ship-pr refused in github mode" S ship-pr "$RUN" PR2
st shipping reviewed reviewed 'doc["shipApproval"] = {"by": "user", "at": "2026-10-24T01:00:00Z"}'
expect 1 "ship-pr refused in github mode (shipping)" S ship-pr "$RUN" PR2; has "  says use ready" "state.sh ready"
expect 0 "ready PR2" S ready "$RUN" PR2
has "  opened" "PR2 opened: https://github.com/shim/repo/pull/1"
is "  state opened" "$RUN/status.json" prs.1.state '"opened"'
is "  draft false" "$RUN/status.json" prs.1.draft false
python3 -c 'import json,sys; p=json.load(open(sys.argv[1]))[0]; sys.exit(0 if not p["isDraft"] and "Eagle-Eye review rounds" in p["body"] and "#issuecomment-1001" in p["body"] else 1)' "$GH_SHIM_DIR/prs.json" \
  && ok "  marked ready with the final body (review round links)" || bad "  shim: $(cat "$GH_SHIM_DIR/prs.json")"
expect 0 "ready PR2 again" S ready "$RUN" PR2; has "  already" "PR2 already opened"
expect 0 "ready PR1 (no remote)" S ready "$RUN" PR1; is "  local-only" "$RUN/status.json" prs.0.state '"local-only"'

echo "# ready without a draft (draft failed earlier): opens a ready PR"
RUN3="$d/runs/r-h"; mkdir -p "$RUN3"; cp "$RUN/request.md" "$RUN/plan.md" "$RUN/meta.json" "$RUN3/"
RUN_SAVE="$RUN"; RUN="$RUN3"
newwt() { # <pr2 worktree commit message>: make PR2's worktree in $RUN through state.sh, one commit
  st building pending pending 'for p in doc["prs"]: p["commits"] = 0'
  DCV2_RUN_DIR="$RUN" bash "$STATE" worktree "$RUN" "$RR" PR2 "$RBASE" >/dev/null
  git -C "$WS/rr-worktrees/dcv2-$(basename "$RUN")-pr2" commit -q --allow-empty -m "$1"
}
newwt "S2: y"
st shipping reviewed reviewed 'doc["shipApproval"] = {"by": "user", "at": "2026-10-24T01:00:00Z"}'
expect 0 "ready with no draft opens the PR" env DCV2_RUN_DIR="$RUN3" bash "$STATE" ready "$RUN3" PR2
is "  opened" "$RUN3/status.json" prs.1.state '"opened"'
python3 -c 'import json,sys; p=json.load(open(sys.argv[1]))[1]; sys.exit(0 if not p["isDraft"] else 1)' "$GH_SHIM_DIR/prs.json" && ok "  not a draft" || bad "  shim: $(cat "$GH_SHIM_DIR/prs.json")"
RUN="$d/runs/r-k"; mkdir -p "$RUN"; cp "$RUN3/request.md" "$RUN3/plan.md" "$RUN3/meta.json" "$RUN/"
newwt "S2: z"
st building pending needs-attention 'doc["prs"][1]["note"] = "x"'
expect 1 "draft-pr refused for a needs-attention PR" env DCV2_RUN_DIR="$RUN" bash "$STATE" draft-pr "$RUN" PR2
st building pending reviewed
expect 0 "a late draft-pr (PR already reviewed) still opens the draft" env DCV2_RUN_DIR="$RUN" bash "$STATE" draft-pr "$RUN" PR2
has "  opened" "draft=opened"
is "  state stays reviewed" "$RUN/status.json" prs.1.state '"reviewed"'
is "  draft true" "$RUN/status.json" prs.1.draft true
RUN="$RUN_SAVE"

echo "# writeback: a PR left as a draft does not count as opened"
cat > "$RUN/tickets.json" <<'EOF'
{"tasks": [{"id": "t2", "custom_id": "T-2", "name": "prefix", "url": "https://x/t2"}]}
EOF
printf '\n### S2: Prefix results\n- Tickets: t2\n## Tickets\n- t2 | T-2 | prefix | https://x/t2 | stories: S2 | ship status: done\n' >> "$RUN/plan.md"
python3 - "$RUN/status.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1])); s["prs"][1]["state"] = "local-only"; s["prs"][1]["note"] = "left out by the user: stays a draft"; json.dump(s, open(sys.argv[1], "w"))
PY
expect 0 "writeback" S writeback "$RUN"
is "  no status for a draft" "$RUN/writeback.json" drafts.0.status null
is "  url still listed" "$RUN/writeback.json" drafts.0.prs.0.url '"https://github.com/shim/repo/pull/1"'
grep -q '"opened"' "$RUN/writeback.json" && bad "  internal key leaked" || ok "  no internal keys"

echo "# gate reads the comment-friendly first line"
printf 'Eagle-Eye review — round 1: APPROVED — 0 MEDIUM+, 1 LOW\n\n## Findings\n1. LOW — a.js:1 — x — y\n' > "$RUN/reviews/PR1-round1.md"
st building reviewing pending
expect 0 "gate PASS from the first line" S gate "$RUN" PR1 PASS "clean"

echo "# review fixes: forged prUrl, foreign PRs, recorded remote, hooks, copied files, mentions, lost records"
RUN="$d/runs/r-m"; mkdir -p "$RUN"; cp "$RUN3/request.md" "$RUN3/plan.md" "$RUN3/meta.json" "$RUN_SAVE/repos.json" "$RUN/"
export DCV2_RUN_DIR="$RUN"
newwt "S2: m"
WTM="$WS/rr-worktrees/dcv2-r-m-pr2"
is "  remote recorded at worktree time" "$RUN/.guard.json" remotes.PR2.repo '"shim/repo"'
st building pending building
python3 -c 'import json,sys; s=json.load(open(sys.argv[1])); s["prs"][1]["prUrl"]="https://github.com/shim/repo/pull/1"; print(json.dumps(s))' "$RUN/status.json" > "$d/doc.json"
expect 0 "put cannot set a prUrl (HIGH 1)" S put "$RUN/status.json" < "$d/doc.json"
has "  says so" "prUrl is set only by state.sh"
is "  prUrl stays null" "$RUN/status.json" prs.1.prUrl null
# The repo's origin is re-pointed after the worktree was made, and a pre-push hook exists: neither is used.
git init -q --bare -b main "$d/evil.git"
git -C "$RR" remote set-url origin "$d/evil.git"
mkdir -p "$RR/.git/hooks"
printf '#!/bin/sh\ntouch "%s/hook-ran"\n' "$d" > "$RR/.git/hooks/pre-push"; chmod +x "$RR/.git/hooks/pre-push"
expect 0 "draft-pr pushes to the recorded URL" S draft-pr "$RUN" PR2
has "  opened" "draft=opened"
git -C "$d/origin.git" show-ref -q --verify refs/heads/dcv2/r-m-pr2 && ok "  on the recorded remote" || bad "  not on the recorded remote"
git -C "$d/evil.git" show-ref -q --verify refs/heads/dcv2/r-m-pr2 && bad "  pushed to the re-pointed remote" || ok "  not on the re-pointed remote"
[[ ! -e "$d/hook-ran" ]] && ok "  no push hook ran" || bad "  the pre-push hook ran"
git -C "$RR" remote set-url origin "$d/origin.git"
grep -q -- "-R shim/repo" "$GH_SHIM_DIR/calls.log" && ok "  gh calls name the repo (-R)" || bad "  no -R"
mkdir -p "$RUN/reviews"; MURL="$(field "$RUN/status.json" prs.1.prUrl | tr -d '"')"
printf 'Eagle-Eye review — round 1: REQUEST CHANGES — 1 MEDIUM+, 0 LOW\n\ncc @octocat, see other/repo#12 and #3; mail a@b.com\n' > "$RUN/reviews/PR2-round1.md"
st building pending reviewing
expect 0 "post-review neutralises mentions and cross-repo refs" S post-review "$RUN" PR2 1
f="$RUN/pr-comments/PR2-round1.md"
python3 - "$f" <<'PY' && ok "  @octocat and other/repo#12 neutralised, #3 and a@b.com kept" || bad "  text: $(cat "$f")"
import sys; t = open(sys.argv[1]).read()
sys.exit(0 if "@‍octocat" in t and "other/repo#‍12" in t and " #3" in t and "a@b.com" in t and "@octocat" not in t else 1)
PY
# LOW 10: the record of a posted round is lost; the comment on GitHub (its footer) is found, not posted twice.
python3 - "$RUN/status.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1])); s["prs"][1]["comments"] = []; json.dump(s, open(sys.argv[1], "w"))
PY
n0="$(grep -c '^pr comment' "$GH_SHIM_DIR/calls.log")"
expect 0 "post-review after a lost record" S post-review "$RUN" PR2 1
has "  found" "posted=found"
[[ "$(grep -c '^pr comment' "$GH_SHIM_DIR/calls.log")" == "$n0" ]] && ok "  not posted twice" || bad "  posted again"
# HIGH 1: a prUrl that is not this branch's PR (written past state.sh) is refused before any write.
python3 - "$GH_SHIM_DIR/prs.json" <<'PY'
import json, sys
p = json.load(open(sys.argv[1]))
p.append({"url": "https://github.com/shim/repo/pull/%d" % (len(p) + 1), "head": "someone-elses-branch", "isDraft": False, "state": "OPEN", "body": ""})
p.append({"url": "https://github.com/shim/repo/pull/%d" % (len(p) + 1), "head": "dcv2/r-m-pr2", "headRepo": "fork/repo", "isDraft": True, "state": "OPEN", "body": ""})
json.dump(p, open(sys.argv[1], "w"))
PY
NP="$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))))' "$GH_SHIM_DIR/prs.json")"
forge() { python3 - "$RUN/status.json" "$1" <<'PY'
import json, sys
s = json.load(open(sys.argv[1])); s["prs"][1]["prUrl"] = sys.argv[2]; s["prs"][1]["comments"] = []; json.dump(s, open(sys.argv[1], "w"))
PY
}
n0="$(grep -c '^pr comment' "$GH_SHIM_DIR/calls.log")"
printf 'Eagle-Eye review — round 2: APPROVED — 0 MEDIUM+, 0 LOW\n' > "$RUN/reviews/PR2-round2.md"
forge "https://github.com/shim/repo/pull/$((NP - 1))"
expect 1 "post-review to another branch's PR refused" S post-review "$RUN" PR2 2; has "  says why" "is not this run's PR"
forge "https://github.com/shim/repo/pull/$NP"
expect 1 "post-review to a fork's PR with the same branch name refused" S post-review "$RUN" PR2 2
forge "https://github.com/other/repo/pull/1"
expect 1 "post-review to another repo's PR refused" S post-review "$RUN" PR2 2; has "  says why" "is not a pull request of shim/repo"
[[ "$(grep -c '^pr comment' "$GH_SHIM_DIR/calls.log")" == "$n0" ]] && ok "  nothing posted" || bad "  posted to a foreign PR"
st shipping pending reviewed 'doc["shipApproval"] = {"by": "user", "at": "2026-10-24T01:00:00Z"}'
forge "https://github.com/shim/repo/pull/$((NP - 1))"
expect 0 "ready on a foreign prUrl" S ready "$RUN" PR2
is "  needs attention, not readied" "$RUN/status.json" prs.1.state '"needs-attention"'
grep -q "^pr \(ready\|edit\) https://github.com/shim/repo/pull/$((NP - 1))" "$GH_SHIM_DIR/calls.log" && bad "  touched the foreign PR" || ok "  foreign PR untouched"
forge "$MURL"
# MED 6: a copied file committed on the branch blocks every push.
cp "$WTM/.env" "$d/env.bak"; git -C "$WTM" add -f .env; git -C "$WTM" commit -q -m "oops"
st building pending fixing 'doc["prs"][1]["review"]["round"] = 1'
expect 3 "push refused when the branch commits a copied file" S push "$RUN" PR2
has "  names it" ".env, copied in from the main checkout"
is "  needs attention" "$RUN/status.json" prs.1.state '"needs-attention"'
[[ "$(git -C "$d/origin.git" rev-parse dcv2/r-m-pr2)" != "$(git -C "$WTM" rev-parse HEAD)" ]] && ok "  not pushed" || bad "  pushed"
git -C "$WTM" reset -q --hard HEAD~1; cp "$d/env.bak" "$WTM/.env"
git -C "$WTM" add -f .env; git -C "$WTM" commit -q -m "add env"; git -C "$WTM" rm -q --cached .env; git -C "$WTM" commit -q -m "drop env"
st building pending fixing 'doc["prs"][1]["review"]["round"] = 1'
python3 - "$RUN/status.json" "$(git -C "$WTM" rev-parse HEAD)" <<'PY'
import json, sys
s = json.load(open(sys.argv[1])); s["prs"][1]["baseCommit"] = sys.argv[2]; json.dump(s, open(sys.argv[1], "w"))
PY
expect 3 "push refused when a copied file was added and later deleted (history; status.json's base ignored)" S push "$RUN" PR2
git -C "$WTM" reset -q --hard HEAD~2; cp "$d/env.bak" "$WTM/.env"
is "  base recorded in .guard.json" "$RUN/.guard.json" bases.PR2 "\"$RBASE\""
st building pending reviewing
forge "https://evil.example/shim/repo/pull/1"
expect 1 "a PR URL on another host refused" S post-review "$RUN" PR2 2; has "  says why" "is not a pull request of shim/repo on github.com"
forge "$MURL"
# A comment that only quotes the footer mid-text is not taken as the posted round.
python3 - "$GH_SHIM_DIR/prs.json" "$MURL" <<'PY'
import json, sys
p = json.load(open(sys.argv[1]))
pr = next(x for x in p if x["url"] == sys.argv[2])
pr.setdefault("comments", []).append({"url": sys.argv[2] + "#issuecomment-9", "body": "quoting _Eagle-Eye run r-m, review round 2._ here, then more text"})
json.dump(p, open(sys.argv[1], "w"))
PY
expect 0 "a footer quoted mid-comment does not count as posted" S post-review "$RUN" PR2 2; has "  posted" "posted=ok"
# A worktree recorded before 0.3.0 (no remotes entry): the first GitHub command records the current one.
python3 - "$RUN/.guard.json" <<'PY'
import json, sys
g = json.load(open(sys.argv[1])); g.pop("remotes"); json.dump(g, open(sys.argv[1], "w"))
PY
expect 0 "push with no recorded remote (pre-0.3.0 worktree)" S push "$RUN" PR2
is "  remote recorded now" "$RUN/.guard.json" remotes.PR2.repo '"shim/repo"'
# MED 8: a secret in the PR title is never sent.
RUN="$d/runs/r-n"; mkdir -p "$RUN"; cp "$RUN3/request.md" "$RUN3/plan.md" "$RUN3/meta.json" "$RUN/"; export DCV2_RUN_DIR="$RUN"
newwt "S2: n"
st building pending building 'doc["prs"][1]["title"] = "Fix token=ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789"'
n0="$(grep -c '^pr create' "$GH_SHIM_DIR/calls.log")"
expect 3 "draft-pr with a secret in the title" S draft-pr "$RUN" PR2
has "  says why" "PR title/body not sent"; hasnt "  no value" "AbCdEfGh"
[[ "$(grep -c '^pr create' "$GH_SHIM_DIR/calls.log")" == "$n0" ]] && ok "  no PR created" || bad "  created"
git -C "$d/origin.git" show-ref -q --verify refs/heads/dcv2/r-n-pr2 && bad "  pushed anyway" || ok "  nothing pushed"
# Old runs (meta.json without reviewMode): no detected setup, only an explicit one.
RUN="$d/runs/r-o"; mkdir -p "$RUN"; cp "$RUN3/request.md" "$RUN3/plan.md" "$RUN/"; echo '{"schema": 1}' > "$RUN/meta.json"; export DCV2_RUN_DIR="$RUN"
python3 - "$RUN/repos.json" "$R" <<'PY'
import json, sys
json.dump({"schema": 1, "repos": [{"name": "r", "path": sys.argv[2], "check": "true"}]}, open(sys.argv[1], "w"))
PY
st building pending pending 'for p in doc["prs"]: p["commits"] = 0'
expect 0 "old run: worktree with a package-lock.json" S worktree "$RUN" "$R" PR1 "$BASE"
has "  no detected setup" "setup=none"

echo "# memory.sh scan"
out="$(printf 'ok\n' | bash "$ROOT/plugin/scripts/memory.sh" scan)"; has "clean text" "clean"
got=0; out="$(printf 'x\npassword = "hunter2hunter2"\n' | bash "$ROOT/plugin/scripts/memory.sh" scan)" || got=$?
[[ $got == 1 ]] && ok "a secret: exit 1" || bad "exit $got"; has "  line number" "line 2"; hasnt "  no value" "hunter2"

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
