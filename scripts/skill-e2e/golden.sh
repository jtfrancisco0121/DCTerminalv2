#!/usr/bin/env bash
# Golden-run suite (R7): three toy workspaces through the whole Eagle-Eye run, headless, launched
# as docs/launch.md says (--settings plugin/launch/settings.json, --append-system-prompt-file from
# memory.sh compose, DCV2_MEMORY_DIR, DCV2_MAX_IMPLEMENTERS). Run before every release.
#
#   scripts/skill-e2e/golden.sh               # all three, in parallel
#   scripts/skill-e2e/golden.sh multi fix     # some of them
#
#   multi : a folder of two independent repos; /dcv2:onboard writes their memory first; the run
#           has DCV2_MAX_IMPLEMENTERS=1, so the two PRs must build one after the other; the run
#           ends with memory-update.md and report.md, and memory is written only via memory.sh.
#   dep   : two repos where PR2 depends on PR1: PR2 starts only after PR1 is reviewed clean.
#   fix   : one repo whose check already fails for a reason the request does not name: the
#           PR is reviewed only once the whole check passes.
#   github: github review mode (meta.json reviewMode github) on one repo with a local bare remote
#           and the gh shim (scripts/skill-e2e/gh-shim, no real GitHub): a draft PR after the first
#           build, each review round posted as a comment, marked ready at stop #2; repos.json
#           copyFiles brings a git-ignored .env into the worktree and it is never committed.
#           Not in the default set: `golden.sh github`.
#   bmad  : BMAD planning (meta.json bmad.mode "on") on one repo, in a run folder whose path has
#           a space (like Application Support): the Planner writes the spec and the architecture
#           under <run>/bmad/output, state.sh bmad-check passes, plan.md cites the spec items, and
#           the repo gets no BMAD files. Not in the default set: `golden.sh bmad`.
#   fix also writes meta.json bmad.mode "off" (the unchanged flow).
#
# Env as run.sh (DCV2_E2E_MODEL default haiku, DCV2_E2E_TIMEOUT, DCV2_E2E_TMP). Uses the current
# Claude account (CLAUDE_CONFIG_DIR). No remotes (github: a local bare one): nothing reaches GitHub.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

CASES=("$@")
[[ ${#CASES[@]} -gt 0 ]] || CASES=(multi dep fix)
make_tmp
PLUGIN="$TMP/plugin"
make_test_plugin "$PLUGIN"
MEM="$PLUGIN/scripts/memory.sh"
SETTINGS="$PLUGIN/launch/settings.json"

node_repo() { # <dir> <package.json scripts json> ; files come from the caller's heredocs
  mkdir -p "$1"
  printf '{"name": "%s", "version": "1.0.0", "private": true, "scripts": %s}\n' "$(basename "$1")" "$2" > "$1/package.json"
}

request() { # <run> <ws> ; request text on stdin
  mkdir -p "$1"
  { echo "folder: $2"; echo; cat; } > "$1/request.md"
}

# launch <case> <ws> <run> [VAR=value...]: compose the memory prompt and run /dcv2:eagle-eye like the app.
launch() {
  local c="$1" ws="$2" run="$3"; shift 3
  DCV2_MEMORY_DIR="$TMP/memory" bash "$MEM" compose "$ws" "$TMP/$c-prompt.md" >/dev/null
  CLAUDE_ARGS="--settings $SETTINGS --append-system-prompt-file $TMP/$c-prompt.md" \
    run_claude "$ws" "$PLUGIN" "$TMP/$c.jsonl" "/dcv2:eagle-eye run=$run" \
      DCV2_RUN_DIR="$run" DCV2_TEST_MODE=auto-approve DCV2_MEMORY_DIR="$TMP/memory" "$@"
}

case_multi() {
  local ws="$TMP/ws-multi" run="$TMP/runs/r-golden-multi"
  node_repo "$ws/strutil" '{"check": "node test.js"}'
  cat > "$ws/strutil/index.js" <<'EOF'
// String helpers used by the CLI.
function reverse(s) { return s; }
function shout(s) { return s.toUpperCase() + "!"; }
module.exports = { reverse, shout };
EOF
  cat > "$ws/strutil/test.js" <<'EOF'
const assert = require("assert");
const { reverse, shout } = require("./index");
assert.strictEqual(shout("hi"), "HI!");
assert.strictEqual(reverse("abc"), "cba");
assert.strictEqual(reverse(""), "");
console.log("strutil: ok");
EOF
  node_repo "$ws/units" '{"test": "node test.js"}'
  cat > "$ws/units/convert.js" <<'EOF'
// Unit conversions. 1 km = 0.621371 miles.
function kmToMiles(km) { return km * 1.609344; }
module.exports = { kmToMiles };
EOF
  cat > "$ws/units/test.js" <<'EOF'
const assert = require("assert");
const { kmToMiles } = require("./convert");
assert.strictEqual(Math.round(kmToMiles(10) * 1000) / 1000, 6.214);
assert.strictEqual(kmToMiles(0), 0);
console.log("units: ok");
EOF
  git_init_repo "$ws/strutil"; git_init_repo "$ws/units"
  note "[multi] onboard"
  CLAUDE_ARGS="--settings $SETTINGS" run_claude "$ws" "$PLUGIN" "$TMP/multi-onboard.jsonl" \
    "/dcv2:onboard Keep it very small: per repo an index.md, one topic and a contracts topic; and the workspace.md." \
    DCV2_MEMORY_DIR="$TMP/memory"
  request "$run" "$ws" <<'EOF'
# Request

Two independent fixes, one PR per repo, no dependency between them. Keep the plan small: one story per repo.

1. strutil: `reverse(s)` returns `s` unchanged; it must return the string reversed. `npm run check` must pass.
2. units: `kmToMiles(km)` multiplies by 1.609344 (that is miles to km); it must return km * 0.621371. `npm test` must pass.
EOF
  launch multi "$ws" "$run" DCV2_MAX_IMPLEMENTERS=1
  python3 "$E2E_DIR/golden_check.py" multi "$run" "$ws" "$TMP/multi.jsonl" "$TMP/memory" "$TMP/multi-onboard.jsonl"
}

case_dep() {
  local ws="$TMP/ws-dep" run="$TMP/runs/r-golden-dep"
  node_repo "$ws/pricing" '{"check": "node test.js"}'
  cat > "$ws/pricing/money.js" <<'EOF'
function cents(n) { return Math.round(n * 100); }
module.exports = { cents };
EOF
  cat > "$ws/pricing/test.js" <<'EOF'
const assert = require("assert");
const m = require("./money");
assert.strictEqual(m.cents(1.234), 123);
assert.strictEqual(typeof m.formatPrice, "function", "formatPrice is missing");
assert.strictEqual(m.formatPrice(123), "$1.23");
assert.strictEqual(m.formatPrice(5), "$0.05");
console.log("pricing: ok");
EOF
  node_repo "$ws/shop" '{"test": "node test.js"}'
  cat > "$ws/shop/label.js" <<'EOF'
// The shop's price label. It will show prices formatted like pricing's formatPrice ("$1.23").
function label(name, cents) { return name + ": " + cents; }
module.exports = { label };
EOF
  cat > "$ws/shop/test.js" <<'EOF'
const assert = require("assert");
const { label } = require("./label");
assert.strictEqual(label("Tea", 123), "Tea: $1.23");
assert.strictEqual(label("Gum", 5), "Gum: $0.05");
console.log("shop: ok");
EOF
  git_init_repo "$ws/pricing"; git_init_repo "$ws/shop"
  request "$run" "$ws" <<'EOF'
# Request

Prices as dollars. One story per repo, one PR per repo.

1. pricing (PR1): add `formatPrice(cents)` to money.js returning a dollar string like `"$1.23"` (two decimals). `npm run check` must pass.
2. shop (PR2): `label(name, cents)` must return `"<name>: <price>"` with the price formatted the same way (`"Tea: $1.23"`); copy the formatting logic into shop (no cross-repo import). `npm test` must pass.

PR2 depends on PR1: the shop change ships after pricing.
EOF
  launch dep "$ws" "$run"
  python3 "$E2E_DIR/golden_check.py" dep "$run" "$ws" "$TMP/dep.jsonl" "$TMP/memory"
}

case_fix() {
  local ws="$TMP/ws-fix" run="$TMP/runs/r-golden-fix"
  node_repo "$ws/textkit" '{"check": "node test.js"}'
  cat > "$ws/textkit/text.js" <<'EOF'
function capitalize(s) { return s[0].toUpperCase() + s.slice(1); }
module.exports = { capitalize };
EOF
  cat > "$ws/textkit/test.js" <<'EOF'
const assert = require("assert");
const t = require("./text");
assert.strictEqual(t.capitalize("tea"), "Tea");
assert.strictEqual(t.capitalize(""), "");
assert.strictEqual(typeof t.slugify, "function", "slugify is missing");
assert.strictEqual(t.slugify("Hello World"), "hello-world");
assert.strictEqual(t.slugify("  Tea & Cake  "), "tea-cake");
console.log("textkit: ok");
EOF
  git_init_repo "$ws/textkit"
  request "$run" "$ws" <<'EOF'
# Request

textkit: add `slugify(s)` to text.js: lowercase, words joined by single hyphens, anything that is not a letter or digit dropped, no leading or trailing hyphen (`"  Tea & Cake  "` → `"tea-cake"`). One story, one PR. `npm run check` must pass as a whole (it fails today).
EOF
  printf '{"schema": 1, "runId": "r-golden-fix", "bmad": {"mode": "off", "version": "bda3c59 6.13.0-next"}}\n' > "$run/meta.json"
  launch fix "$ws" "$run"
  python3 "$E2E_DIR/golden_check.py" fix "$run" "$ws" "$TMP/fix.jsonl" "$TMP/memory"
}

case_github() {
  local ws="$TMP/ws-github" run="$TMP/runs/r-golden-github" shimdir="$TMP/gh-shim-state"
  node_repo "$ws/greet" '{"check": "node test.js"}'
  cat > "$ws/greet/greet.js" <<'EOF'
function greet(name) { return "Hello " + name; }
module.exports = { greet };
EOF
  cat > "$ws/greet/test.js" <<'EOF'
const assert = require("assert");
const { greet } = require("./greet");
assert.strictEqual(greet("Ann"), "Hello, Ann!");
assert.strictEqual(greet(""), "Hello, friend!");
console.log("greet: ok");
EOF
  printf '.env\n' > "$ws/greet/.gitignore"
  git_init_repo "$ws/greet"
  echo 'GREETING_STYLE=formal' > "$ws/greet/.env"
  git init -q --bare -b main "$TMP/greet-origin.git"
  git -C "$ws/greet" remote add origin "$TMP/greet-origin.git"
  git -C "$ws/greet" push -q origin main
  git -C "$ws/greet" remote set-head origin main >/dev/null 2>&1 || true
  mkdir -p "$shimdir"
  request "$run" "$ws" <<'EOF'
# Request

greet: `greet(name)` must return `"Hello, <name>!"`, and `"Hello, friend!"` for an empty name. One story, one PR. `npm run check` must pass.
EOF
  printf '{"schema": 1, "runId": "r-golden-github", "reviewMode": "github"}\n' > "$run/meta.json"
  printf '{"schema": 1, "repos": [{"name": "greet", "path": "%s", "check": "npm run check", "setup": "", "copyFiles": [".env"]}]}\n' "$ws/greet" > "$run/repos.json"
  launch github "$ws" "$run" PATH="$E2E_DIR/gh-shim:$PATH" GH_SHIM_DIR="$shimdir" DCV2_TEST_LOCAL_REMOTES=1
  GH_SHIM_DIR="$shimdir" python3 "$E2E_DIR/golden_check.py" github "$run" "$ws" "$TMP/github.jsonl" "$TMP/memory" "$TMP/greet-origin.git"
}

case_bmad() {
  local ws="$TMP/ws-bmad" run="$TMP/App Support/runs/r-golden-bmad"
  node_repo "$ws/pct" '{"check": "node test.js"}'
  cat > "$ws/pct/pct.js" <<'EOF'
// Percentages for the report screen.
function percent(part, whole) { return part / whole; }
module.exports = { percent };
EOF
  cat > "$ws/pct/test.js" <<'EOF'
const assert = require("assert");
const { percent } = require("./pct");
assert.strictEqual(percent(1, 4), 25);
assert.strictEqual(percent(1, 3), 33.3);
assert.strictEqual(percent(0, 5), 0);
assert.strictEqual(percent(1, 0), null);
console.log("pct: ok");
EOF
  git_init_repo "$ws/pct"
  request "$run" "$ws" <<'EOF'
# Request

pct: `percent(part, whole)` must return the percentage rounded to one decimal (`percent(1, 3)` is `33.3`), and `null` when `whole` is 0. One story, one PR. `npm run check` must pass.
EOF
  printf '{"schema": 1, "runId": "r-golden-bmad", "bmad": {"mode": "on", "version": "bda3c59 6.13.0-next"}}\n' > "$run/meta.json"
  launch bmad "$ws" "$run"
  python3 "$E2E_DIR/golden_check.py" bmad "$run" "$ws" "$TMP/bmad.jsonl" "$TMP/memory"
}

status=0
pids=()
for c in "${CASES[@]}"; do
  declare -F "case_$c" >/dev/null || fail "unknown case $c (multi, dep, fix, github, bmad)"
  ( "case_$c" > "$TMP/$c.out" 2>&1 ) &
  pids+=("$!")
  note "started case $c (output $TMP/$c.out)"
done
for i in "${!CASES[@]}"; do
  c="${CASES[$i]}"
  if wait "${pids[$i]}"; then note "case $c: PASS"; else note "case $c: FAILED"; status=1; fi
  sed 's/^/    /' "$TMP/$c.out"
done
if [[ $status == 0 ]]; then note "GOLDEN PASS (artifacts in $TMP)"; else note "GOLDEN FAILED (artifacts in $TMP)"; fi
exit $status
