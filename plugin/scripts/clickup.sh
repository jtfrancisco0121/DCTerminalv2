#!/usr/bin/env bash
# The ClickUp helper of the persona roles (docs/skills.md, "ClickUp helper"): a thin launcher for
# lib/clickup.py (stdlib Python, started through the one picker lib/python.sh).
#
#   clickup.sh lists                                  read-only: every list with its id
#   clickup.sh preview <file>                         read-only: what `create` would make, and its digest
#   clickup.sh create <list id> <file> --yes <digest> creates the tickets (only after the preview's digest)
#
# macOS only for now (the token comes from the keychain); refuses inside an Eagle-Eye run.
_cdir="${BASH_SOURCE[0]%/*}"; [[ "$_cdir" != "${BASH_SOURCE[0]}" ]] || _cdir="."
. "$_cdir/lib/python.sh" 2>/dev/null
if (( ${#DCV2_PY[@]} == 0 )); then
  echo "ClickUp helper: install Python 3.8+ (tried python3, python, py -3)" >&2
  exit 1
fi
_clib="$(cd "$_cdir/lib" 2>/dev/null && pwd -P)" || { echo "ClickUp helper: the plugin's lib folder is missing" >&2; exit 1; }
# -I: no PYTHON* variables and no user site; the lib folder is passed by this script itself, never read from the caller.
DCV2_LIB_DIR="$_clib" exec "${DCV2_PY[@]}" -I "$_clib/clickup.py" "$@"
