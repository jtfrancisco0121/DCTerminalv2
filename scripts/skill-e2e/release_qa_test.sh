#!/usr/bin/env bash
# Hermetic test of scripts/release-qa/ (prepare.sh, seed.sh, check.sh): no Claude call, no network, no real build and no
# second macOS user. HOME, whoami, security, id, chmod (+a is recorded) and release-mac.sh are stubs on PATH or by variable;
# the root of the shared folder is a temp folder. macOS only (BSD stat, chmod +a); elsewhere it says so and passes.
set -uo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
if [ "$(uname -s)" != "Darwin" ]; then echo "skip: the release QA test runs on macOS only"; echo PASS; exit 0; fi
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
trap 'rm -rf "$d"' EXIT
unset DCV2_TEST_MODE DCV2_RUN_DIR DCV2_MEMORY_DIR || true

fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
check() { local label="$1"; shift; if "$@"; then ok "$label"; else bad "$label"; fi; }
has()   { if grep -qF -- "$2" <<<"$out"; then ok "$1"; else bad "$1: no '$2' in: ${out:0:500}"; fi; }

BIN="$d/bin"; mkdir -p "$BIN" "$d/home"
export HOME="$d/home"
export PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin"
export DCV2_RELEASE_QA_ROOT="$d/qa"
export STUB_ACL_LOG="$d/acl.log"
export DCV2_RELEASE_MAC="$BIN/release-mac.sh"

cat > "$BIN/whoami" <<'EOF'
#!/bin/sh
echo "${STUB_WHOAMI:-operator1}"
EOF
cat > "$BIN/security" <<'EOF'
#!/bin/sh
echo "security $*" >> "${STUB_ACL_LOG:-/dev/null}"
exit "${STUB_SECURITY_RC:-44}"
EOF
# chmod: +a is recorded (the hermetic test has no second macOS user to grant); everything else is the real chmod.
cat > "$BIN/chmod" <<'EOF'
#!/bin/sh
if [ "$1" = "+a" ]; then echo "chmod +a $2 | $3" >> "$STUB_ACL_LOG"; exit 0; fi
exec /bin/chmod "$@"
EOF
cat > "$BIN/id" <<'EOF'
#!/bin/sh
if [ "$1" = "-u" ] && [ -n "${STUB_ID_U:-}" ]; then echo "$STUB_ID_U"; exit 0; fi
exec /usr/bin/id "$@"
EOF
# release-mac.sh: no build; it "builds" a small DMG file and says where, the way the real script ends.
cat > "$BIN/release-mac.sh" <<'EOF'
#!/bin/sh
dmg="$STUB_DMG_DIR/DCTerminalv2_0.0.0_aarch64.dmg"
mkdir -p "$STUB_DMG_DIR"; printf 'dmg bytes' > "$dmg"
echo "==> Building the app"
echo "==> Done: $dmg (nothing was published; pass --publish to create the GitHub release)"
EOF
chmod 755 "$BIN"/*
export STUB_DMG_DIR="$d/dmgs"

VERSION="$(sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$REPO/package.json" | head -n 1)"
HEAD_SHA="$(git -C "$REPO" rev-parse HEAD)"
QA="$d/qa/$VERSION"
prep() { out="$(bash "$REPO/scripts/release-qa/prepare.sh" "$@" 2>&1)"; rc=$?; }
mode() { stat -f %Lp "$1"; }
fresh_root() { rm -rf "$d/qa"; : > "$STUB_ACL_LOG"; unset STUB_ID_U STUB_SECURITY_RC; export STUB_WHOAMI=operator1; }

echo "# prepare.sh (as the operator)"
fresh_root
prep --test-user qatester
check "prepare succeeds" [ "$rc" = 0 ]
check "  release.json names the version, HEAD, the operator and the test user" python3 - "$QA/release.json" "$VERSION" "$HEAD_SHA" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
assert d["version"] == sys.argv[2] and d["commit"] == sys.argv[3], d
assert d["operator"] == "operator1" and d["testUser"] == "qatester", d
assert len(d["dmg"]["sha256"]) == 64 and d["dmg"]["name"].endswith(".dmg"), d
PY
SHA="$(shasum -a 256 "$QA/DCTerminalv2_0.0.0_aarch64.dmg" | awk '{print $1}')"
check "  the recorded SHA-256 is the DMG's" grep -q "$SHA" "$QA/release.json"
check "  webqa-target.json holds the ten release charters" python3 - "$QA/webqa-target.json" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
ids = [c["id"] for c in t["charters"]]
assert len(ids) == 10 and "first-launch" in ids and "escape-everywhere" in ids, ids
PY
check "  the DMG, release.json and webqa-target.json are 0644" [ "$(mode "$QA/release.json")" = 644 ] && [ "$(mode "$QA/webqa-target.json")" = 644 ] && [ "$(mode "$QA/DCTerminalv2_0.0.0_aarch64.dmg")" = 644 ]
check "  the root, the version folder and webqa/ are 0755" [ "$(mode "$d/qa")" = 755 ] && [ "$(mode "$QA")" = 755 ] && [ "$(mode "$QA/webqa")" = 755 ]
check "  they belong to the operator" [ "$(stat -f %u "$d/qa")" = "$(id -u)" ] && [ "$(stat -f %u "$QA/release.json")" = "$(id -u)" ]
check "  exactly one ACL call was made" [ "$(grep -c '^chmod +a' "$STUB_ACL_LOG")" = 1 ]
out="$(cat "$STUB_ACL_LOG")"
has "  it is for the named test user on webqa/ only" "chmod +a user:qatester allow list,search,add_file,add_subdirectory,read,write,append,readattr,writeattr,readextattr,writeextattr,file_inherit,directory_inherit | $QA/webqa"
if grep -qiE "everyone|group:|user:operator1" "$STUB_ACL_LOG"; then bad "  the ACL names someone else"; else ok "  the ACL names no one else"; fi
check "  nothing but webqa/ is group- or world-writable" [ -z "$(find "$d/qa" -mindepth 0 -not -path "$QA/webqa" -perm -g+w -o -mindepth 0 -not -path "$QA/webqa" -perm -o+w)" ]

echo "## prepare.sh refusals"
fresh_root; prep --test-user operator1
check "the operator cannot be the test user" [ "$rc" != 0 ]; has "  says so" "must be a different macOS user"
fresh_root; prep --test-user "Bad Name"
check "a bad user name is refused" [ "$rc" != 0 ]
fresh_root; prep
check "no --test-user is refused" [ "$rc" != 0 ]
fresh_root; mkdir -p "$QA"; chmod 777 "$QA"
prep --test-user qatester
check "an existing world-writable <version>/ is refused" [ "$rc" != 0 ]; has "  says why" "writable by its group or by everyone"
check "  nothing was built or written" [ ! -e "$QA/release.json" ]
fresh_root; mkdir -p "$QA"; chmod 775 "$QA"
prep --test-user qatester
check "an existing group-writable <version>/ is refused" [ "$rc" != 0 ]
fresh_root; mkdir -p "$d/qa"; chmod 777 "$d/qa"
prep --test-user qatester
check "a world-writable root is refused" [ "$rc" != 0 ]; has "  names the root" "the release QA root"
fresh_root; mkdir -p "$d/qa"; chmod 755 "$d/qa"; export STUB_ID_U=99999
prep --test-user qatester
check "a root owned by another user is refused" [ "$rc" != 0 ]; has "  says so" "is not owned by you"
unset STUB_ID_U
fresh_root; mkdir -p "$d/qa" "$d/elsewhere"; chmod 755 "$d/qa" "$d/elsewhere"; ln -s "$d/elsewhere" "$QA"
prep --test-user qatester
check "a symlinked <version> is refused" [ "$rc" != 0 ]; has "  says so" "is a symlink"
check "  nothing was written through the link" [ -z "$(ls -A "$d/elsewhere")" ]
fresh_root; ln -s "$d/elsewhere" "$d/qa"
prep --test-user qatester
check "a symlinked root is refused" [ "$rc" != 0 ]; has "  says so" "is a symlink"
rm -f "$d/qa"

echo "# check.sh (as the operator)"
fresh_root; prep --test-user qatester
mkshots() { mkdir -p "$QA/webqa/shots"; for c in first-launch start-screen settings-roundtrip plain-tab role-tab eagle-eye-run run-panel trace receipt escape-everywhere; do printf 'png' > "$QA/webqa/shots/$c.png"; done; }
results() { # <failing charter id or ->: a results.md with every charter row, one of them FAIL
  { if [ "$1" = - ]; then echo "WEBQA: pass"; else echo "WEBQA: fail"; fi
    for c in first-launch start-screen settings-roundtrip plain-tab role-tab eagle-eye-run run-panel trace receipt escape-everywhere; do
      if [ "$c" = "$1" ]; then echo "$c | FAIL | it did not work | webqa/shots/$c.png"; else echo "$c | PASS | fine | webqa/shots/$c.png"; fi
    done; } > "$QA/webqa/results.md"
}
chk() { out="$(bash "$REPO/scripts/release-qa/check.sh" "$VERSION" 2>&1)"; rc=$?; }
mkshots; results -
chk
check "every row PASS prints pass" [ "$rc" = 0 ]; has "  says release QA pass" "release QA pass: $VERSION"
check "  webqa.json was written by the operator, in the version folder" [ -s "$QA/webqa.json" ]
results trace
chk
check "a FAIL row fails the check" [ "$rc" != 0 ]; has "  names the failing charter" "FAIL trace: it did not work"
results -; rm "$QA/webqa/shots/receipt.png"
chk
check "a missing screenshot fails the check" [ "$rc" != 0 ]; has "  names the screenshot" "webqa/shots/receipt.png is missing"
printf 'png' > "$QA/webqa/shots/receipt.png"; grep -v '^receipt ' "$QA/webqa/results.md" > "$d/r.md"; cp "$d/r.md" "$QA/webqa/results.md"
chk
check "a missing charter row fails the check" [ "$rc" != 0 ]; has "  names the charter" "charter receipt has no row"
results -
chmod 666 "$QA/release.json"; chk
check "a world-writable release.json is refused" [ "$rc" != 0 ]; has "  says why" "writable by its group or by everyone"
chmod 644 "$QA/release.json"; chmod 664 "$QA/webqa-target.json"; chk
check "a group-writable webqa-target.json is refused" [ "$rc" != 0 ]
chmod 644 "$QA/webqa-target.json"
export STUB_ID_U=99999; chk; unset STUB_ID_U
check "a release.json owned by someone else is refused" [ "$rc" != 0 ]; has "  says so" "is not owned by you"
export STUB_WHOAMI=qatester; chk; export STUB_WHOAMI=operator1
check "check.sh refuses as the test user" [ "$rc" != 0 ]; has "  says so" "this is the test user (qatester)"
mv "$QA/webqa-target.json" "$d/t.json"; ln -s "$d/t.json" "$QA/webqa-target.json"; chk
check "a symlinked webqa-target.json is refused" [ "$rc" != 0 ]; has "  says so" "is a symlink"
rm "$QA/webqa-target.json"; mv "$d/t.json" "$QA/webqa-target.json"
out="$(bash "$REPO/scripts/release-qa/check.sh" 9.9.9 2>&1)"; rc=$?
check "an unprepared version is refused" [ "$rc" != 0 ]

echo "# seed.sh (as the test user)"
fresh_root; prep --test-user qatester
seed() { out="$(bash "$REPO/scripts/release-qa/seed.sh" "$VERSION" 2>&1)"; rc=$?; }
export STUB_WHOAMI=operator1
seed
check "seed refuses as the operator" [ "$rc" != 0 ]; has "  says so" "operator's macOS user"
check "  and touched nothing" [ ! -e "$HOME/.claude-release-qa" ] && [ ! -e "$HOME/dcv2-release-qa-repo" ]
export STUB_WHOAMI=qatester STUB_SECURITY_RC=0
seed
check "seed refuses when the keychain holds a DCTerminalv2 ClickUp item" [ "$rc" != 0 ]; has "  says so" "keychain holds a DCTerminalv2 ClickUp item"
check "  and touched nothing" [ ! -e "$HOME/.claude-release-qa" ] && [ ! -e "$HOME/dcv2-release-qa-repo" ]
export STUB_SECURITY_RC=44
DATA="$HOME/Library/Application Support/com.jtfrancisco.dcterminalv2"; mkdir -p "$DATA"
printf '{"schemaVersion":1,"claude":{"accounts":[{"id":"default","name":"Personal"}]}}\n' > "$DATA/settings.json"
seed
check "seed refuses when the app data already holds another account" [ "$rc" != 0 ]; has "  names it" "other accounts (default)"
check "  and left settings.json alone" grep -q '"Personal"' "$DATA/settings.json"
printf '{"schemaVersion":1,"claude":{"accounts":[]},"clickup":{"autoPost":{"default":true}}}\n' > "$DATA/settings.json"
seed
check "seed refuses when the app data holds ClickUp settings" [ "$rc" != 0 ]
rm -f "$DATA/settings.json"; mkdir -p "$DATA/run-trust/r-x"; : > "$DATA/run-trust/r-x/posted.json"
seed
check "seed refuses when the app data holds ClickUp run records" [ "$rc" != 0 ]
rm -rf "$DATA/run-trust"
seed
check "seed succeeds for a clean test user" [ "$rc" = 0 ]
check "  settings.json has the one test account" python3 - "$DATA/settings.json" "$HOME/.claude-release-qa" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
assert s["schemaVersion"] == 1, s
assert s["claude"]["accounts"] == [{"id": "release-qa", "name": "Release QA", "configDir": sys.argv[2]}], s
PY
check "  the test Claude config folder is private" [ "$(mode "$HOME/.claude-release-qa")" = 700 ]
check "  the scratch repo has a commit" git -C "$HOME/dcv2-release-qa-repo" rev-parse HEAD
seed
check "  seeding again is fine" [ "$rc" = 0 ]

if [ "$fails" = 0 ]; then echo "PASS"; else echo "FAILED"; exit 1; fi
