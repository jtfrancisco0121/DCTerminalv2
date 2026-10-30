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

echo "# scripts parse"
for f in "$P"/scripts/*.sh "$ROOT"/scripts/skill-e2e/*.sh; do
  bash -n "$f" && ok "bash -n $(basename "$f")" || bad "bash -n $f"
done

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
