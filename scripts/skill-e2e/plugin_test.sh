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

echo "# scripts parse"
for f in "$P"/scripts/*.sh "$ROOT"/scripts/skill-e2e/*.sh; do
  bash -n "$f" && ok "bash -n $(basename "$f")" || bad "bash -n $f"
done

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
