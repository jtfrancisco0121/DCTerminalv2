#!/usr/bin/env bash
# Unit test of plugin/scripts/guard.sh (no Claude call).
set -euo pipefail
GUARD="$(cd "$(dirname "$0")/../.." && pwd)/plugin/scripts/guard.sh"
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
mkdir -p "$d/run" "$d/ws/r" "$d/ws/r-worktrees/dcv2-x-pr1"; touch "$d/run/request.md" "$d/run/status.json"
fails=0
t() { # <expected exit> <agent type> <path>
  local got=0
  echo "{\"agent_type\":\"$2\",\"tool_input\":{\"file_path\":\"$3\"},\"cwd\":\"$d/ws\"}" | bash "$GUARD" 2>/dev/null || got=$?
  if [[ $got == "$1" ]]; then echo "ok   $2 ${3#$d} -> $got"; else echo "FAIL $2 ${3#$d} -> $got (want $1)"; fails=1; fi
}
t 2 dcv2:implementer "$d/ws/r/a.js"
t 0 dcv2:implementer "$d/ws/r-worktrees/dcv2-x-pr1/a.js"
t 0 dcv2:implementer "$d/run/reports/PR1-build1.md"
t 2 dcv2:implementer "r/a.js"
t 2 dcv2:pr-reviewer "$d/ws/r-worktrees/dcv2-x-pr1/a.js"
t 0 dcv2:pr-reviewer "$d/run/reviews/PR1-round1.md"
t 2 dcv2:planner "$d/ws/r/plan.md"
t 0 dcv2:planner "$d/run/plan.md"
t 0 "" "$d/ws/r/a.js"
rm -rf "$d"
[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
