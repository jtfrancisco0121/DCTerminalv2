#!/usr/bin/env bash
# Re-vendor the BMAD-METHOD skills that Eagle-Eye planning uses into plugin/bmad/skills/.
# Manual step. Pinned to a fixed upstream commit: `git clone --branch <sha>` does
# not work, so clone main and check out the SHA (same policy as ADE's vendoring).
#
#   scripts/vendor-bmad.sh [<sha> [<label>]]     defaults: bda3c59 "6.13.0-next"
#   BMAD_SRC=<local clone> scripts/vendor-bmad.sh   offline: use a local clone
#
# Only the allowlisted skills are copied, with every scripts/**/*.py and tests/
# folder left out (the Node runtime replaces the scripts). Then the patcher runs.
# Running it again on the same SHA leaves `git status` clean.
set -euo pipefail

SHA="${1:-bda3c59}"
LABEL="${2:-6.13.0-next}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/plugin/bmad"
SKILLS="bmad bmod-core-tools bmad-spec bmad-prd bmad-architecture bmad-ux bmad-review"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if [ -n "${BMAD_SRC:-}" ]; then
  SRC="$BMAD_SRC"
else
  git clone --quiet https://github.com/bmad-code-org/BMAD-METHOD "$TMP/bmad"
  SRC="$TMP/bmad"
fi
git -C "$SRC" checkout --quiet "$SHA"

mkdir -p "$DEST"
rm -rf "$DEST/skills"
mkdir -p "$DEST/skills"
for s in $SKILLS; do
  if [ ! -d "$SRC/skills/$s" ]; then
    echo "vendor-bmad: upstream has no skills/$s at $SHA" >&2
    exit 1
  fi
  cp -R "$SRC/skills/$s" "$DEST/skills/$s"
done
# Python scripts and their tests stay out: the Node runtime replaces them.
find "$DEST/skills" -type d -name tests -prune -exec rm -rf {} +
find "$DEST/skills" -type f -name '*.py' -delete
find "$DEST/skills" -type d -name __pycache__ -prune -exec rm -rf {} +
find "$DEST/skills" -type d -empty -delete

printf '%s %s\n' "$SHA" "$LABEL" > "$DEST/VERSION"
cp "$SRC/LICENSE" "$DEST/LICENSE"

node "$ROOT/scripts/patch-bmad.mjs" "$DEST/skills"
(cd "$ROOT" && npx --no-install vitest run scripts/patch-bmad.test.mjs scripts/bmad-allowlist.test.mjs)
echo "vendored $SHA ($LABEL) at $DEST"
