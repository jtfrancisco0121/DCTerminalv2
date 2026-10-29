#!/usr/bin/env bash
# Smoke test: each standalone role skill loads, reads its role file and replies.
#
#   scripts/skill-e2e/standalone.sh                 # all six
#   scripts/skill-e2e/standalone.sh planner developer
#
# Headless, cheap (DCV2_E2E_MODEL, default haiku), against one toy repo in a temp
# dir. Read-only roles must leave the repo untouched.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

ROLES=("$@")
[[ ${#ROLES[@]} -gt 0 ]] || ROLES=(planner plan-reviewer risk-reviewer developer pr-reviewer recommendation codebase-audit)

make_tmp
PLUGIN="$TMP/plugin"
make_test_plugin "$PLUGIN"
WS="$TMP/ws"
make_workspace "$WS"
REPO="$WS/mathlib"
# A branch with a change, for the PR Reviewer.
git -C "$REPO" switch -q -c fix-multiply
sed -i.bak 's/function multiply(a, b) { return a + b; }/function multiply(a, b) { return a * b; }/' "$REPO/index.js" && rm -f "$REPO/index.js.bak"
git -C "$REPO" commit -q -am "multiply() multiplies"
cat > "$TMP/plan.md" <<'EOF'
# Plan: fix multiply
## Stories
### S1: multiply returns a * b
- Repo: mathlib
- Changes: index.js multiply returns a * b.
- Acceptance criteria:
  - [ ] multiply(2, 3) === 6
## Pull requests
### PR1: Fix multiply
- Repo: mathlib
- Stories: S1
- Depends on: none
- Check: npm run check
EOF

request_for() {
  case "$1" in
    planner)        echo "Plan this, very briefly (one story): add a subtract(a, b) function to index.js with a test." ;;
    plan-reviewer)  echo "Review the plan in $TMP/plan.md, very briefly. The request was: multiply(a, b) must return a * b." ;;
    risk-reviewer)  echo "Review the risk of the plan in $TMP/plan.md, very briefly. The request was: multiply(a, b) must return a * b." ;;
    developer)      echo "In at most three lines, tell me what this repo does and what its check command is. Do not change anything." ;;
    pr-reviewer)    echo "Review the current branch fix-multiply against main, very briefly. The intent: multiply(a, b) must return a * b. Do not post anything." ;;
    recommendation) echo "Very briefly: recommend at most one feature for this tiny library." ;;
    codebase-audit) echo "Very briefly: audit index.js only, report at most two findings." ;;
  esac
}

status=0
for role in "${ROLES[@]}"; do
  note "SMOKE: /dcv2:$role"
  head_before="$(git -C "$REPO" rev-parse HEAD)"
  out="$TMP/standalone-$role.jsonl"
  run_claude "$REPO" "$PLUGIN" "$out" "/dcv2:$role $(request_for "$role")"
  if python3 - "$out" "$role" "$REPO" "$head_before" <<'PY'
import json, subprocess, sys
out, role, repo, head_before = sys.argv[1:5]
events = [json.loads(l) for l in open(out) if l.startswith("{")]
errs = []
results = [e for e in events if e.get("type") == "result"]
res = results[-1] if results else {}
if not res or res.get("is_error"):
    errs.append(f"run ended with an error: {res.get('subtype')}")
text = str(res.get("result") or "")
if len(text.strip()) < 20:
    errs.append(f"reply too short: {text!r}")
init = next((e for e in events if e.get("type") == "system" and e.get("subtype") == "init"), {})
if f"dcv2:{role}" not in (init.get("slash_commands") or []):
    errs.append(f"dcv2:{role} not among the session's slash commands")
read_role = any(
    b.get("type") == "tool_use" and b.get("name") == "Read"
    and str((b.get("input") or {}).get("file_path", "")).endswith(f"/roles/{role}.md")
    for e in events if e.get("type") == "assistant"
    for b in (e.get("message") or {}).get("content") or [] if isinstance(b, dict))
if not read_role:
    errs.append(f"the session never read roles/{role}.md")
if role != "developer":
    st = subprocess.run(["git", "-C", repo, "status", "--porcelain"], capture_output=True, text=True).stdout.strip()
    head = subprocess.run(["git", "-C", repo, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    if st or head != head_before:
        errs.append(f"read-only role changed the repo: {st[:200]} {head[:8]}")
print("    reply: " + text.strip().replace("\n", " ")[:300])
if errs:
    print("    FAILED: " + "; ".join(errs)); sys.exit(1)
print("    ok")
PY
  then :; else status=1; fi
  # Leave the repo as the next role expects it.
  git -C "$REPO" checkout -q -- . 2>/dev/null || true
  git -C "$REPO" clean -qfd 2>/dev/null || true
done

if [[ $status == 0 ]]; then note "PASS (artifacts in $TMP)"; else note "FAILED (artifacts in $TMP)"; fi
exit $status
