#!/usr/bin/env bash
# Hermetic test of scripts/release-mac.sh (no Claude call, no network, no real build): npx, cargo,
# codesign, hdiutil, lipo, gh and uname are stubs on PATH; git is real, against a temp repo with a
# local bare "origin". macOS only (the script uses PlistBuddy, ditto and hdiutil); elsewhere it says
# so and passes. Part of `npm run check` on macOS.
set -uo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
if [ "$(uname -s)" != "Darwin" ]; then echo "skip: the release script test runs on macOS only"; exit 0; fi
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
trap 'rm -rf "$d"' EXIT
unset DCV2_TEST_MODE DCV2_RUN_DIR DCV2_MEMORY_DIR || true

fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
# check <label> <command...>: ok when the command succeeds
check() { local label="$1"; shift; if "$@"; then ok "$label"; else bad "$label"; fi; }
has()   { if grep -qF -- "$2" <<<"$out"; then ok "$1"; else bad "$1: no '$2' in: ${out:0:400}"; fi; }

# Loads the functions only (main does not run when sourced); then point ROOT at a temp repo.
# shellcheck disable=SC1091
. "$REPO/scripts/release-mac.sh"
set +e
set -o pipefail

W="$d/work"; R="$W/repo"; ORIGIN="$W/origin.git"; BIN="$d/bin"; LOG="$d/calls.log"
mkdir -p "$BIN" "$W"
# No real gh, npx or cargo can be reached: only system tools, after the stubs.
export PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin"
ROOT="$R"
export HOME="$d/home"; mkdir -p "$HOME"

write_stubs() {
  cat > "$BIN/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo Darwin ;; -m) echo "${STUB_ARCH:-arm64}" ;; *) echo Darwin ;; esac
EOF
  # npx tauri build --bundles app [--target T]: makes the bundle where Tauri would.
  cat > "$BIN/npx" <<'EOF'
#!/bin/sh
echo "npx $*" >> "$STUB_LOG"
touch "$STUB_DIR/built"
[ "${STUB_BUILD_FAIL:-}" = 1 ] && exit 1
target=""; prev=""
for a in "$@"; do [ "$prev" = "--target" ] && target="$a"; prev="$a"; done
if [ -n "$target" ]; then base="src-tauri/target/$target/release/bundle/macos"; else base="src-tauri/target/release/bundle/macos"; fi
app="$base/DCTerminalv2.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/plugin/.claude-plugin"
cat > "$app/Contents/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>dcterminalv2</string></dict></plist>
PL
printf 'main %s\n' "${STUB_MAIN_ARCHS:-arm64}" > "$app/Contents/MacOS/dcterminalv2"; chmod 755 "$app/Contents/MacOS/dcterminalv2"
[ "${STUB_BUNDLE_HOOK:-}" = 1 ] && { printf 'hook %s\n' "${STUB_HOOK_ARCHS:-arm64}" > "$app/Contents/MacOS/dcv2-hook"; chmod 755 "$app/Contents/MacOS/dcv2-hook"; }
exit 0
EOF
  # cargo build --release --manifest-path <M> --bin dcv2-hook [--target T]
  cat > "$BIN/cargo" <<'EOF'
#!/bin/sh
echo "cargo $*" >> "$STUB_LOG"
mp=""; target=""; prev=""
for a in "$@"; do [ "$prev" = "--manifest-path" ] && mp="$a"; [ "$prev" = "--target" ] && target="$a"; prev="$a"; done
dir="$(dirname "$mp")/target"
if [ -n "$target" ]; then out="$dir/$target/release"; else out="$dir/release"; fi
mkdir -p "$out"
case "$target" in aarch64-apple-darwin) arch=arm64 ;; x86_64-apple-darwin) arch=x86_64 ;; *) arch="${STUB_HOOK_ARCHS:-arm64}" ;; esac
printf 'hook %s\n' "$arch" > "$out/dcv2-hook"; chmod 755 "$out/dcv2-hook"
exit 0
EOF
  # codesign: --verify fails for a path matching STUB_VERIFY_FAIL (built: the build folder; mounted: anywhere else).
  cat > "$BIN/codesign" <<'EOF'
#!/bin/sh
echo "codesign $*" >> "$STUB_LOG"
if [ "$1" = "--verify" ]; then
  for last; do :; done
  case "${STUB_VERIFY_FAIL:-}" in
    built) case "$last" in */src-tauri/target/*) exit 1 ;; esac ;;
    mounted) case "$last" in */src-tauri/target/*) ;; *) exit 1 ;; esac ;;
  esac
fi
exit 0
EOF
  # hdiutil create/attach/detach, with a "mounted" marker so a test can see what is left attached.
  cat > "$BIN/hdiutil" <<'EOF'
#!/bin/sh
echo "hdiutil $*" >> "$STUB_LOG"
verb="$1"; shift
case "$verb" in
  create)
    src=""; prev=""; for a in "$@"; do [ "$prev" = "-srcfolder" ] && src="$a"; prev="$a"; last="$a"; done
    mkdir -p "$last.contents"; cp -R "$src/." "$last.contents/"; echo dmg > "$last" ;;
  attach)
    mp=""; prev=""; for a in "$@"; do [ "$prev" = "-mountpoint" ] && mp="$a"; prev="$a"; last="$a"; done
    cp -R "$last.contents/." "$mp/"; echo "$mp" >> "$STUB_DIR/mounted" ;;
  detach)
    for a in "$@"; do case "$a" in -*) ;; *) mp="$a" ;; esac; done
    rm -rf "$mp"/* 2>/dev/null; grep -vxF "$mp" "$STUB_DIR/mounted" > "$STUB_DIR/mounted.new" 2>/dev/null; mv "$STUB_DIR/mounted.new" "$STUB_DIR/mounted" 2>/dev/null ;;
esac
exit 0
EOF
  # lipo -archs <file>: the architectures the stub binaries were written with ("main arm64 x86_64").
  cat > "$BIN/lipo" <<'EOF'
#!/bin/sh
echo "lipo $*" >> "$STUB_LOG"
case "$1" in
  -archs) sed -n '1s/^[a-z]* //p' "$2" ;;
  -create) out=""; prev=""; for a in "$@"; do [ "$prev" = "-output" ] && out="$a"; prev="$a"; done
           archs=""; for a in "$@"; do case "$a" in -*|"$out") ;; *) archs="$archs $(sed -n '1s/^[a-z]* //p' "$a")" ;; esac; done
           [ "${STUB_LIPO_BROKEN:-}" = 1 ] && archs=" arm64"
           printf 'hook%s\n' "$archs" > "$out" ;;
esac
exit 0
EOF
  cat > "$BIN/rustup" <<'EOF'
#!/bin/sh
echo "rustup $*" >> "$STUB_LOG"
if [ "$1 $2 $3" = "target list --installed" ]; then
  printf '%s\n' "${STUB_RUSTUP_TARGETS-aarch64-apple-darwin
x86_64-apple-darwin}"
fi
exit 0
EOF
  cat > "$BIN/gh" <<'EOF'
#!/bin/sh
echo "gh $*" >> "$STUB_LOG"
case "$1 $2" in
  "repo view") echo "${STUB_GH_BRANCH:-main}" ;;
  "release create") touch "$STUB_DIR/released" ;;
esac
exit 0
EOF
  # id -u: the real answer unless STUB_ID_U names another user (the release QA check compares owners with it).
  cat > "$BIN/id" <<'EOF'
#!/bin/sh
if [ "$1" = "-u" ] && [ -n "${STUB_ID_U:-}" ]; then echo "$STUB_ID_U"; exit 0; fi
exec /usr/bin/id "$@"
EOF
  chmod 755 "$BIN"/*
}
write_stubs
export STUB_LOG="$LOG" STUB_DIR="$d"
export DCV2_RELEASE_QA_ROOT="$d/qa"

# A temp repo that looks like the real one for the script: versions, a bare origin, one pushed commit.
fresh() {
  rm -rf "$W/repo" "$ORIGIN" "$W/other" "$LOG" "$d/built" "$d/mounted" "$d/released"; : > "$LOG"
  unset STUB_ID_U STUB_ARCH STUB_VERIFY_FAIL STUB_BUILD_FAIL STUB_BUNDLE_HOOK STUB_MAIN_ARCHS STUB_HOOK_ARCHS STUB_GH_BRANCH STUB_LIPO_BROKEN STUB_RUSTUP_TARGETS
  mkdir -p "$R/src-tauri" "$R/plugin/.claude-plugin"
  printf '{\n  "name": "x",\n  "version": "1.2.3"\n}\n' > "$R/package.json"
  printf '{\n  "productName": "DCTerminalv2",\n  "version": "1.2.3",\n  "bundle": {}\n}\n' > "$R/src-tauri/tauri.conf.json"
  printf '[package]\nname = "x"\nversion = "1.2.3"\n\n[dependencies]\nserde = { version = "1" }\n' > "$R/src-tauri/Cargo.toml"
  printf '0.9.0\n' > "$R/plugin/VERSION"
  printf '{\n  "name": "dcv2",\n  "version": "0.9.0"\n}\n' > "$R/plugin/.claude-plugin/plugin.json"
  printf 'src-tauri/target/\n' > "$R/.gitignore"
  mkdir -p "$R/scripts" "$R/plugin/scripts"
  cp -R "$REPO/scripts/release-qa" "$R/scripts/release-qa"
  cp "$REPO"/plugin/scripts/state.sh "$R/plugin/scripts/"; cp -R "$REPO/plugin/scripts/lib" "$R/plugin/scripts/lib"
  rm -rf "$d/qa"
  git init -q -b main "$R"
  git -C "$R" config user.name t; git -C "$R" config user.email t@t
  git -C "$R" add -A; git -C "$R" commit -q -m init
  git init -q --bare -b main "$ORIGIN"
  git -C "$R" remote add origin "$ORIGIN"; git -C "$R" push -q origin main
}
commit_all() { git -C "$R" add -A; git -C "$R" commit -q -m "$1"; }
# qa_make <pass|fail>: a release QA folder for 1.2.3 at the current HEAD, the way prepare.sh and the test session leave it.
qa_make() {
  local head D="$d/qa/1.2.3"
  head="$(git -C "$R" rev-parse HEAD)"
  rm -rf "$d/qa"; mkdir -p "$D/webqa/shots"; chmod 755 "$d/qa" "$D" "$D/webqa" "$D/webqa/shots"
  printf '{"schema":1,"version":"1.2.3","commit":"%s","operator":"op","testUser":"qatester"}\n' "$head" > "$D/release.json"
  printf '{"schema":1,"id":"release-qa","name":"x","allowlist":[],"charters":[{"id":"first-launch","title":"First launch","text":"t","mayChangeData":false},{"id":"settings-roundtrip","title":"Settings round-trip","text":"t","mayChangeData":false}]}\n' > "$D/webqa-target.json"
  printf 'png' > "$D/webqa/shots/first-launch.png"; printf 'png' > "$D/webqa/shots/settings-roundtrip.png"
  if [ "$1" = pass ]; then
    printf 'WEBQA: pass\nfirst-launch | PASS | the start screen showed | webqa/shots/first-launch.png\nsettings-roundtrip | PASS | saved and reloaded | webqa/shots/settings-roundtrip.png\n' > "$D/webqa/results.md"
  else
    printf 'WEBQA: fail\nfirst-launch | PASS | the start screen showed | webqa/shots/first-launch.png\nsettings-roundtrip | FAIL | Escape did nothing | webqa/shots/settings-roundtrip.png\n' > "$D/webqa/results.md"
  fi
  chmod 644 "$D/release.json" "$D/webqa-target.json" "$D/webqa/results.md" "$D/webqa/shots/"*.png
}
# run_main <args...>: main in a subshell, so its exits and its EXIT trap stay there; sets $out and $rc
run_main() { out="$(main "$@" 2>&1)"; rc=$?; }
not_built() { [ ! -e "$d/built" ]; }
calls() { cat "$LOG"; }
DMG="$R/src-tauri/target/release/bundle/dmg/DCTerminalv2_1.2.3_aarch64.dmg"

echo "# refusals come before any build"
fresh; touch "$R/stray.txt"
run_main
check "a dirty tree (an untracked file) is refused" [ "$rc" != 0 ]; has "  says the tree is not clean" "not clean"; check "  nothing was built" not_built
fresh; echo "x" >> "$R/package.json"
run_main
check "a modified tracked file is refused" [ "$rc" != 0 ]; has "  says so" "not clean"

fresh; sed -i '' 's/"version": "1.2.3"/"version": "1.2.4"/' "$R/src-tauri/tauri.conf.json"; commit_all mismatch
run_main
check "package.json vs tauri.conf.json is refused" [ "$rc" != 0 ]; has "  names the files" "package.json 1.2.3, src-tauri/tauri.conf.json 1.2.4"; check "  nothing was built" not_built
fresh; sed -i '' 's/^version = "1.2.3"/version = "1.2.9"/' "$R/src-tauri/Cargo.toml"; commit_all mismatch
run_main
check "Cargo.toml's [package] version mismatch is refused" [ "$rc" != 0 ]; has "  names Cargo.toml" "src-tauri/Cargo.toml 1.2.9"; check "  nothing was built" not_built
fresh; sed -i '' 's/1\.2\.3/1.2.3/; s/serde = { version = "1" }/serde = { version = "2" }/' "$R/src-tauri/Cargo.toml"; commit_all "dependency versions are not the package version"
run_main
check "a dependency's version is not mistaken for the package version" [ "$rc" = 0 ]
fresh; printf '0.9.1\n' > "$R/plugin/VERSION"; commit_all mismatch
run_main
check "plugin/VERSION vs plugin.json is refused" [ "$rc" != 0 ]; has "  names the plugin files" "plugin/VERSION 0.9.1, plugin/.claude-plugin/plugin.json 0.9.0"; check "  nothing was built" not_built

fresh; export STUB_ARCH=x86_64
run_main
check "a non-arm64 host without --universal is refused" [ "$rc" != 0 ]; has "  says why" "labelled aarch64"; check "  nothing was built" not_built

echo "# --publish refusals"
fresh; git -C "$R" tag v1.2.3
run_main --publish
check "--publish with an existing local tag is refused" [ "$rc" != 0 ]; has "  names the tag" "v1.2.3 already exists locally"; check "  nothing was built" not_built
fresh; git -C "$R" tag v1.2.3; git -C "$R" push -q origin v1.2.3; git -C "$R" tag -d v1.2.3 >/dev/null
run_main --publish
check "--publish with an existing remote tag is refused" [ "$rc" != 0 ]; has "  names the tag" "v1.2.3 already exists on origin"; check "  nothing was built" not_built
fresh; rm -f "$BIN/gh"
run_main --publish
check "--publish without gh is refused" [ "$rc" != 0 ]; has "  says gh is missing" "needs gh"; check "  nothing was built" not_built
write_stubs
fresh; git clone -q "$ORIGIN" "$W/other"; git -C "$W/other" config user.name t; git -C "$W/other" config user.email t@t
echo more > "$W/other/more.txt"; git -C "$W/other" add -A; git -C "$W/other" commit -q -m ahead; git -C "$W/other" push -q origin main
run_main --publish
check "--publish with HEAD behind the remote's default-branch tip is refused" [ "$rc" != 0 ]; has "  says HEAD is not the tip" "is not the tip of origin/main"; check "  nothing was built" not_built
check "  no release was created" [ ! -e "$d/released" ]
fresh; echo ahead > "$R/ahead.txt"; commit_all "local only"
run_main --publish
check "--publish with HEAD ahead of the remote is refused too" [ "$rc" != 0 ]; has "  says HEAD is not the tip" "is not the tip of origin/main"; check "  nothing was built" not_built

echo "# a normal build"
fresh
run_main
check "a clean arm64 run builds" [ "$rc" = 0 ]
check "  the DMG is named for the version and aarch64" [ -f "$DMG" ]
check "  dcv2-hook was added to the app and the DMG holds it" [ -x "$DMG.contents/DCTerminalv2.app/Contents/MacOS/dcv2-hook" ]
check "  the binary path comes from CFBundleExecutable (lowercase)" [ "$(bundle_binary "$R/src-tauri/target/release/bundle/macos/DCTerminalv2.app")" = "$R/src-tauri/target/release/bundle/macos/DCTerminalv2.app/Contents/MacOS/dcterminalv2" ]
check "  the DMG has the Applications link" [ -L "$DMG.contents/Applications" ]
calls | grep -q "codesign --force --deep --sign - " && ok "  signed ad hoc" || bad "  no ad hoc signing: $(calls)"
calls | grep -q "codesign --verify --deep --strict " && ok "  verified with --deep --strict" || bad "  no verify"
calls | grep -q "hdiutil attach -readonly -nobrowse" && ok "  mounted read-only, nobrowse" || bad "  no read-only mount"
check "  the volume is detached afterwards" [ ! -s "$d/mounted" ]
sum="$(shasum -a 256 "$DMG" | awk '{ print $1 }')"
has "  the printed SHA-256 equals shasum -a 256" "SHA-256: $sum"
check "  the .sha256 file holds it" [ "$(cat "$DMG.sha256")" = "$sum  DCTerminalv2_1.2.3_aarch64.dmg" ]
check "  without --publish gh was never called" [ ! -e "$d/released" ] ; ! calls | grep -q "^gh " && ok "  and the gh stub recorded no call at all" || bad "  gh was called: $(calls | grep '^gh ')"
check "  the default build stays aarch64: no --target, no rustup, no lipo" [ -z "$(calls | grep -E '^(rustup|lipo) |--target')" ]
check "  the order: build, hook, then the signature" [ "$(calls | grep -n -m1 '^npx ' | cut -d: -f1)" -lt "$(calls | grep -n -m1 '^codesign --force' | cut -d: -f1)" ]

echo "# --universal"
UDMG="$R/src-tauri/target/universal-apple-darwin/release/bundle/dmg/DCTerminalv2_1.2.3_universal.dmg"
UAPP="$R/src-tauri/target/universal-apple-darwin/release/bundle/macos/DCTerminalv2.app"
fresh; export STUB_RUSTUP_TARGETS="aarch64-apple-darwin"
run_main --universal
check "a missing Rust target stops the script" [ "$rc" != 0 ]; has "  prints the exact rustup line" "rustup target add x86_64-apple-darwin"; check "  nothing was built" not_built
fresh; export STUB_RUSTUP_TARGETS=""
run_main --universal
check "no installed Apple target: both are named" [ "$rc" != 0 ]; has "  prints the rustup line" "rustup target add aarch64-apple-darwin x86_64-apple-darwin"; check "  nothing was built" not_built
fresh; export STUB_MAIN_ARCHS="arm64 x86_64" STUB_BUNDLE_HOOK=1 STUB_HOOK_ARCHS=arm64 STUB_ARCH=x86_64
run_main --universal
check "--universal builds on an Intel host, with a single-arch bundled hook" [ "$rc" = 0 ]
check "  the DMG is named universal" [ -f "$UDMG" ]
calls | grep -q "^npx tauri build --bundles app --target universal-apple-darwin" && ok "  tauri built the universal target" || bad "  calls: $(calls | grep '^npx')"
calls | grep -q "^cargo build --release .* --bin dcv2-hook --target aarch64-apple-darwin" && calls | grep -q "^cargo build --release .* --bin dcv2-hook --target x86_64-apple-darwin" && ok "  dcv2-hook was built for both targets" || bad "  calls: $(calls | grep '^cargo')"
calls | grep -q "^lipo -create -output $UAPP/Contents/MacOS/dcv2-hook " && ok "  the single-arch hook was joined with lipo -create" || bad "  calls: $(calls | grep '^lipo')"
check "  the bundled hook now lists both architectures" [ "$(lipo -archs "$UAPP/Contents/MacOS/dcv2-hook")" = "arm64 x86_64" ]
check "  the app binary path is the lowercase CFBundleExecutable one" [ "$(bundle_binary "$UAPP")" = "$UAPP/Contents/MacOS/dcterminalv2" ]
lipo_line="$(calls | grep -n -m1 '^lipo -create' | cut -d: -f1)"; sign_line="$(calls | grep -n -m1 '^codesign --force' | cut -d: -f1)"
check "  it signed after the lipo" [ "$lipo_line" -lt "$sign_line" ]
check "  the volume is detached" [ ! -s "$d/mounted" ]
fresh; export STUB_MAIN_ARCHS="arm64 x86_64" STUB_BUNDLE_HOOK=1 STUB_HOOK_ARCHS="arm64 x86_64"
run_main --universal
check "a bundled hook that is already universal is not joined again" [ "$rc" = 0 ]; check "  no lipo -create" [ -z "$(calls | grep '^lipo -create')" ]
fresh; export STUB_MAIN_ARCHS="arm64"
run_main --universal
check "an app binary with one architecture fails the script" [ "$rc" != 0 ]; has "  says which" "the app binary is not universal"; check "  no DMG was made" [ ! -e "$UDMG" ]
fresh; export STUB_MAIN_ARCHS="arm64 x86_64" STUB_LIPO_BROKEN=1
run_main --universal
check "a dcv2-hook with one architecture after lipo fails the script" [ "$rc" != 0 ]; has "  says which" "dcv2-hook is not universal"; check "  no DMG was made" [ ! -e "$UDMG" ]
fresh; export STUB_MAIN_ARCHS="arm64 x86_64" STUB_VERIFY_FAIL=mounted
run_main --universal
check "a bad seal on the universal DMG's app stops the script, detached" [ "$rc" != 0 ]; check "  nothing is left mounted" [ ! -s "$d/mounted" ]

echo "# a bad seal stops the script"
fresh; export STUB_VERIFY_FAIL=mounted
run_main
check "a failing verify of the app inside the DMG stops the script" [ "$rc" != 0 ]; has "  says which app" "the app inside the DMG"
check "  the mounted volume was detached" [ ! -s "$d/mounted" ]
calls | grep -q "hdiutil detach" && ok "  detach was called" || bad "  no detach: $(calls)"
check "  no SHA file" [ ! -e "$DMG.sha256" ]
fresh; qa_make pass; export STUB_VERIFY_FAIL=mounted
run_main --publish
check "  and --publish creates nothing" [ "$rc" != 0 ] ; check "  no release" [ ! -e "$d/released" ]
fresh; export STUB_VERIFY_FAIL=built
run_main
check "a failing verify of the built app stops before the DMG" [ "$rc" != 0 ]; check "  no DMG was made" [ ! -e "$DMG" ]
fresh; export STUB_BUILD_FAIL=1
run_main
check "a failing build stops the script" [ "$rc" != 0 ]; has "  says so" "build failed"

echo "# --publish from the built commit"
fresh; qa_make pass
run_main --publish
check "--publish from a HEAD that is the remote tip succeeds" [ "$rc" = 0 ]
head="$(git -C "$R" rev-parse HEAD)"
calls | grep -q "^gh release create v1.2.3 --target $head " && ok "  gh release create records --target <HEAD sha>" || bad "  gh calls: $(calls | grep '^gh ')"
calls | grep "^gh release create" | grep -q "$DMG $DMG.sha256" && ok "  the DMG and its .sha256 are uploaded" || bad "  gh calls: $(calls | grep '^gh ')"
fresh; qa_make pass
git clone -q "$ORIGIN" "$W/other"; git -C "$W/other" config user.name t; git -C "$W/other" config user.email t@t
# The remote tip moves between the first check and the upload: publish_release re-reads it and refuses.
require_head_is_remote_tip
echo moved > "$W/other/moved.txt"; git -C "$W/other" add -A; git -C "$W/other" commit -q -m moved; git -C "$W/other" push -q origin main
out="$(publish_release v1.2.3 "$d/none.dmg" 2>&1)"; rc=$?
check "the remote tip is read again before upload: a moved tip refuses" [ "$rc" != 0 ]; has "  says HEAD is not the tip" "is not the tip of origin/main"; check "  no release was created" [ ! -e "$d/released" ]


echo "# --publish needs a passing release QA for this version and HEAD"
fresh
run_main --publish
check "--publish with no release QA results is refused" [ "$rc" != 0 ]; has "  says a passing release QA is needed" "needs a passing release QA for 1.2.3"; check "  nothing was built" not_built; check "  no release was created" [ ! -e "$d/released" ]
fresh; qa_make fail
run_main --publish
check "--publish with a failing result is refused" [ "$rc" != 0 ]; has "  names the failing charter" "settings-roundtrip: Escape did nothing"; check "  nothing was built" not_built; check "  no release was created" [ ! -e "$d/released" ]
fresh; qa_make pass; sed -i '' 's/"version":"1.2.3"/"version":"1.2.2"/' "$d/qa/1.2.3/release.json"
run_main --publish
check "--publish with a release.json for another version is refused" [ "$rc" != 0 ]; has "  says which version" "release.json is for version 1.2.2, not 1.2.3"; check "  nothing was built" not_built
fresh; qa_make pass; sed -i '' 's/"commit":"[0-9a-f]*"/"commit":"0000000000000000000000000000000000000000"/' "$d/qa/1.2.3/release.json"
run_main --publish
check "--publish with a release.json for another commit is refused" [ "$rc" != 0 ]; has "  says the commit" "release QA was run for commit 0000000000000000000000000000000000000000"; check "  nothing was built" not_built
fresh; qa_make pass; chmod 666 "$d/qa/1.2.3/release.json"
run_main --publish
check "--publish with a world-writable release.json is refused" [ "$rc" != 0 ]; has "  says why" "writable by its group or by everyone"; check "  nothing was built" not_built
fresh; qa_make pass; chmod 664 "$d/qa/1.2.3/webqa-target.json"
run_main --publish
check "--publish with a group-writable webqa-target.json is refused" [ "$rc" != 0 ]; has "  says why" "writable by its group or by everyone"
fresh; qa_make pass; export STUB_ID_U=99999
run_main --publish
check "--publish with a release.json owned by someone else is refused" [ "$rc" != 0 ]; has "  says why" "is not owned by you"; check "  nothing was built" not_built
unset STUB_ID_U
fresh; qa_make fail; printf '{"schema":1,"digest":"forged","result":"pass","rows":[],"findings":[]}\n' > "$d/qa/1.2.3/webqa.json"
run_main --publish
check "--publish with a hand-edited webqa.json that says pass over failing results is refused" [ "$rc" != 0 ]; has "  names the failing charter" "settings-roundtrip"; check "  no release was created" [ ! -e "$d/released" ]
fresh; qa_make pass; ln -s "$d/qa/1.2.3" "$d/qa/linked"; rm -rf "$d/qa/linked"
run_main --publish
check "--publish with a passing release QA proceeds to the existing publish path" [ "$rc" = 0 ]; check "  the release was created (stubbed gh)" [ -e "$d/released" ]
check "  webqa.json was rewritten by the validator, not trusted" grep -q '"result": "pass"' "$d/qa/1.2.3/webqa.json"

if [ "$fails" = 0 ]; then echo "PASS"; else echo "FAILED"; exit 1; fi
