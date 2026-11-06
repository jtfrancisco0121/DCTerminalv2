#!/usr/bin/env bash
# Static test of the plugin folder (A5, A4, hooks): no Claude call.
# - plugin/VERSION exists (one line, same as plugin.json's version)
# - every standalone skill and every pipeline agent reads its role file, and the role file exists
# - every role file carries the memory trust rule (D9)
# - hooks.json and launch/settings.json parse; hook scripts exist; settings use only known keys
# - every script passes bash -n
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
P="$ROOT/plugin"
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }

echo "# VERSION (A5)"
if [[ -f "$P/VERSION" ]]; then
  ver="$(cat "$P/VERSION")"
  [[ "$(wc -l < "$P/VERSION" | tr -d ' ')" == 1 && "$ver" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.+-][0-9A-Za-z.]+)?$ ]] && ok "VERSION is one line: $ver" || bad "VERSION: '$ver'"
  pj="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$P/.claude-plugin/plugin.json")"
  [[ "$pj" == "$ver" ]] && ok "plugin.json version matches" || bad "plugin.json version $pj != VERSION $ver"
else
  bad "plugin/VERSION missing"
fi

echo "# vendored BMAD (plugin/bmad)"
ls "$P"/skills | grep -qiE '^(bmad|bmod)' && bad "a BMAD skill is registered under plugin/skills (they are read by path, never registered)" || ok "no BMAD skill is registered in sessions"
for f in VERSION LICENSE NOTICE.md ade-runtime.mjs runtime.mjs; do
  [[ -s "$P/bmad/$f" ]] && ok "bmad/$f exists" || bad "bmad/$f missing"
done
[[ "$(ls "$P/bmad/skills" | wc -l | tr -d ' ')" == 7 ]] && ok "bmad/skills holds 7 skills" || bad "bmad/skills must hold exactly the 7 allowlisted skills"
[[ -z "$(find "$P/bmad/skills" -name '*.py' | head -1)" ]] && ok "no .py file under bmad/skills" || bad "a .py file under bmad/skills"
node --check "$P/bmad/runtime.mjs" 2>/dev/null && ok "node --check runtime.mjs" || bad "runtime.mjs does not parse"

echo "# roles are the single source (A5, D8)"
for f in "$P"/skills/*/SKILL.md; do
  name="$(basename "$(dirname "$f")")"
  [[ "$name" == eagle-eye ]] && continue
  grep -qF '${CLAUDE_PLUGIN_ROOT}/roles/'"$name.md" "$f" && ok "skill $name reads roles/$name.md" || bad "skill $name does not read roles/$name.md"
  [[ -f "$P/roles/$name.md" ]] || bad "roles/$name.md missing"
  grep -q "^name: $name$" "$f" || bad "skill $name: frontmatter name"
done
for f in "$P"/agents/*.md; do
  name="$(basename "$f" .md)"
  grep -qF '${CLAUDE_PLUGIN_ROOT}/roles/'"$name.md" "$f" && ok "agent $name reads roles/$name.md" || bad "agent $name does not read roles/$name.md"
  [[ -f "$P/roles/$name.md" ]] || bad "roles/$name.md missing"
  grep -qE '^model: (opus|sonnet|haiku)$' "$f" && ok "  agent $name names its model" || bad "agent $name: no model"
done
for f in "$P"/roles/*.md; do
  name="$(basename "$f" .md)"
  [[ -f "$P/skills/$name/SKILL.md" || -f "$P/agents/$name.md" ]] && ok "role $name is used" || bad "role $name used by no skill or agent"
  grep -qF "the code is the truth" "$f" && ok "  role $name has the trust rule" || bad "role $name lacks the trust rule (memory is a hint, the code is the truth)"
done

echo "# hooks and launch settings (A3, A4)"
python3 - "$P" <<'PY' && ok "hooks.json and launch/settings.json are valid" || fails=1
import json, os, re, sys
p = sys.argv[1]
h = json.load(open(os.path.join(p, "hooks", "hooks.json")))["hooks"]
for ev in ("PreToolUse", "SubagentStop", "Stop"):
    assert ev in h, f"hooks.json has no {ev}"
pre = h["PreToolUse"][0]["matcher"].split("|")
for t in ("Edit", "Write", "NotebookEdit", "MultiEdit", "Bash", "Monitor", "PowerShell", "mcp__.*"):
    assert t in pre, f"the PreToolUse matcher does not cover {t}: {pre}"
for ev, groups in h.items():
    for g in groups:
        for hk in g["hooks"]:
            m = re.search(r"\$\{CLAUDE_PLUGIN_ROOT\}/(scripts/[\w.-]+)", hk["command"])
            assert m and os.path.isfile(os.path.join(p, m.group(1))), f"{ev}: missing script in {hk['command']}"
s = json.load(open(os.path.join(p, "launch", "settings.json")))
assert set(s) <= {"env", "permissions"}, f"unexpected settings keys {set(s)}"
# Only env vars the installed Claude Code (2.1.287) knows; see docs/launch.md.
assert set(s["env"]) <= {"BASH_MAX_OUTPUT_LENGTH", "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", "CLAUDE_CODE_SUBAGENT_MODEL"}, s["env"]
assert all(isinstance(v, str) for v in s["env"].values()), "env values are strings"
assert 1 <= int(s["env"]["CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"]) <= 100
assert int(s["env"]["BASH_MAX_OUTPUT_LENGTH"]) <= 150000
assert set(s["permissions"]) == {"deny"}
for r in s["permissions"]["deny"]:
    assert re.fullmatch(r"(Read|Edit)\([^()]+\)", r), r
    assert "/Users/" not in r and "~" not in r, f"no machine paths: {r}"
assert any("node_modules" in r for r in s["permissions"]["deny"])
assert not any(x in r for r in s["permissions"]["deny"] for x in (".lock", "lock.json", "lock.yaml", "go.sum")), "lockfiles stay readable"
assert all(r.startswith(("Read(./", "Read(**/node_modules", "Read(**/dist", "Read(**/.next", "Read(**/__pycache__")) for r in s["permissions"]["deny"]), \
    "only unambiguous build output is denied at any depth; target/ and coverage/ only at the start folder (and one level down)"
PY

echo "# Agent description rule (0.7.0): exact PR attribution"
SK="$P/skills/eagle-eye/SKILL.md"
for a in implementer pr-reviewer; do
  python3 - "$SK" "$a" <<'PY' && ok "SKILL.md names the PR<n> description rule for dcv2:$a" || bad "SKILL.md: no PR<n> description rule for dcv2:$a"
import re, sys
text, agent = open(sys.argv[1]).read(), sys.argv[2]
step3 = text.split("## Step 3", 1)[1].split("\n## ", 1)[0]
assert f"`dcv2:{agent}`" in step3
assert re.search(r"`description` of every `dcv2:implementer` and `dcv2:pr-reviewer` launch starts with `PR<n> `", step3)
PY
done

echo "# commit body and version-bump rules (AD-10)"
VFILES=('package.json` `version' 'src-tauri/Cargo.toml` `[package] version' 'src-tauri/tauri.conf.json` `version' 'plugin/VERSION' 'plugin/.claude-plugin/plugin.json` `version')
for r in implementer developer; do
  grep -qF 'why paragraph' "$P/roles/$r.md" && grep -qF 'S<n>: <short title>' "$P/roles/$r.md" && grep -qF 'review fixes: <short summary>' "$P/roles/$r.md" \
    && ok "$r.md requires a commit body after the subject" || bad "$r.md lacks the commit body rule"
done
for r in implementer developer pr-reviewer; do
  for v in "${VFILES[@]}"; do
    grep -qF "$v" "$P/roles/$r.md" && ok "  $r.md lists $v" || bad "$r.md does not list the version file $v"
  done
done
grep -qE 'unplanned change to a release version file' "$P/roles/pr-reviewer.md" && grep -qE '\*\*MEDIUM\*\*: an unplanned change' "$P/roles/pr-reviewer.md" \
  && ok "pr-reviewer.md classes an unplanned version bump as MEDIUM" || bad "pr-reviewer.md: unplanned version bump is not MEDIUM"
F="$ROOT/fixtures/reviews/unplanned-version-bump.md"
grep -qE '^1\. MEDIUM .*version' "$F" && grep -q '^## Criteria → tests' "$F" && head -1 "$F" | grep -q '^Eagle-Eye review' \
  && ok "fixture review flags the unplanned bump MEDIUM, with the criteria table" || bad "fixture review structure"

echo "# decision log (AD-5, AD-6)"
for r in planner plan-reviewer risk-reviewer recommendation memory-updater developer; do
  grep -q 'topics/decisions.md' "$P/roles/$r.md" && ok "role $r describes the decision log" || bad "role $r does not mention topics/decisions.md"
done
grep -q '^## Decisions' "$P/roles/planner.md" && grep -q 'rules out: what' "$P/roles/planner.md" && ok "planner.md has the Decisions section and the bullet form" || bad "planner.md Decisions"
grep -q 'why the entry' "$P/roles/plan-reviewer.md" && grep -q 'MEDIUM' "$P/roles/plan-reviewer.md" && ok "plan-reviewer.md flags a contradiction without a stated reason" || bad "plan-reviewer.md rule"
python3 - "$ROOT" <<'PY' && ok "seed decisions file and the contradiction fixtures are well formed" || bad "decision fixtures"
import re, sys
root = sys.argv[1]
seed = open(root + "/fixtures/memory/dcterminalv2-decisions.md").read()
bullets = [l for l in seed.splitlines() if l.startswith("- ")]
assert len(bullets) == 4 and all(re.match(r"^- \d{4}-\d{2}-\d{2} \| [^|]+ \| [^|]+ \| rules out: [^|]+$", b) for b in bullets), bullets
assert "covers:" in seed and "checked-at:" in seed
plan = open(root + "/fixtures/plans/contradicts-decision.md").read()
assert "GitHub Actions" in plan and "## Decisions" not in plan
rev = open(root + "/fixtures/reviews/plan-contradicts-decision.md").read()
assert re.search(r"^1\. \[MEDIUM\] .*No hosted CI", rev, re.M) and "REQUIRES REVISION" in rev
PY

echo "# portable helpers (lib/dcv2_portable.py) and no bytecode in the plugin"
if out="$(python3 -B "$ROOT/scripts/skill-e2e/portable_test.py" 2>&1)"; then
  ok "portable_test.py: $(grep -E '^(Ran|OK)' <<<"$out" | tr '\n' ' ')"
  grep '^skip:' <<<"$out" | sed 's/^/     /' || true
else
  bad "portable_test.py failed"; echo "$out" | tail -20
fi
[[ -z "$(find "$P" -name __pycache__ -o -name '*.pyc' | head -1)" ]] && ok "plugin/ holds no __pycache__ or .pyc" || bad "bytecode under plugin/: $(find "$P" -name __pycache__ -o -name '*.pyc' | head -3)"

echo "# eval runs in the eagle-eye skill (0.8.0)"
SK="$P/skills/eagle-eye/SKILL.md"
grep -q '^## Eval runs' "$SK" && ok "SKILL.md has the Eval runs section" || bad "SKILL.md: no Eval runs section"
grep -qF 'S approve <run> plan eval' "$SK" && ok "  eval plan approval" || bad "  SKILL.md: no S approve <run> plan eval"
grep -qF 'S base <repoPath> <run>' "$SK" && ok "  step 2 passes the run to S base" || bad "  SKILL.md: Step 2 does not call S base <repoPath> <run>"
grep -qF 'with an `eval=` line from preflight: report "eval run finished" and stop' "$SK" && ok "  resume rule for ship-approval with eval=" || bad "  SKILL.md: no eval resume rule"
grep -qF 'S request <run> cleanup' "$SK" && ok "  eval clean-up routing through S request <run> cleanup" || bad "  SKILL.md: no eval clean up routing"
grep -qF 'There is **no stop #2**' "$SK" && ok "  no stop #2 for an eval run" || bad "  SKILL.md: the eval end does not say there is no stop #2"

echo "# memory paths come only from memory.sh (0.8.1)"
MU="$P/roles/memory-updater.md"; MA="$P/agents/memory-updater.md"; DV="$P/skills/developer/SKILL.md"
grep -qF 'paths `memory.sh` prints' "$MU" && ok "  role: only printed paths" || bad "  memory-updater role: no 'only paths memory.sh prints' rule"
grep -qF 'is `FAILED` with its message, never `NO MEMORY`' "$MU" && ok "  role: a failed command is FAILED" || bad "  memory-updater role: no FAILED rule"
grep -qF '`NO MEMORY` only when' "$MU" && ok "  role: NO MEMORY only for present false" || bad "  memory-updater role: NO MEMORY rule"
grep -qF 'only at paths `memory.sh` prints' "$MA" && ok "  agent: only printed paths" || bad "  memory-updater agent: no printed-paths rule"
grep -qF 'is `FAILED` with its message, never `NO MEMORY`' "$MA" && ok "  agent: FAILED rule" || bad "  memory-updater agent: no FAILED rule"
grep -qF 'memory.sh id <repo>' "$DV" && ok "  developer skill step 8 passes the repo id" || bad "  developer SKILL.md step 8 does not pass id"

echo "# QA and triage commands are documented (0.10.0)"
for c in 'qa-serve' 'state.sh qa ' 'plan <run> qa' 'state.sh triage' 'run_qa_image' 'triage_start' 'triage_approve' 'triage_apply' 'triageSchedule' 'create_task'; do
  { grep -qF "$c" "$ROOT/docs/launch.md" || grep -qF "$c" "$ROOT/docs/run-folder.md"; } && ok "  docs name $c" || bad "  docs do not name $c"
done
grep -q 'dcv2:qa' "$ROOT/docs/skills.md" && ok "  skills.md names dcv2:qa" || bad "  skills.md lacks dcv2:qa"

for r in planner plan-reviewer; do
  grep -qF 'visible criteria marker' "$P/roles/$r.md" && grep -qF '`(app)`' "$P/roles/$r.md" && ok "  $r.md describes the (app) marker" || bad "  $r.md lacks the visible criteria marker"
done

echo "# every pipeline agent is a guard pipeline role (an agent missing from PIPE_RE or ROLES is treated as the orchestrator)"
for f in "$P"/agents/*.md; do
  name="$(basename "$f" .md)"
  grep -E '^PIPE_RE=' "$P/scripts/guard.sh" | grep -qE "[(|]$name[|)]" && ok "  $name is in guard.sh PIPE_RE" || bad "  $name is missing from guard.sh PIPE_RE"
  grep -E '^ROLES = ' "$P/scripts/guard.sh" | grep -qF "\"$name\"" && ok "  $name is in guard.sh ROLES" || bad "  $name is missing from guard.sh ROLES"
done
grep -qF 'dcv2:qa' "$P/skills/eagle-eye/SKILL.md" && grep -qF 'qa-serve' "$P/skills/eagle-eye/SKILL.md" && grep -qF 'S qa-serve <run> <PR id> stop' "$P/skills/eagle-eye/SKILL.md" \
  && ok "  eagle-eye SKILL.md describes the qa step and always stops the server" || bad "  SKILL.md: no QA step"

QS="$P/skills/qa/SKILL.md"
[[ -f "$QS" ]] && grep -qF '${CLAUDE_PLUGIN_ROOT}/roles/qa.md' "$QS" && ok "standalone /dcv2:qa reads roles/qa.md" || bad "skills/qa/SKILL.md missing or does not read roles/qa.md"
grep -qF 'Never edit, commit, stash or switch branches in the user' "$QS" && ok "  forbids edits, commits and branch switches in the user's checkout" || bad "  qa skill: no checkout rule"
grep -qF 'Cleanup on every exit path' "$QS" && grep -qF 'git worktree remove --force' "$QS" && grep -qF 'stop the server you started' "$QS" \
  && ok "  stops the server and removes its worktree on every exit path" || bad "  qa skill: no server stop / worktree removal"
grep -qF 'not by a prefix' "$QS" && ok "  checks the URL host, not a prefix" || bad "  qa skill: no loopback host rule"

echo "# scripts parse"
for f in "$P"/scripts/*.sh "$P"/scripts/lib/*.sh "$ROOT"/scripts/skill-e2e/*.sh; do
  bash -n "$f" && ok "bash -n $(basename "$f")" || bad "bash -n $f"
done

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
