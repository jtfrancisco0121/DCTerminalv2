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
- Files: multiply.sh
### S2: Prefix results
- Files: prefix.sh
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
grep -q '^Size:' "$RUN/pr-bodies/PR2.md" && bad "  body has a Size line" || ok "  body has no Size line"
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
python3 -c 'import json,sys; p=json.load(open(sys.argv[1]))[0]; sys.exit(0 if "Size:" not in p["body"] else 1)' "$GH_SHIM_DIR/prs.json" \
  && ok "  the final body has no Size line" || bad "  shim: $(cat "$GH_SHIM_DIR/prs.json")"
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
printf '\n### S2: Prefix results\n- Tickets: t2\n- Files: prefix.sh\n## Tickets\n- t2 | T-2 | prefix | https://x/t2 | stories: S2 | ship status: done\n' >> "$RUN/plan.md"
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

echo "# two PRs of one repo (disjoint Files): two worktrees, two branches, no size line in the bodies, two opened PRs, cleanup after both merge"
TW="$WS/tw"; mkdir -p "$TW"
git -C "$TW" init -q -b main; git -C "$TW" config user.name t; git -C "$TW" config user.email t@t
echo base > "$TW/base.txt"; git -C "$TW" add -A; git -C "$TW" commit -q -m init
git init -q --bare -b main "$d/origin-tw.git"; git -C "$TW" remote add origin "$d/origin-tw.git"; git -C "$TW" push -q origin main
TWB="$(git -C "$TW" rev-parse HEAD)"
RUN="$d/runs/r-two"; mkdir -p "$RUN"; cp "$RUN3/request.md" "$RUN/"; echo '{"schema": 1}' > "$RUN/meta.json"
printf '# Plan\n## Stories\n### S1: Big part\n- Files: a.txt\n### S2: Small part\n- Files: b.txt\n## Pull requests\n### PR1: Big\n- Repo: tw\n- Stories: S1\n### PR2: Small\n- Repo: tw\n- Stories: S2\n' > "$RUN/plan.md"
printf '{"schema":1,"repos":[{"name":"tw","path":"%s","check":"true"}]}\n' "$TW" > "$RUN/repos.json"
export DCV2_RUN_DIR="$RUN"
# twst <stage> <pr1 state> <pr2 state> [python]: both PRs are in the one repo tw; the GitHub fields of the old file are kept
twst() {
  python3 - "$RUN" "$TW" "$TWB" "$@" <<'PY'
import json, os, sys
run, tw, base, stage, s1, s2 = sys.argv[1:7]
extra = sys.argv[7] if len(sys.argv) > 7 else ""
rid = os.path.basename(run)
old = {}
try:
    old = {p["id"]: p for p in json.load(open(os.path.join(run, "status.json")))["prs"]}
except Exception:
    pass
def pr(i, state):
    lc = f"pr{i}"
    p = {"id": f"PR{i}", "title": f"PR {i} title", "repo": "tw", "repoPath": tw, "branch": f"dcv2/{rid}-{lc}", "base": "main", "baseCommit": base,
         "worktree": os.path.join(os.path.dirname(tw), "tw-worktrees", f"dcv2-{rid}-{lc}"), "dependsOn": [], "stories": {"done": 1, "total": 1, "ids": [f"S{i}"]},
         "commits": 0 if stage == "building" else 1, "check": {"command": "true", "result": "pass", "summary": None},
         "review": {"round": 1, "verdict": "APPROVED", "openFindings": 0, "path": None}, "prUrl": None, "state": state, "note": None}
    for k in ("prUrl", "draft", "comments"):
        if k in old.get(p["id"], {}):
            p[k] = old[p["id"]][k]
    return p
doc = {"schema": 1, "runId": rid, "stage": stage, "waitingFor": None, "folder": os.path.dirname(tw),
       "plan": {"path": "plan.md", "reviewRound": 1, "verdict": "APPROVED", "openFindings": 0, "approved": True, "approval": {"by": "user", "at": "2026-10-24T00:00:00Z"}},
       "prs": [pr(1, s1), pr(2, s2)], "shipApproval": None, "ship": None, "writeback": None, "error": None}
exec(extra)
json.dump(doc, open(os.path.join(run, "status.json"), "w"))
PY
}
twst building pending pending
expect 0 "PR1 worktree (same repo)" S worktree "$RUN" "$TW" PR1 "$TWB"
expect 0 "PR2 worktree (same repo)" S worktree "$RUN" "$TW" PR2 "$TWB"
TW1="$WS/tw-worktrees/dcv2-r-two-pr1"; TW2="$WS/tw-worktrees/dcv2-r-two-pr2"
[[ -d "$TW1" && -d "$TW2" && "$TW1" != "$TW2" ]] && ok "  two worktree folders" || bad "  worktrees: $TW1 $TW2"
[[ "$(git -C "$TW" worktree list --porcelain | grep -c '^branch refs/heads/dcv2/r-two-pr')" == 2 ]] && ok "  two worktrees on two branches" || bad "  $(git -C "$TW" worktree list)"
is "  both recorded in .guard.json" "$RUN/.guard.json" worktrees.PR2 "\"$TW2\""
seq 1 450 > "$TW1/a.txt"; git -C "$TW1" add -A; git -C "$TW1" commit -q -m "S1: big part"
seq 1 5 > "$TW2/b.txt"; git -C "$TW2" add -A; git -C "$TW2" commit -q -m "S2: small part"
mkdir -p "$RUN/reviews"; printf '## Verdict\nAPPROVED\n' > "$RUN/reviews/PR1-round1.md"; printf '## Verdict\nAPPROVED\n' > "$RUN/reviews/PR2-round1.md"
expect 0 "gate PASS for the big PR1 (no size limit)" S gate "$RUN" PR1 PASS "clean"
expect 0 "gate PASS for PR2" S gate "$RUN" PR2 PASS "clean"
twst shipping reviewed reviewed 'doc["shipApproval"] = {"by": "user", "at": "2026-10-24T01:00:00Z"}'
expect 0 "ship-pr PR1 (local mode)" S ship-pr "$RUN" PR1; has "  opened" "PR1 opened: https://github.com/shim/repo/pull/"
expect 0 "ship-pr PR2 (local mode)" S ship-pr "$RUN" PR2; has "  opened" "PR2 opened: https://github.com/shim/repo/pull/"
python3 -c 'import json,sys; h=[p["head"] for p in json.load(open(sys.argv[1]))]; sys.exit(0 if "dcv2/r-two-pr1" in h and "dcv2/r-two-pr2" in h else 1)' "$GH_SHIM_DIR/prs.json" \
  && ok "  two PRs opened through the shim, one per branch" || bad "  shim: $(cat "$GH_SHIM_DIR/prs.json")"
for f in PR1 PR2; do grep -q '^Size:' "$RUN/pr-bodies/$f.md" && bad "  $f body has a Size line" || ok "  $f body has no Size line"; done
git -C "$TW" merge -q --no-ff -m "merge both" dcv2/r-two-pr1 dcv2/r-two-pr2
twst done opened opened 'doc["ship"] = {"remote": "origin", "pushed": True, "note": "x"}'
expect 0 "cleanup at done after both PRs merged" S cleanup "$RUN"
has "  PR1 worktree removed" "PR1: worktree removed"; has "  PR2 worktree removed" "PR2: worktree removed"
[[ ! -d "$TW1" && ! -d "$TW2" ]] && ok "  both worktree folders are gone" || bad "  worktrees left"
[[ -z "$(git -C "$TW" branch --list 'dcv2/r-two-*')" ]] && ok "  both branches deleted (git branch -d)" || bad "  branches: $(git -C "$TW" branch --list)"

echo "# stacked PRs: ship-check, opened against the parent's branch, retargeted before the parent's branch is deleted"
TWB2="$(git -C "$TW" rev-parse HEAD)"
# skst <stage> <pr1 state> <pr2 state> [python]: PR2 Depends on PR1 in the same repo; stackedOn, base and baseCommit as .guard.json has them
skst() {
  python3 - "$RUN" "$TW" "$TWB2" "$@" <<'PY'
import json, os, sys
run, tw, base, stage, s1, s2 = sys.argv[1:7]
extra = sys.argv[7] if len(sys.argv) > 7 else ""
rid = os.path.basename(run)
old = {}
try:
    old = {p["id"]: p for p in json.load(open(os.path.join(run, "status.json")))["prs"]}
except Exception:
    pass
try:
    g = json.load(open(os.path.join(run, ".guard.json")))
except Exception:
    g = {}
def pr(i, state):
    lc = f"pr{i}"
    p = {"id": f"PR{i}", "title": f"PR {i} title", "repo": "tw", "repoPath": tw, "branch": f"dcv2/{rid}-{lc}", "base": "main", "baseCommit": base,
         "worktree": os.path.join(os.path.dirname(tw), "tw-worktrees", f"dcv2-{rid}-{lc}"), "dependsOn": [] if i == 1 else ["PR1"],
         "stories": {"done": 1, "total": 1, "ids": [f"S{i}"]}, "commits": 0 if stage == "building" else 1, "check": {"command": "true", "result": "pass", "summary": None},
         "review": {"round": 1, "verdict": "APPROVED", "openFindings": 0, "path": None}, "prUrl": None, "state": state, "note": None}
    if i == 2 and (g.get("stacks") or {}).get("PR2"):
        p.update(stackedOn="PR1", base=f"dcv2/{rid}-pr1", baseCommit=g["bases"]["PR2"])
    for k in ("prUrl", "draft", "comments", "restack"):
        if k in old.get(p["id"], {}):
            p[k] = old[p["id"]][k]
    return p
doc = {"schema": 1, "runId": rid, "stage": stage, "waitingFor": None, "folder": os.path.dirname(tw),
       "plan": {"path": "plan.md", "reviewRound": 1, "verdict": "APPROVED", "openFindings": 0, "approved": True, "approval": {"by": "user", "at": "2026-10-24T00:00:00Z"}},
       "prs": [pr(1, s1), pr(2, s2)], "shipApproval": None, "ship": None, "writeback": None, "error": None}
exec(extra)
json.dump(doc, open(os.path.join(run, "status.json"), "w"))
PY
}
SHIP='doc["shipApproval"] = {"by": "user", "at": "2026-10-24T01:00:00Z"}'
RUN="$d/runs/r-stk"; mkdir -p "$RUN"; cp "$RUN3/request.md" "$RUN/"; echo '{"schema": 1}' > "$RUN/meta.json"
printf '# Plan\n## Stories\n### S1: Parent part\n- Files: a.txt\n### S2: Child part\n- Files: a.txt, b.txt\n## Pull requests\n### PR1: Parent\n- Repo: tw\n- Stories: S1\n- Depends on: none\n### PR2: Child\n- Repo: tw\n- Stories: S2\n- Depends on: PR1\n' > "$RUN/plan.md"
printf '{"schema":1,"repos":[{"name":"tw","path":"%s","check":"true"}]}\n' "$TW" > "$RUN/repos.json"
export DCV2_RUN_DIR="$RUN"
skst building pending pending
S worktree "$RUN" "$TW" PR1 "$TWB2" >/dev/null
TWS1="$WS/tw-worktrees/dcv2-r-stk-pr1"; TWS2="$WS/tw-worktrees/dcv2-r-stk-pr2"
echo parent > "$TWS1/a.txt"; git -C "$TWS1" add -A; git -C "$TWS1" commit -q -m "S1: parent part"
skst building reviewed pending
expect 1 "worktree of PR2 (Depends on PR1, same repo) with a commit base" S worktree "$RUN" "$TW" PR2 "$TWB2"
has "  says to pass the parent" "pass PR1 as the base"
expect 0 "worktree PR2 PR1" S worktree "$RUN" "$TW" PR2 PR1; has "  stackedOn" "stackedOn=PR1"
echo child >> "$TWS2/a.txt"; echo child > "$TWS2/b.txt"; git -C "$TWS2" add -A; git -C "$TWS2" commit -q -m "S2: child part"
skst ship-approval reviewed reviewed
expect 0 "ship-check in local mode" S ship-check "$RUN"
has "  PR1: its base is on the remote" "PR1 tw: remote $d/origin-tw.git, base main is on the remote"
has "  PR2: stacked, ships after PR1" "PR2 tw: stacked on PR1 (ships after it); remote $d/origin-tw.git, base main is on the remote"
hasnt "  not needs attention" "needs attention"
skst shipping reviewed reviewed "$SHIP"
expect 1 "ship-pr PR2 before PR1 has a PR" S ship-pr "$RUN" PR2
has "  says why" "stacked on PR1, which has no PR yet"
skst shipping pushed reviewed "$SHIP"
expect 0 "ship-pr PR2 when PR1 ended pushed with no PR: needs-attention, not a refusal" S ship-pr "$RUN" PR2
has "  note names the parent" "PR1 ended pushed with no PR"
is "  PR2 needs-attention" "$RUN/status.json" prs.1.state '"needs-attention"'
skst shipping local-only reviewed "$SHIP"
expect 0 "ship-pr PR2 when PR1 ended local-only" S ship-pr "$RUN" PR2
is "  PR2 needs-attention" "$RUN/status.json" prs.1.state '"needs-attention"'
skst shipping reviewed reviewed "$SHIP"
expect 0 "ship-pr PR1" S ship-pr "$RUN" PR1; has "  opened" "PR1 opened: https://github.com/shim/repo/pull/"
N1="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["prs"][0]["prUrl"].rsplit("/", 1)[1])' "$RUN/status.json")"
expect 0 "ship-pr PR2 (after PR1)" S ship-pr "$RUN" PR2; has "  opened" "PR2 opened: https://github.com/shim/repo/pull/"
grep -q -- "pr create --base dcv2/r-stk-pr1 --head dcv2/r-stk-pr2" "$GH_SHIM_DIR/calls.log" && ok "  PR2 opened against PR1's branch" || bad "  calls: $(tail -5 "$GH_SHIM_DIR/calls.log")"
grep -qxF "Stacked on #$N1. Merge #$N1 first." "$RUN/pr-bodies/PR2.md" && ok "  PR2's body: Stacked on #$N1. Merge #$N1 first." || bad "  body: $(cat "$RUN/pr-bodies/PR2.md")"
grep -q 'Stacked on' "$RUN/pr-bodies/PR1.md" && bad "  PR1's body says stacked" || ok "  PR1's body has no stack line"
! grep -qE -- 'push.*(--force|-f )' "$GH_SHIM_DIR/calls.log" && ok "  no force push" || bad "  force push in the calls"
echo "# epic siblings (local mode): the Epic section, the epic label, the sibling body update; failures are notes"
TX="$WS/tx"; mkdir -p "$TX"
git -C "$TX" init -q -b main; git -C "$TX" config user.name t; git -C "$TX" config user.email t@t
echo base > "$TX/base.txt"; git -C "$TX" add -A; git -C "$TX" commit -q -m init
git init -q --bare -b main "$d/origin-tx.git"; git -C "$TX" remote add origin "$d/origin-tx.git"; git -C "$TX" push -q origin main
TXB="$(git -C "$TX" rev-parse HEAD)"
epst() {
  python3 - "$RUN" "$TW" "$TWB2" "$TX" "$TXB" "$@" <<'PY'
import json, os, sys
run, tw, twb, tx, txb, stage, s1, s2 = sys.argv[1:9]
extra = sys.argv[9] if len(sys.argv) > 9 else ""
rid = os.path.basename(run)
old = {}
try:
    old = {p["id"]: p for p in json.load(open(os.path.join(run, "status.json")))["prs"]}
except Exception:
    pass
def pr(i, repo, path, b, state):
    lc = f"pr{i}"
    p = {"id": f"PR{i}", "title": f"PR {i} title", "repo": repo, "repoPath": path, "branch": f"dcv2/{rid}-{lc}", "base": "main", "baseCommit": b,
         "worktree": os.path.join(os.path.dirname(path), repo + "-worktrees", f"dcv2-{rid}-{lc}"), "dependsOn": [],
         "stories": {"done": 1, "total": 1, "ids": [f"S{i}"]}, "commits": 0 if stage == "building" else 1, "check": {"command": "true", "result": "pass", "summary": None},
         "review": {"round": 1, "verdict": "APPROVED", "openFindings": 0, "path": None}, "prUrl": None, "state": state, "note": None}
    for k in ("prUrl", "draft", "comments", "epic"):
        if k in old.get(p["id"], {}):
            p[k] = old[p["id"]][k]
    return p
doc = {"schema": 1, "runId": rid, "stage": stage, "waitingFor": None, "folder": os.path.dirname(tw),
       "plan": {"path": "plan.md", "reviewRound": 1, "verdict": "APPROVED", "openFindings": 0, "approved": True, "approval": {"by": "user", "at": "2026-10-24T00:00:00Z"}},
       "prs": [pr(1, "tw", tw, twb, s1), pr(2, "tx", tx, txb, s2)], "shipApproval": None, "ship": None, "writeback": None, "error": None}
exec(extra)
json.dump(doc, open(os.path.join(run, "status.json"), "w"))
PY
}
# epsetup <tag> <epic id> [PR2's epic line]: PR1 in tw, PR2 in tx, both in epic <epic id> (or PR2's own Epic line); worktrees
# made, one commit each, stage shipping
epsetup() {
  RUN="$d/runs/r-$1"; EPIC="$2"; EPIC2="${3:-$2 Epic title}"; mkdir -p "$RUN"; cp "$RUN3/request.md" "$RUN/"; echo '{"schema": 1}' > "$RUN/meta.json"
  printf '# Plan\n## Stories\n### S1: Tw part\n- Files: a.txt\n### S2: Tx part\n- Files: b.txt\n## Pull requests\n### PR1: Tw\n- Repo: tw\n- Stories: S1\n- Depends on: none\n- Epic: %s Epic title\n### PR2: Tx\n- Repo: tx\n- Stories: S2\n- Depends on: none\n- Epic: %s\n' "$EPIC" "$EPIC2" > "$RUN/plan.md"
  printf '{"schema":1,"repos":[{"name":"tw","path":"%s","check":"true"},{"name":"tx","path":"%s","check":"true"}]}\n' "$TW" "$TX" > "$RUN/repos.json"
  printf '{"schema":1,"tasks":[],"epics":[{"id":"%s","name":"Epic title","url":"https://x"}]}\n' "$EPIC" > "$RUN/tickets.json"
  export DCV2_RUN_DIR="$RUN"
  epst building pending pending
  S worktree "$RUN" "$TW" PR1 "$TWB2" >/dev/null; S worktree "$RUN" "$TX" PR2 "$TXB" >/dev/null
  EW1="$WS/tw-worktrees/dcv2-r-$1-pr1"; EW2="$WS/tx-worktrees/dcv2-r-$1-pr2"
  echo one > "$EW1/e1-$1.txt"; git -C "$EW1" add -A; git -C "$EW1" commit -q -m "S1"
  echo two > "$EW2/e2-$1.txt"; git -C "$EW2" add -A; git -C "$EW2" commit -q -m "S2"
  epst shipping reviewed reviewed "$SHIP"
}
prfield() { python3 -c 'import json,sys; p=[x for x in json.load(open(sys.argv[1])) if x["head"]==sys.argv[2]][0]; print(p.get(sys.argv[3]) if sys.argv[3] != "labels" else ",".join(p.get("labels", [])))' "$GH_SHIM_DIR/prs.json" "$1" "$2"; }
epsetup epa E1
expect 0 "ship-pr PR1 of a 2-PR epic" S ship-pr "$RUN" PR1; has "  opened" "PR1 opened:"
grep -qx '## Epic' "$RUN/pr-bodies/PR1.md" && grep -qF 'Epic E1: Epic title (2 PRs)' "$RUN/pr-bodies/PR1.md" && grep -qF -- '- tx: PR2 (not opened yet)' "$RUN/pr-bodies/PR1.md" && ok "  PR1's body: the Epic section lists PR2, not opened yet" || bad "  body: $(cat "$RUN/pr-bodies/PR1.md")"
[[ "$(prfield dcv2/r-epa-pr1 labels)" == "epic-E1" ]] && ok "  PR1 has the epic-E1 label" || bad "  labels: $(prfield dcv2/r-epa-pr1 labels)"
grep -q 'epic-E1' "$GH_SHIM_DIR/labels.json" && ok "  the missing label was created" || bad "  labels.json: $(cat "$GH_SHIM_DIR/labels.json" 2>&1)"
expect 0 "ship-pr PR2" S ship-pr "$RUN" PR2; has "  opened" "PR2 opened:"
U1="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["prs"][0]["prUrl"])' "$RUN/status.json")"
U2="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["prs"][1]["prUrl"])' "$RUN/status.json")"
grep -qF -- "- tw: $U1" "$RUN/pr-bodies/PR2.md" && ok "  PR2's body links PR1" || bad "  body: $(cat "$RUN/pr-bodies/PR2.md")"
prfield dcv2/r-epa-pr1 body | grep -qF -- "- tx: $U2" && ok "  PR1's open body was updated with PR2's link" || bad "  PR1 body: $(prfield dcv2/r-epa-pr1 body)"
[[ "$(prfield dcv2/r-epa-pr2 labels)" == "epic-E1" && "$(grep -c 'label create epic-E1' "$GH_SHIM_DIR/calls.log")" == 1 ]] && ok "  PR2 has the label; the label was created once" || bad "  labels: $(prfield dcv2/r-epa-pr2 labels)"
echo "# epic: a label failure and a secret in a sibling body are notes, not state changes"
epsetup epb E2
GH_SHIM_FAIL=label:create bash "$STATE" ship-pr "$RUN" PR1 >/dev/null
printf '# Plan\n## Stories\n### S1: password = "hunter2hunter2"\n- Files: a.txt\n### S2: Tx part\n- Files: b.txt\n## Pull requests\n### PR1: Tw\n- Repo: tw\n- Stories: S1\n- Depends on: none\n- Epic: E2 Epic title\n### PR2: Tx\n- Repo: tx\n- Stories: S2\n- Depends on: none\n- Epic: E2 Epic title\n' > "$RUN/plan.md"
expect 0 "ship-pr PR2 while the label cannot be created and PR1's new body would hold a secret" env GH_SHIM_FAIL=label:create bash "$STATE" ship-pr "$RUN" PR2
has "  opened" "PR2 opened:"
has "  a note about the label" "note: "
is "  PR2 is opened (state unchanged by the failures)" "$RUN/status.json" prs.1.state '"opened"'
is "  PR1 is still opened" "$RUN/status.json" prs.0.state '"opened"'
has "  PR1's body was not updated (secret)" "PR1's body not updated with the epic links"
prfield dcv2/r-epb-pr1 body | grep -q hunter2 && bad "  the secret reached PR1's body" || ok "  no secret was sent"
[[ -z "$(prfield dcv2/r-epb-pr2 labels)" ]] && ok "  PR2 has no label" || bad "  labels: $(prfield dcv2/r-epb-pr2 labels)"
echo "# epic: a PR that is its epic's only PR still gets the epic label (no Epic section, no sibling update)"
epsetup epc E3 none
expect 0 "ship-pr PR1, the only PR of epic E3" S ship-pr "$RUN" PR1; has "  opened" "PR1 opened:"
[[ "$(prfield dcv2/r-epc-pr1 labels)" == "epic-E3" ]] && ok "  PR1 has the epic-E3 label" || bad "  labels: $(prfield dcv2/r-epc-pr1 labels)"
grep -qx '## Epic' "$RUN/pr-bodies/PR1.md" && bad "  a single-PR epic got an Epic section" || ok "  no Epic section in PR1's body"
expect 0 "ship-pr PR2, which has no epic" S ship-pr "$RUN" PR2; has "  opened" "PR2 opened:"
[[ -z "$(prfield dcv2/r-epc-pr2 labels)" ]] && ok "  PR2 (no epic) has no label" || bad "  labels: $(prfield dcv2/r-epc-pr2 labels)"
export DCV2_RUN_DIR="$RUN3"

echo "# GitHub mode: a stacked PR is drafted against its parent; a review comment after a restack is posted again"
RUN="$d/runs/r-sg"; mkdir -p "$RUN"; cp "$RUN3/request.md" "$RUN/"; echo '{"schema": 1, "reviewMode": "github"}' > "$RUN/meta.json"
TWB2="$(git -C "$TW" rev-parse HEAD)"
printf '# Plan\n## Stories\n### S1: Parent part\n- Files: a.txt\n### S2: Child part\n- Files: a.txt, b.txt\n## Pull requests\n### PR1: Parent\n- Repo: tw\n- Stories: S1\n- Depends on: none\n### PR2: Child\n- Repo: tw\n- Stories: S2\n- Depends on: PR1\n' > "$RUN/plan.md"
printf '{"schema":1,"repos":[{"name":"tw","path":"%s","check":"true"}]}\n' "$TW" > "$RUN/repos.json"
export DCV2_RUN_DIR="$RUN"
skst building pending pending
S worktree "$RUN" "$TW" PR1 "$TWB2" >/dev/null
TWS1="$WS/tw-worktrees/dcv2-r-sg-pr1"; TWS2="$WS/tw-worktrees/dcv2-r-sg-pr2"
echo parent > "$TWS1/a.txt"; git -C "$TWS1" add -A; git -C "$TWS1" commit -q -m "S1"
skst building reviewed pending
S worktree "$RUN" "$TW" PR2 PR1 >/dev/null
echo child > "$TWS2/b.txt"; git -C "$TWS2" add -A; git -C "$TWS2" commit -q -m "S2"
skst building reviewed reviewing
expect 1 "draft-pr PR2 before PR1 has a PR" S draft-pr "$RUN" PR2
has "  says why" "stacked on PR1, which has no PR yet"
expect 0 "draft-pr PR1" S draft-pr "$RUN" PR1; has "  opened" "draft=opened"
expect 0 "draft-pr PR2" S draft-pr "$RUN" PR2; has "  opened" "draft=opened"
grep -q -- "pr create --draft --base dcv2/r-sg-pr1 --head dcv2/r-sg-pr2" "$GH_SHIM_DIR/calls.log" && ok "  PR2's draft is against PR1's branch" || bad "  calls: $(tail -4 "$GH_SHIM_DIR/calls.log")"
grep -q 'Stacked on #' "$RUN/pr-bodies/PR2.md" && ok "  the draft body says Stacked on" || bad "  body: $(cat "$RUN/pr-bodies/PR2.md")"
mkdir -p "$RUN/reviews"; printf 'Eagle-Eye review \xe2\x80\x94 round 1: APPROVED \xe2\x80\x94 first\n' > "$RUN/reviews/PR2-round1.md"
expect 0 "post-review PR2 round 1" S post-review "$RUN" PR2 1; has "  posted" "posted=ok"
echo more > "$TWS1/more.txt"; git -C "$TWS1" add -A; git -C "$TWS1" commit -q -m "PR1 fix"
skst building reviewed reviewing
expect 0 "restack PR2" S restack "$RUN" PR2; has "  restack 1" "restack=1"
printf 'Eagle-Eye review \xe2\x80\x94 round 1: APPROVED \xe2\x80\x94 after the restack\n' > "$RUN/reviews/PR2-round1.md"
expect 0 "post-review PR2 round 1 after the restack posts a new comment" S post-review "$RUN" PR2 1
has "  posted, not skipped" "posted=ok"
C2="$(ls "$GH_SHIM_DIR/comments" | grep -E "^$(python3 -c 'import json,sys; print([x for x in json.load(open(sys.argv[1])) if x["head"]=="dcv2/r-sg-pr2"][0]["url"].rsplit("/", 1)[1])' "$GH_SHIM_DIR/prs.json")-" | sort | tail -1)"
grep -qF 'review round 1 (after restack 1)._' "$GH_SHIM_DIR/comments/$C2" && ok "  the new comment has the restack footer" || bad "  comments: $(ls "$GH_SHIM_DIR/comments")"
is "  the comment is recorded with restack 1" "$RUN/status.json" prs.1.comments.1.restack 1
expect 0 "post-review PR2 round 1 again: already posted" S post-review "$RUN" PR2 1; has "  already" "posted=already"
echo "# GitHub mode: ready opens the stack in order; cleanup retargets the child before the parent's branch is deleted"
skst shipping reviewed reviewed "$SHIP"
expect 0 "ready PR1" S ready "$RUN" PR1; has "  opened" "PR1 opened:"
expect 0 "ready PR2 (stacked)" S ready "$RUN" PR2; has "  opened" "PR2 opened:"
grep -q 'Stacked on #' "$RUN/pr-bodies/PR2.md" && ok "  PR2's final body says Stacked on" || bad "  body: $(cat "$RUN/pr-bodies/PR2.md")"
echo "# the parent PR is closed without merge: nothing is retargeted, its branch is kept, the stacked PR gets a note"
skst done opened opened 'doc["ship"] = {"remote": "origin", "pushed": True, "note": "x"}'
setpr() { python3 - "$GH_SHIM_DIR/prs.json" "$1" "$2" <<'PY'
import json, sys
prs = json.load(open(sys.argv[1]))
for p in prs:
    if p["head"] == sys.argv[2]:
        exec(sys.argv[3])
json.dump(prs, open(sys.argv[1], "w"))
PY
}
setpr dcv2/r-sg-pr1 'p["state"] = "CLOSED"'
expect 0 "cleanup with PR1 closed without merge" S cleanup "$RUN"
has "  closed, not merged" "remote branch kept (not merged (closed))"
has "  PR2 gets a note" "PR2: note: stacked on PR1, which was closed without merge"
git -C "$d/origin-tw.git" show-ref --verify -q refs/heads/dcv2/r-sg-pr1 && ok "  PR1's remote branch is still there" || bad "  PR1's remote branch is gone"
python3 -c 'import json,sys; p=[x for x in json.load(open(sys.argv[1])) if x["head"]=="dcv2/r-sg-pr2"][0]; sys.exit(0 if p["base"]=="dcv2/r-sg-pr1" else 1)' "$GH_SHIM_DIR/prs.json" && ok "  PR2's base is unchanged" || bad "  PR2 retargeted"
echo "# the parent PR is merged: the retarget fails, so its branch is kept; then it works, and the branch goes after it"
TIP1="$(git -C "$d/origin-tw.git" rev-parse refs/heads/dcv2/r-sg-pr1)"
setpr dcv2/r-sg-pr1 "p['state'] = 'MERGED'; p['headRefOid'] = '$TIP1'; p['mergeCommit'] = {'oid': '$TIP1'}"
expect 0 "cleanup while gh pr edit --base fails" env GH_SHIM_FAIL=edit bash "$STATE" cleanup "$RUN"
has "  the branch is kept and the reason is given" "remote branch kept (not deleted:"
git -C "$d/origin-tw.git" show-ref --verify -q refs/heads/dcv2/r-sg-pr1 && ok "  PR1's remote branch is kept while PR2 still targets it" || bad "  deleted"
expect 0 "cleanup again" S cleanup "$RUN"
has "  PR1's remote branch deleted" "remote branch deleted"
python3 -c 'import json,sys; p=[x for x in json.load(open(sys.argv[1])) if x["head"]=="dcv2/r-sg-pr2"][0]; sys.exit(0 if p["base"]=="main" else 1)' "$GH_SHIM_DIR/prs.json" && ok "  PR2 was retargeted to the repo base (verified through gh pr view)" || bad "  PR2's base: $(cat "$GH_SHIM_DIR/prs.json")"
git -C "$d/origin-tw.git" show-ref --verify -q refs/heads/dcv2/r-sg-pr1 && bad "  PR1's remote branch still there" || ok "  PR1's remote branch is gone"
[[ "$(grep -c 'pr list --base dcv2/r-sg-pr1' "$GH_SHIM_DIR/calls.log")" -ge 1 && "$(grep -c 'pr view .*--json baseRefName' "$GH_SHIM_DIR/calls.log")" -ge 1 ]] && ok "  gh pr list --base and gh pr view baseRefName were used" || bad "  calls: $(tail -8 "$GH_SHIM_DIR/calls.log")"
expect 0 "a second cleanup" S cleanup "$RUN"; has "  is a no-op for PR1's remote branch" "remote branch already deleted"

export DCV2_RUN_DIR="$RUN3"

echo "# memory.sh scan"
out="$(printf 'ok\n' | bash "$ROOT/plugin/scripts/memory.sh" scan)"; has "clean text" "clean"
got=0; out="$(printf 'x\npassword = "hunter2hunter2"\n' | bash "$ROOT/plugin/scripts/memory.sh" scan)" || got=$?
[[ $got == 1 ]] && ok "a secret: exit 1" || bad "exit $got"; has "  line number" "line 2"; hasnt "  no value" "hunter2"

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
