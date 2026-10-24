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

[ "$(uname -s)" = "Darwin" ] || die "macOS only."

build=1
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
  say "Adding the hook forwarder to the app"
  cp "$HOOK_BIN" "$BUNDLE/Contents/MacOS/dcv2-hook"
  chmod 755 "$BUNDLE/Contents/MacOS/dcv2-hook"
  codesign --force --deep --sign - "$BUNDLE" >/dev/null 2>&1 || die "codesign failed."
fi

if [ -w /Applications ]; then DEST_DIR=/Applications; else DEST_DIR="$HOME/Applications"; mkdir -p "$DEST_DIR"; fi
DEST="$DEST_DIR/$APP_NAME"

if pgrep -f "$DEST/Contents/MacOS/" >/dev/null 2>&1; then
  die "DCTerminalv2 is running from $DEST. Quit it (Cmd+Q) and run this again."
fi

say "Installing to $DEST"
if [ -d "$DEST" ]; then
  rm -rf "$DEST.old"
  mv "$DEST" "$DEST.old"
fi
ditto "$BUNDLE" "$DEST"
rm -rf "$DEST.old"
xattr -dr com.apple.quarantine "$DEST" 2>/dev/null || true

say "Done: $(cat "$DEST/Contents/Resources/plugin/VERSION" 2>/dev/null | sed 's/^/plugin /')"
say "Open it with: open \"$DEST\""
