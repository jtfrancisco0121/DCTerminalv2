#!/usr/bin/env bash
# Run-folder writes for the eagle-eye orchestrator.
#
#   state.sh put <file.json>   < json     validate the JSON on stdin, then write it
#                                          atomically (<file>.tmp + mv). Invalid JSON:
#                                          exit 1, the file is left as it was.
#   state.sh log <run dir> <stage> <text>  append "<UTC time> <stage> <text>" to log.md
#   state.sh now                           print the UTC time (ISO-8601)
set -euo pipefail

validate() { # <file>: JSON parses (python3, else node, else macOS JavaScript)
  local f="$1"
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$f" <<'PY'
import json, sys
try:
    doc = json.load(open(sys.argv[1]))
except json.JSONDecodeError as e:
    sys.exit(f"JSON syntax error: {e}")
if isinstance(doc, dict) and "stage" in doc:
    stages = {"planning", "plan-approval", "building", "ship-approval", "shipping", "done", "failed"}
    states = {"pending", "building", "reviewing", "fixing", "reviewed", "needs-attention", "opened", "pushed", "local-only"}
    problems = []
    if doc.get("schema") != 1: problems.append("schema must be 1")
    if doc.get("stage") not in stages: problems.append(f"stage {doc.get('stage')!r} not one of {sorted(stages)}")
    if doc.get("waitingFor") not in (None, "plan-approval", "ship-approval"): problems.append("bad waitingFor")
    if not isinstance(doc.get("prs"), list): problems.append("prs must be a list")
    for pr in doc.get("prs") or []:
        if pr.get("state") not in states: problems.append(f"{pr.get('id')}: state {pr.get('state')!r} not one of {sorted(states)}")
    if problems:
        sys.exit("status.json: " + "; ".join(problems))
PY
  elif command -v node >/dev/null 2>&1; then
    node -e 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"))' "$f"
  elif command -v osascript >/dev/null 2>&1; then
    osascript -l JavaScript -e 'ObjC.import("Foundation"); function run(a) { JSON.parse($.NSString.stringWithContentsOfFileEncodingError(a[0], 4, null).js); }' "$f" >/dev/null
  fi
}

case "${1:-}" in
  put)
    dest="${2:?usage: state.sh put <file.json>}"
    tmp="$dest.tmp"
    cat > "$tmp"
    if ! err="$(validate "$tmp" 2>&1)"; then
      rm -f "$tmp"
      echo "NOT WRITTEN: invalid JSON for $dest: $err" >&2
      exit 1
    fi
    mv -f "$tmp" "$dest"
    echo "wrote $dest"
    ;;
  log)
    run="${2:?usage: state.sh log <run dir> <stage> <text>}"; stage="${3:?stage}"; shift 3
    text="$*"; text="${text//$'\n'/ }"
    printf '%s %s %s\n' "$(date -u +%FT%TZ)" "$stage" "$text" >> "$run/log.md"
    ;;
  now)
    date -u +%FT%TZ
    ;;
  *)
    echo "usage: state.sh put <file.json> | log <run> <stage> <text> | now" >&2
    exit 2
    ;;
esac
