#!/usr/bin/env bash
# Spike (B1): can the Planner follow the vendored bmad-spec skill headless, from a file path,
# with the runtime wrapper, in a run folder whose path has a space (like Application Support)?
#
#   scripts/skill-e2e/bmad_spike.sh
#
# It makes a toy repo and a run folder (meta.json bmad "on", status.json at planning, then
# `state.sh bmad-init`), launches `claude -p` as the dcv2:planner agent with pass=bmad limited
# to the bmad-spec step, and asserts:
#   - <run>/bmad/output/<run id>/spec-*/spec-*.md exists with an `id: SPEC-` frontmatter;
#   - the spec folder's .memlog.md exists (written through plugin/bmad/runtime.mjs);
#   - the transcript shows the Planner calling the runtime, and no call to uv/npx;
#   - the toy repo's `git status` is clean and it holds no BMAD files.
# Exit 0 = the spike worked; non-zero with a reason on stderr = it did not.
#
# Env as run.sh: DCV2_E2E_MODEL (default haiku), DCV2_E2E_TIMEOUT, DCV2_E2E_TMP. Uses the current
# Claude account (CLAUDE_CONFIG_DIR). Nothing leaves the temp folder.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_tmp
PLUGIN="$TMP/plugin"
make_test_plugin "$PLUGIN"
STATE="$PLUGIN/scripts/state.sh"

WS="$TMP/ws"
RUN="$TMP/App Support/runs/r-spike"
mkdir -p "$WS/toy" "$RUN"
cat > "$WS/toy/package.json" <<'EOF'
{"name": "toy", "version": "1.0.0", "private": true, "scripts": {"check": "node test.js"}}
EOF
cat > "$WS/toy/tally.js" <<'EOF'
// A running tally for the report screen.
function add(total, n) { return total + n; }
module.exports = { add };
EOF
cat > "$WS/toy/test.js" <<'EOF'
const assert = require("assert");
const { add } = require("./tally");
assert.strictEqual(add(1, 2), 3);
console.log("toy: ok");
EOF
git_init_repo "$WS/toy"

{
  echo "folder: $WS"
  echo
  echo "# Request"
  echo
  echo 'toy: add `reset(total)` to tally.js, which returns 0, and make `add` ignore values that are not numbers. One story, one PR.'
} > "$RUN/request.md"
printf '{"schema": 1, "runId": "r-spike", "bmad": {"mode": "on", "version": "spike"}}\n' > "$RUN/meta.json"
printf '{"schema": 1, "repos": [{"name": "toy", "path": "%s", "check": "npm run check"}]}\n' "$WS/toy" > "$RUN/repos.json"

export DCV2_RUN_DIR="$RUN"
bash "$STATE" put "$RUN/status.json" <<'EOF' >/dev/null || fail "could not write the starting status.json"
{"schema": 1, "runId": "r-spike", "stage": "planning", "waitingFor": null, "paused": null, "folder": "-", "plan": null, "prs": [],
 "shipApproval": null, "ship": null, "writeback": null, "error": null}
EOF
bash "$STATE" bmad-init "$RUN" || fail "state.sh bmad-init failed (is node 18+ installed?)"
unset DCV2_RUN_DIR

PROMPT="run=$RUN pass=bmad. repos: name=toy path=$WS/toy check=\"npm run check\". This is a spike: follow ONLY the bmad-spec skill (read \${CLAUDE_PLUGIN_ROOT}/bmad/skills/bmad-spec/SKILL.md and its references/headless guidance), headless, to write the spec for the request in $RUN/request.md. Do not write a PRD, UX or architecture, and do not write plan.md. The run path contains a space: quote it in every command. Reply with the pass=bmad JSON."

CLAUDE_ARGS="--agent dcv2:planner" \
  run_claude "$WS" "$PLUGIN" "$TMP/spike.jsonl" "$PROMPT" DCV2_RUN_DIR="$RUN" DCV2_TEST_MODE=auto-approve

problems=()
spec="$(find "$RUN/bmad/output" -type f -name 'spec-*.md' -path '*/spec-*/spec-*' 2>/dev/null | head -1)"
if [[ -z "$spec" ]]; then
  problems+=("no spec-*/spec-*.md under <run>/bmad/output")
else
  grep -Eq '^id:[[:space:]]*SPEC-' "$spec" || problems+=("the spec has no 'id: SPEC-' frontmatter: $spec")
  [[ -s "$(dirname "$spec")/.memlog.md" ]] || problems+=("no .memlog.md next to the spec (the runtime was not used to write it)")
fi
python3 - "$TMP/spike.jsonl" <<'PY' || problems+=("the transcript does not show the Planner using the runtime, or shows uv/npx")
import json, sys
used, bad = False, False
for line in open(sys.argv[1]):
    try:
        ev = json.loads(line)
    except ValueError:
        continue
    if ev.get("type") != "assistant":
        continue
    for b in (ev.get("message") or {}).get("content") or []:
        if isinstance(b, dict) and b.get("type") == "tool_use" and b.get("name") == "Bash":
            cmd = str((b.get("input") or {}).get("command", ""))
            used = used or "bmad/runtime.mjs" in cmd
            bad = bad or any(w in cmd.split() for w in ("uv", "npx"))
sys.exit(0 if used and not bad else 1)
PY
[[ -z "$(git -C "$WS/toy" status --porcelain)" ]] || problems+=("the toy repo is not clean: $(git -C "$WS/toy" status --porcelain | head -3 | tr '\n' ' ')")
[[ -z "$(find "$WS" -name '_bmad*' -o -name 'bmad' 2>/dev/null | head -1)" ]] || problems+=("BMAD files in the toy workspace")
[[ "$(git -C "$WS/toy" rev-list --count HEAD)" == 1 ]] || problems+=("the toy repo got commits")

if [[ ${#problems[@]} -gt 0 ]]; then
  for p in "${problems[@]}"; do echo "SPIKE FAILED: $p" >&2; done
  echo "artifacts: $TMP (transcript $TMP/spike.jsonl)" >&2
  exit 1
fi
note "SPIKE PASS: $spec (artifacts in $TMP)"
