# Shared helpers for the dcv2 skill end-to-end tests. Source it; do not run it.
# Uses the current Claude account (CLAUDE_CONFIG_DIR as set in your shell).

set -euo pipefail

E2E_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$E2E_DIR/../.." && pwd)"
SRC_PLUGIN="$REPO_ROOT/plugin"

# Model for the orchestrator session and (via the plugin copy) every sub-agent.
MODEL="${DCV2_E2E_MODEL:-haiku}"
# Seconds before a claude run is killed.
CLAUDE_TIMEOUT="${DCV2_E2E_TIMEOUT:-1800}"

fail() { echo "FAIL: $*" >&2; exit 1; }
note() { echo "--- $*"; }

# make_tmp: a fresh temp dir for this test run (kept for inspection; path printed).
make_tmp() {
  local base="${DCV2_E2E_TMP:-${TMPDIR:-/tmp}}"
  TMP="$(mktemp -d "${base%/}/dcv2-e2e.XXXXXX")"
  TMP="$(cd "$TMP" && pwd -P)"
  note "temp dir: $TMP"
}

# make_test_plugin <dest>: a test-only copy of the plugin with every agent's
# model replaced by $MODEL, so the test stays cheap. The source plugin is untouched.
make_test_plugin() {
  local dest="$1"
  rm -rf "$dest"
  cp -R "$SRC_PLUGIN" "$dest"
  local f
  for f in "$dest"/agents/*.md; do
    sed -i.bak -E "s/^model: .*/model: $MODEL/" "$f" && rm -f "$f.bak"
  done
  grep -h '^model:' "$dest"/agents/*.md | sort -u | grep -qx "model: $MODEL" || fail "model override did not apply"
}

# git_init_repo <dir>: a repo on branch main with a local identity, one commit.
git_init_repo() {
  local dir="$1"
  git -C "$dir" init -q -b main
  git -C "$dir" config user.name "dcv2 e2e"
  git -C "$dir" config user.email "dcv2-e2e@example.invalid"
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" add -A
  git -C "$dir" commit -q -m "initial"
}

# make_workspace <ws>: two tiny node repos, each with a failing check.
#   mathlib  : `npm run check` fails (multiply adds)
#   calc-cli : `npm test` fails (formatResult has no "Result: " prefix)
make_workspace() {
  local ws="$1"
  mkdir -p "$ws/mathlib" "$ws/calc-cli"

  cat > "$ws/mathlib/package.json" <<'EOF'
{"name": "mathlib", "version": "1.0.0", "private": true, "scripts": {"check": "node test.js"}}
EOF
  cat > "$ws/mathlib/index.js" <<'EOF'
function add(a, b) { return a + b; }
function multiply(a, b) { return a + b; }
module.exports = { add, multiply };
EOF
  cat > "$ws/mathlib/test.js" <<'EOF'
const assert = require("assert");
const { add, multiply } = require("./index");
assert.strictEqual(add(2, 3), 5);
assert.strictEqual(multiply(2, 3), 6);
assert.strictEqual(multiply(4, 0), 0);
console.log("mathlib: all tests passed");
EOF

  cat > "$ws/calc-cli/package.json" <<'EOF'
{"name": "calc-cli", "version": "1.0.0", "private": true, "scripts": {"test": "node test.js"}}
EOF
  cat > "$ws/calc-cli/format.js" <<'EOF'
function formatResult(n) { return String(n); }
module.exports = { formatResult };
EOF
  cat > "$ws/calc-cli/test.js" <<'EOF'
const assert = require("assert");
const { formatResult } = require("./format");
assert.strictEqual(formatResult(6), "Result: 6");
assert.strictEqual(formatResult(0), "Result: 0");
console.log("calc-cli: all tests passed");
EOF

  git_init_repo "$ws/mathlib"
  git_init_repo "$ws/calc-cli"
  (cd "$ws/mathlib" && ! npm run -s check >/dev/null 2>&1) || fail "mathlib check should fail before the run"
  (cd "$ws/calc-cli" && ! npm test -s >/dev/null 2>&1) || fail "calc-cli check should fail before the run"
}

# write_request <run dir> <ws> <extra test-mode lines...>
write_request() {
  local run="$1" ws="$2"; shift 2
  mkdir -p "$run"
  {
    echo "folder: $ws"
    local line
    for line in "$@"; do echo "$line"; done
    cat <<'EOF'

# Request

Two small fixes in this folder of repos. Keep the plan small: exactly one story per repo, one PR per repo.

1. mathlib (ticket T-1): `multiply(a, b)` returns `a + b`; it must return `a * b`. `npm run check` must pass.
2. calc-cli (ticket T-2): `formatResult(n)` must return `"Result: <n>"` (for example `"Result: 6"`). `npm test` must pass.

Number the mathlib PR as PR1 and the calc-cli PR as PR2. PR2 depends on PR1 (release order: mathlib ships first).
EOF
  } > "$run/request.md"
  cat > "$run/tickets.json" <<'EOF'
{"schema": 1, "tasks": [
  {"id": "t1", "custom_id": "T-1", "name": "multiply() adds instead of multiplying", "url": "https://app.clickup.com/t/t1",
   "status": {"status": "to do"}, "list": {"id": "l1", "name": "Bugs", "statuses": ["to do", "in progress", "in review", "done"]},
   "text_content": "multiply(2, 3) returns 5."},
  {"id": "t2", "custom_id": "T-2", "name": "Prefix CLI results with 'Result: '", "url": "https://app.clickup.com/t/t2",
   "status": {"status": "to do"}, "list": {"id": "l1", "name": "Bugs", "statuses": ["to do", "in progress", "in review", "done"]},
   "text_content": "Show 'Result: 6' instead of '6'."}
]}
EOF
}

# run_claude <cwd> <plugin> <out.jsonl> <prompt>: headless, stream-json, killed after $CLAUDE_TIMEOUT s.
run_claude() {
  local cwd="$1" plugin="$2" out="$3" prompt="$4"
  note "claude -p \"$prompt\" (model $MODEL) in $cwd"
  local start=$SECONDS
  (cd "$cwd" && perl -e 'alarm shift; exec @ARGV' "$CLAUDE_TIMEOUT" \
     claude -p "$prompt" --plugin-dir "$plugin" --model "$MODEL" \
       --permission-mode bypassPermissions --output-format stream-json --verbose \
       > "$out" 2> "${out%.jsonl}.err") || note "claude exited with $? (see ${out%.jsonl}.err)"
  note "claude took $((SECONDS - start))s; transcript $out"
}
