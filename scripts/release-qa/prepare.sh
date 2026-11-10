#!/usr/bin/env bash
# Release QA, step 1 (as the OPERATOR, never the test user): build the DMG and make the shared folder the test session reads.
#
#   scripts/release-qa/prepare.sh --test-user <macOS user name> [--universal]
#
# Builds the DMG with scripts/release-mac.sh, then writes /Users/Shared/dcv2-release-qa/<version>/ (DCV2_RELEASE_QA_ROOT
# replaces the root, for tests) with the DMG, webqa-target.json (from scripts/release-qa/charters.json) and release.json
# (version, commit, DMG SHA-256, operator, test user). bash 3.2, BSD tools, umask 022.
#
# Ownership: dcv2-release-qa/ and <version>/ are operator-owned 0755; the DMG, release.json and webqa-target.json are
# operator-owned 0644. The one place the test user can write is <version>/webqa/ (with shots/ and steps/): operator-owned
# 0755 plus ONE ACL entry for the named test user only. Never world- or group-writable. /Users/Shared is sticky, so the
# test user cannot remove or replace the operator's folders. Refuses a root or <version>/ that is a symlink, is not
# operator-owned or is group/world-writable.
set -euo pipefail
umask 022

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
QA_ROOT="${DCV2_RELEASE_QA_ROOT:-/Users/Shared/dcv2-release-qa}"
BUILD="${DCV2_RELEASE_MAC:-$ROOT/scripts/release-mac.sh}"

die() { printf 'prepare.sh: %s\n' "$*" >&2; exit 1; }
say() { printf '==> %s\n' "$*"; }

TEST_USER=""
BUILD_ARGS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --test-user) [ $# -ge 2 ] || die "--test-user needs a macOS user name."; TEST_USER="$2"; shift 2 ;;
    --universal) BUILD_ARGS="--universal"; shift ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done
[ -n "$TEST_USER" ] || die "usage: prepare.sh --test-user <macOS user name>"
case "$TEST_USER" in *[!a-z0-9_-]*|[!a-z_]*) die "$TEST_USER is not a macOS short user name (lower case letters, digits, - and _)." ;; esac

OPERATOR="$(whoami)"
[ "$OPERATOR" != "$TEST_USER" ] || die "the test user must be a different macOS user than you ($OPERATOR): the test session must not run as you."

json_version() { sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$1" | head -n 1; }

# A folder that is the operator's and that nobody else can write. A link is refused before anything is read.
safe_node() { # <path> <what>
  local p="$1" what="$2" mode
  [ ! -L "$p" ] || die "$what ($p) is a symlink."
  [ "$(stat -f %u "$p")" = "$(id -u)" ] || die "$what ($p) is not owned by you."
  mode="$(stat -f %Lp "$p")"
  case "${mode: -2}" in *[2367]?|?[2367]) die "$what ($p) is writable by its group or by everyone (mode $mode)." ;; esac
}

VERSION="$(json_version "$ROOT/package.json")"
[ -n "$VERSION" ] || die "no version in package.json."
COMMIT="$(git -C "$ROOT" rev-parse HEAD)" || die "git rev-parse HEAD failed."
DIR="$QA_ROOT/$VERSION"

# Refusals before anything is built.
if [ -e "$QA_ROOT" ] || [ -L "$QA_ROOT" ]; then safe_node "$QA_ROOT" "the release QA root"; fi
if [ -e "$DIR" ] || [ -L "$DIR" ]; then safe_node "$DIR" "the release QA folder for $VERSION"; fi

say "Building the DMG with release-mac.sh"
OUT="$("$BUILD" $BUILD_ARGS 2>&1)" || { printf '%s\n' "$OUT" >&2; die "release-mac.sh failed."; }
printf '%s\n' "$OUT"
DMG="$(printf '%s\n' "$OUT" | sed -n 's/^==> Done: \(.*\.dmg\) (.*$/\1/p' | tail -n 1)"
[ -n "$DMG" ] && [ -f "$DMG" ] || die "release-mac.sh did not say which DMG it built."

if [ ! -e "$QA_ROOT" ]; then mkdir -p "$QA_ROOT"; chmod 0755 "$QA_ROOT"; safe_node "$QA_ROOT" "the release QA root"; fi
if [ ! -e "$DIR" ]; then mkdir "$DIR"; chmod 0755 "$DIR"; fi
safe_node "$DIR" "the release QA folder for $VERSION"

# Files first, then the one folder the test user can write.
cp "$DMG" "$DIR/$(basename "$DMG")"
chmod 0644 "$DIR/$(basename "$DMG")"
SHA="$(shasum -a 256 "$DIR/$(basename "$DMG")" | awk '{ print $1 }')"
python3 - "$DIR" "$HERE/charters.json" "$VERSION" "$COMMIT" "$(basename "$DMG")" "$SHA" "$OPERATOR" "$TEST_USER" <<'PY'
import json, sys, time
d, charters, version, commit, dmg, sha, operator, test_user = sys.argv[1:9]
rel = {"schema": 1, "version": version, "commit": commit, "dmg": {"name": dmg, "sha256": sha},
       "operator": operator, "testUser": test_user, "preparedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
target = {"schema": 1, "id": "release-qa", "name": "DCTerminalv2 " + version + " (installed app)", "kind": "installed-app",
          "allowlist": [], "charters": json.load(open(charters))["charters"], "clickupListId": None}
for name, doc in (("release.json", rel), ("webqa-target.json", target)):
    with open(d + "/" + name, "w") as f:
        json.dump(doc, f, indent=2)
        f.write("\n")
PY
chmod 0644 "$DIR/release.json" "$DIR/webqa-target.json"
rm -f "$DIR/webqa.json"

# The one folder the test user can write: webqa/ holds the results.md, shots/ and steps/ the session makes. The ACL entry is
# inherited by what the test user creates there; nothing else in <version>/ or above it is writable by anyone but you.
mkdir -p "$DIR/webqa"
chmod 0755 "$DIR/webqa"
ACL="user:$TEST_USER allow list,search,add_file,add_subdirectory,read,write,append,readattr,writeattr,readextattr,writeextattr,file_inherit,directory_inherit"
chmod +a "$ACL" "$DIR/webqa" || die "could not give $TEST_USER write access to $DIR/webqa (chmod +a)."

say "Release QA folder ready: $DIR"
say "DMG SHA-256: $SHA"
say "Next: as $TEST_USER run scripts/release-qa/seed.sh $VERSION, install the DMG, start the computer-use session (docs/release-qa.md), then as you: scripts/release-qa/check.sh $VERSION"
