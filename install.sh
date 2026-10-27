#!/usr/bin/env bash
# Build DCTerminalv2 and install it into /Applications (or ~/Applications).
#
#   ./install.sh            build, then install
#   ./install.sh --no-build install the last build (src-tauri/target/release/bundle/macos)
#
# macOS only. Local install: no notarization. Bash 3.2 safe. The old DCTerminal is not touched.
set -euo pipefail

APP_NAME="DCTerminalv2.app"
ROOT="$(cd "$(dirname "$0")" && pwd)"
BUNDLE="$ROOT/src-tauri/target/release/bundle/macos/$APP_NAME"
HOOK_BIN="$ROOT/src-tauri/target/release/dcv2-hook"

say() { printf '==> %s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

# Every process's command line (BSD ps). A function so a test can replace it.
process_list() { ps -axo command= 2>/dev/null; }

# A DCTerminalv2 app is running from any folder (/Applications, ~/Applications,
# a build folder, a copy elsewhere). Its main binary is dcterminalv2; the hook
# forwarder dcv2-hook and a cargo dev binary (not inside a .app) do not count.
running_app_lines() {
  process_list | grep -E '\.app/Contents/MacOS/dcterminalv2( |$)' || true
}

# The hook forwarder is older than the app's main binary: it was built before
# the last app build and may not speak the app's protocol.
hook_is_stale() { # <hook binary> <main binary>
  [ "$1" -ot "$2" ]
}

ROLLBACK_DEST=""
ROLLBACK_OLD=""
ROLLBACK_PENDING=0

# Runs on exit. While a copy is in flight (between moving the old app aside
# and the end of ditto) it removes the partial copy and puts the old app back.
rollback_install() {
  [ "$ROLLBACK_PENDING" = 1 ] || return 0
  ROLLBACK_PENDING=0
  printf 'install.sh: the copy did not finish; removing the partial copy.\n' >&2
  rm -rf "$ROLLBACK_DEST"
  if [ -d "$ROLLBACK_OLD" ]; then
    mv "$ROLLBACK_OLD" "$ROLLBACK_DEST"
    printf 'install.sh: the previous app is back at %s.\n' "$ROLLBACK_DEST" >&2
  fi
}

# Copy <bundle> to <dest>, keeping the old app until the copy is complete.
install_copy() { # <bundle> <dest>
  local bundle="$1" dest="$2"
  ROLLBACK_DEST="$dest"
  ROLLBACK_OLD="$dest.old"
  trap rollback_install EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  ROLLBACK_PENDING=1
  if [ -d "$dest" ]; then
    rm -rf "$ROLLBACK_OLD"
    mv "$dest" "$ROLLBACK_OLD"
  fi
  ditto "$bundle" "$dest" || die "could not copy the app to $dest."
  ROLLBACK_PENDING=0
  trap - EXIT INT TERM
  rm -rf "$ROLLBACK_OLD"
}

main() {
  [ "$(uname -s)" = "Darwin" ] || die "macOS only."

  local build=1 arg
  for arg in "$@"; do
    case "$arg" in
      --no-build) build=0 ;;
      -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
      *) die "unknown option: $arg" ;;
    esac
  done

  if [ "$build" = 1 ]; then
    say "Installing npm packages"
    (cd "$ROOT" && npm ci --silent)
    say "Building the app (a few minutes the first time)"
    (cd "$ROOT" && npx tauri build --bundles app)
    say "Building the hook forwarder"
    cargo build --release --manifest-path "$ROOT/src-tauri/Cargo.toml" --bin dcv2-hook
  fi

  [ -d "$BUNDLE" ] || die "no build at $BUNDLE (run without --no-build)."
  [ -f "$BUNDLE/Contents/Resources/plugin/.claude-plugin/plugin.json" ] \
    || die "the build has no plugin folder; check tauri.conf.json bundle.resources."

  # The app looks for dcv2-hook beside its main binary. Tauri normally bundles it;
  # add it (and re-sign ad hoc) only when it is missing.
  if [ ! -x "$BUNDLE/Contents/MacOS/dcv2-hook" ]; then
    [ -x "$HOOK_BIN" ] || die "no hook forwarder at $HOOK_BIN (run without --no-build)."
    if hook_is_stale "$HOOK_BIN" "$BUNDLE/Contents/MacOS/dcterminalv2"; then
      die "the hook forwarder at $HOOK_BIN is older than the app build. Rebuild it: cargo build --release --manifest-path src-tauri/Cargo.toml --bin dcv2-hook (or run without --no-build)."
    fi
    say "Adding the hook forwarder to the app"
    cp "$HOOK_BIN" "$BUNDLE/Contents/MacOS/dcv2-hook"
    chmod 755 "$BUNDLE/Contents/MacOS/dcv2-hook"
    codesign --force --deep --sign - "$BUNDLE" >/dev/null 2>&1 || die "codesign failed."
  fi

  local dest_dir dest running
  if [ -w /Applications ]; then dest_dir=/Applications; else dest_dir="$HOME/Applications"; mkdir -p "$dest_dir"; fi
  dest="$dest_dir/$APP_NAME"

  running="$(running_app_lines)"
  if [ -n "$running" ]; then
    printf '%s\n' "$running" >&2
    die "DCTerminalv2 is running (see above). Quit it (Cmd+Q) and run this again."
  fi

  say "Installing to $dest"
  install_copy "$BUNDLE" "$dest"
  xattr -dr com.apple.quarantine "$dest" 2>/dev/null || true

  say "Done: $(cat "$dest/Contents/Resources/plugin/VERSION" 2>/dev/null | sed 's/^/plugin /')"
  say "Open it with: open \"$dest\""
}

# Sourced (by a test), the functions load and nothing runs.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
