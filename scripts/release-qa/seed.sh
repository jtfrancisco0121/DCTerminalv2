#!/usr/bin/env bash
# Release QA, step 2 (as the TEST USER, never the operator): seed the test user's DCTerminalv2 data.
#
#   scripts/release-qa/seed.sh <version>
#
# Makes a test Claude account (its own config folder), a scratch git repo and the app's settings.json with that one account.
# REFUSES, touching nothing: when `whoami` is the operator named in release.json, when the keychain holds a DCTerminalv2
# ClickUp item, or when the app data already holds other accounts or a ClickUp token. bash 3.2, BSD tools.
set -euo pipefail

QA_ROOT="${DCV2_RELEASE_QA_ROOT:-/Users/Shared/dcv2-release-qa}"
die() { printf 'seed.sh: %s\n' "$*" >&2; exit 1; }
say() { printf '==> %s\n' "$*"; }

[ $# -eq 1 ] || die "usage: seed.sh <version>"
VERSION="$1"
case "$VERSION" in *[!0-9A-Za-z.-]*|"") die "$VERSION is not a version." ;; esac
REL="$QA_ROOT/$VERSION/release.json"
[ -f "$REL" ] || die "no $REL: the operator runs prepare.sh first."

OPERATOR="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("operator", ""))' "$REL")"
[ -n "$OPERATOR" ] || die "$REL names no operator."
ME="$(whoami)"
[ "$ME" != "$OPERATOR" ] || die "this is the operator's macOS user ($ME). Run seed.sh as the separate test user, never as yourself."

# The operator's ClickUp token must not be reachable from this user's session (a keychain item named for the app).
if security find-generic-password -s "com.jtfrancisco.dcterminalv2.clickup" >/dev/null 2>&1; then
  die "the keychain holds a DCTerminalv2 ClickUp item: this is not a clean test user. Use a fresh macOS user."
fi

DATA="$HOME/Library/Application Support/com.jtfrancisco.dcterminalv2"
CLAUDE_DIR="$HOME/.claude-release-qa"
REPO="$HOME/dcv2-release-qa-repo"
SETTINGS="$DATA/settings.json"

if [ -f "$SETTINGS" ]; then
  python3 - "$SETTINGS" <<'PY' || exit 1
import json, sys
try:
    s = json.load(open(sys.argv[1]))
except Exception:
    sys.exit("seed.sh: settings.json is not readable JSON: this is not a clean test user.")
accounts = (s.get("claude") or {}).get("accounts") or []
others = [a.get("id") for a in accounts if a.get("id") != "release-qa"]
if others:
    sys.exit("seed.sh: the app data already holds other accounts (" + ", ".join(map(str, others)) + "): this is not a clean test user.")
if (s.get("clickup") or {}).get("autoPost"):
    sys.exit("seed.sh: the app data already holds ClickUp settings: this is not a clean test user.")
PY
fi
if [ -d "$DATA/run-trust" ] && [ -n "$(ls -A "$DATA/run-trust" 2>/dev/null)" ]; then
  die "the app data already holds ClickUp run records: this is not a clean test user."
fi

say "Seeding the test account, the test Claude config folder and the scratch repo"
mkdir -p "$DATA" "$CLAUDE_DIR"
chmod 0700 "$CLAUDE_DIR"
if [ ! -d "$REPO/.git" ]; then
  mkdir -p "$REPO"
  git -C "$REPO" init -q -b main
  printf '# Scratch repo\n\nA tiny repo for the release QA run.\n' > "$REPO/README.md"
  git -C "$REPO" add README.md
  git -C "$REPO" -c user.name="Release QA" -c user.email="release-qa@example.invalid" commit -q -m "Initial commit"
fi
python3 - "$SETTINGS" "$CLAUDE_DIR" <<'PY'
import json, os, sys
path, claude_dir = sys.argv[1:3]
s = {}
if os.path.exists(path):
    s = json.load(open(path))
s["schemaVersion"] = 1
s.setdefault("claude", {})["accounts"] = [{"id": "release-qa", "name": "Release QA", "configDir": claude_dir}]
tmp = path + ".tmp"
with open(tmp, "w") as f:
    json.dump(s, f, indent=2)
    f.write("\n")
os.replace(tmp, path)
PY
say "Done. Test account: Release QA ($CLAUDE_DIR). Scratch repo: $REPO."
say "Install the DMG from $QA_ROOT/$VERSION, sign in to Claude with a test account, and start the computer-use session (docs/release-qa.md)."
