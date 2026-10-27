#!/usr/bin/env bash
# Unit test of install.sh's safety functions (no Claude call, no build, no real
# install: ditto is a stub, the process list is fake). Not in the npm run check
# gate; run it by hand after changing install.sh.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
trap 'rm -rf "$d"' EXIT
unset DCV2_TEST_MODE DCV2_RUN_DIR || true

fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }

# Loads the functions only (main does not run when sourced).
# shellcheck disable=SC1091
. "$REPO/install.sh"
set +e

# 1. A failing copy leaves the previous app in place.
mkdir -p "$d/Apps/DCTerminalv2.app/Contents" "$d/bundle"
echo old > "$d/Apps/DCTerminalv2.app/Contents/marker"
(
  ditto() { mkdir -p "$2/Contents"; echo partial > "$2/Contents/partial"; return 1; }
  install_copy "$d/bundle" "$d/Apps/DCTerminalv2.app"
) >"$d/fail.out" 2>&1
rc=$?
if [ "$rc" != 0 ]; then ok "a failing copy exits non-zero ($rc)"; else bad "a failing copy exited 0"; fi
if [ "$(cat "$d/Apps/DCTerminalv2.app/Contents/marker" 2>/dev/null)" = old ]; then ok "the previous app is back"; else bad "the previous app is not back: $(cat "$d/fail.out")"; fi
if [ ! -e "$d/Apps/DCTerminalv2.app/Contents/partial" ]; then ok "no partial copy is left"; else bad "a partial copy is left"; fi
if [ ! -e "$d/Apps/DCTerminalv2.app.old" ]; then ok "no .old is left"; else bad ".old is left behind"; fi

# 2. A good copy replaces the app and removes .old.
mkdir -p "$d/bundle/Contents"; echo new > "$d/bundle/Contents/marker"
(
  ditto() { rm -rf "$2"; cp -R "$1" "$2"; }
  install_copy "$d/bundle" "$d/Apps/DCTerminalv2.app"
) >"$d/good.out" 2>&1
if [ "$(cat "$d/Apps/DCTerminalv2.app/Contents/marker")" = new ] && [ ! -e "$d/Apps/DCTerminalv2.app.old" ]; then ok "a good copy installs and cleans up"; else bad "a good copy did not install: $(cat "$d/good.out")"; fi

# 3. The running check sees the app anywhere, and not the hook forwarder.
process_list() {
  printf '%s\n' \
    '/usr/sbin/cfprefsd agent' \
    '/tmp/x/DCTerminalv2.app/Contents/MacOS/dcv2-hook' \
    '/Users/me/target/debug/dcterminalv2 --flag'
}
if [ -z "$(running_app_lines)" ]; then ok "the hook forwarder and a dev binary are not the app"; else bad "matched a non-app: $(running_app_lines)"; fi
process_list() { printf '%s\n' '/tmp/x/DCTerminalv2.app/Contents/MacOS/dcterminalv2' '/bin/zsh'; }
if [ -n "$(running_app_lines)" ]; then ok "the app outside /Applications is seen"; else bad "the app in /tmp/x was not seen"; fi
process_list() { printf '%s\n' '/Applications/DCTerminalv2.app/Contents/MacOS/dcterminalv2 -psn_0_1'; }
if [ -n "$(running_app_lines)" ]; then ok "the app with arguments is seen"; else bad "the app with arguments was not seen"; fi

# 4. A hook forwarder older than the app binary is refused.
echo hook > "$d/hook"; echo app > "$d/app"
touch -t 202001010000 "$d/hook"; touch -t 202101010000 "$d/app"
if hook_is_stale "$d/hook" "$d/app"; then ok "an older hook binary is stale"; else bad "an older hook binary was accepted"; fi
touch -t 202201010000 "$d/hook"
if ! hook_is_stale "$d/hook" "$d/app"; then ok "a newer hook binary is fine"; else bad "a newer hook binary was refused"; fi

bash -n "$REPO/install.sh" && ok "bash -n install.sh" || bad "bash -n install.sh"
if [ "$fails" = 0 ]; then echo PASS; else echo FAILED; exit 1; fi
