#!/usr/bin/env bash
# Release QA, step 3 (as the OPERATOR): check the test session's results.
#
#   scripts/release-qa/check.sh <version>
#
# Refuses as the test user named in release.json. First checks that the root dcv2-release-qa/, <version>/, release.json and
# webqa-target.json are owned by you, not group/world-writable and not links (a link is tested before stat), then runs
# plugin/scripts/state.sh webqa on the folder (results in webqa/results.md and webqa/shots/) and prints "release QA pass",
# or the failing release charters. Exit 0 only when every charter row is PASS. webqa.json is written by state.sh as you,
# where the test user cannot write. bash 3.2, BSD tools.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
QA_ROOT="${DCV2_RELEASE_QA_ROOT:-/Users/Shared/dcv2-release-qa}"
STATE="$ROOT/plugin/scripts/state.sh"
die() { printf 'check.sh: %s\n' "$*" >&2; exit 1; }

[ $# -eq 1 ] || die "usage: check.sh <version>"
VERSION="$1"
case "$VERSION" in *[!0-9A-Za-z.-]*|"") die "$VERSION is not a version." ;; esac
DIR="$QA_ROOT/$VERSION"

safe_node() { # <path> <what>
  local p="$1" what="$2" mode
  [ ! -L "$p" ] || die "$what ($p) is a symlink."
  [ -e "$p" ] || die "$what ($p) does not exist."
  [ "$(stat -f %u "$p")" = "$(id -u)" ] || die "$what ($p) is not owned by you."
  mode="$(stat -f %Lp "$p")"
  case "${mode: -2}" in *[2367]?|?[2367]) die "$what ($p) is writable by its group or by everyone (mode $mode)." ;; esac
}
safe_node "$QA_ROOT" "the release QA root"
safe_node "$DIR" "the release QA folder for $VERSION"
safe_node "$DIR/release.json" "release.json"
safe_node "$DIR/webqa-target.json" "webqa-target.json"

TEST_USER="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("testUser", ""))' "$DIR/release.json")"
REL_VERSION="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("version", ""))' "$DIR/release.json")"
[ "$REL_VERSION" = "$VERSION" ] || die "release.json is for version ${REL_VERSION:-none}, not $VERSION."
[ "$(whoami)" != "$TEST_USER" ] || die "this is the test user ($TEST_USER): the operator runs check.sh, never the test session."

OUT=""
if ! OUT="$(bash "$STATE" webqa "$DIR" 2>&1)"; then
  printf '%s\n' "$OUT" >&2
  die "release QA did not pass: the results are missing or not valid."
fi
printf '%s\n' "$OUT"
case "$OUT" in
  "webqa=pass "*) printf 'release QA pass: %s\n' "$VERSION" ;;
  *) die "release QA did not pass for $VERSION (the failing release charters are above)." ;;
esac
