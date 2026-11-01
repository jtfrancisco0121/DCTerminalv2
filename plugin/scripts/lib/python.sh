# Sourced by guard.sh, memory.sh, state.sh and stop-check.sh: the one Python picker (docs/launch.md).
# Sets the array DCV2_PY to the first of `python3`, `python`, `py -3` that runs Python 3.8 or newer,
# and stops probing at the first that does. It stays empty when none does (the Microsoft Store
# stub exits non-zero, so it is skipped). Bash 3.2-safe: no namerefs, no mapfile, and only builtins
# besides the candidates (guard.sh sources it with a possibly broken PATH).
# A caller runs "${DCV2_PY[@]}" only after checking `(( ${#DCV2_PY[@]} > 0 ))`: under `set -u`
# bash 3.2 fails on expanding an empty array.
DCV2_PY=()
if python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)' >/dev/null 2>&1; then
  DCV2_PY=(python3)
elif python -c 'import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)' >/dev/null 2>&1; then
  DCV2_PY=(python)
elif py -3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)' >/dev/null 2>&1; then
  DCV2_PY=(py -3)
fi
