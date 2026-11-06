#!/usr/bin/env bash
# Build, seal, check and package DCTerminalv2 as a DMG on this Mac, then print its SHA-256.
#
#   scripts/release-mac.sh              build DCTerminalv2_<version>_aarch64.dmg (Apple silicon host)
#   scripts/release-mac.sh --publish    also create the GitHub release v<version> from the built commit
#
# macOS only, bash 3.2 and BSD tools. Nothing leaves this machine without --publish.
#
# Refuses, before building anything: a dirty tree (untracked files included), versions that
# disagree (package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml; plugin/VERSION,
# plugin/.claude-plugin/plugin.json), and a host that is not arm64 (the default DMG is labelled
# aarch64). With --publish also: an existing tag v<version> (local or remote), no gh, and a HEAD
# that is not the tip of the remote default branch.
#
# Order: build the app (and dcv2-hook), add dcv2-hook to Contents/MacOS when missing, sign ad hoc
# (`codesign --force --deep --sign -`), verify the seal (`--verify --deep --strict`), make the DMG
# (hdiutil, UDZO), mount it read-only, verify the app inside it, detach (always, even on failure),
# print and save the SHA-256. With --publish, read the remote tip again, refuse unless it still equals
# HEAD, and create the release with --target <HEAD sha> so the tag points at the built commit.
set -euo pipefail

ROOT="${RELEASE_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
APP_NAME="DCTerminalv2.app"
VOLUME_NAME="DCTerminalv2"

say() { printf '==> %s\n' "$*"; }
die() { printf 'release-mac.sh: %s\n' "$*" >&2; exit 1; }

# --- reading the repo --------------------------------------------------------------------------

json_version() { # <file>: the first "version" value
  sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$1" | head -n 1
}

cargo_version() { # <Cargo.toml>: version of the [package] table
  awk '/^\[package\]/ { p = 1; next } /^\[/ { p = 0 } p && /^version[[:space:]]*=/ { gsub(/^version[[:space:]]*=[[:space:]]*"|".*$/, ""); print; exit }' "$1"
}

app_version() { json_version "$ROOT/package.json"; }

require_clean_tree() {
  local dirty
  dirty="$(git -C "$ROOT" status --porcelain)" || die "git status failed."
  if [ -n "$dirty" ]; then
    printf '%s\n' "$dirty" >&2
    die "the working tree is not clean (see above). Commit or remove those changes first."
  fi
}

require_versions_match() {
  local npm tauri cargo plugin plugin_json
  npm="$(json_version "$ROOT/package.json")"
  tauri="$(json_version "$ROOT/src-tauri/tauri.conf.json")"
  cargo="$(cargo_version "$ROOT/src-tauri/Cargo.toml")"
  [ -n "$npm" ] || die "no version in package.json."
  if [ "$npm" != "$tauri" ] || [ "$npm" != "$cargo" ]; then
    die "the app versions differ: package.json $npm, src-tauri/tauri.conf.json ${tauri:-none}, src-tauri/Cargo.toml ${cargo:-none}. Bump them together."
  fi
  plugin="$(tr -d '[:space:]' < "$ROOT/plugin/VERSION" 2>/dev/null || true)"
  plugin_json="$(json_version "$ROOT/plugin/.claude-plugin/plugin.json" 2>/dev/null || true)"
  if [ -z "$plugin" ] || [ "$plugin" != "$plugin_json" ]; then
    die "the plugin versions differ: plugin/VERSION ${plugin:-none}, plugin/.claude-plugin/plugin.json ${plugin_json:-none}. Bump them together."
  fi
}

require_host_arch() { # <universal: 0|1>
  local arch
  arch="$(uname -m)"
  if [ "$1" != 1 ] && [ "$arch" != "arm64" ]; then
    die "this host is $arch, and the default DMG is labelled aarch64. Build on Apple silicon, or pass --universal."
  fi
}

# --- the remote (only with --publish) ----------------------------------------------------------

remote_default_tip() { # prints "<branch> <sha>"
  local branch tip
  branch="$(cd "$ROOT" && gh repo view --json defaultBranchRef --jq .defaultBranchRef.name 2>/dev/null)" || die "gh could not read the default branch (is gh logged in?)."
  [ -n "$branch" ] || die "gh did not name the default branch."
  tip="$(git -C "$ROOT" ls-remote origin "refs/heads/$branch" | awk '{ print $1 }' | head -n 1)" || die "could not read origin/$branch."
  [ -n "$tip" ] || die "origin has no branch $branch."
  printf '%s %s\n' "$branch" "$tip"
}

require_head_is_remote_tip() {
  local pair branch tip head
  pair="$(remote_default_tip)"
  branch="${pair%% *}"
  tip="${pair##* }"
  head="$(git -C "$ROOT" rev-parse HEAD)"
  if [ "$head" != "$tip" ]; then
    die "HEAD ($head) is not the tip of origin/$branch ($tip). Publish only from the commit that is on the remote: pull or push first."
  fi
}

require_publish_ready() { # <tag>
  local tag="$1"
  command -v gh >/dev/null 2>&1 || die "--publish needs gh (the GitHub CLI), and it is not installed."
  if git -C "$ROOT" rev-parse -q --verify "refs/tags/$tag" >/dev/null 2>&1; then
    die "the tag $tag already exists locally."
  fi
  if [ -n "$(git -C "$ROOT" ls-remote --tags origin "refs/tags/$tag" 2>/dev/null)" ]; then
    die "the tag $tag already exists on origin."
  fi
  require_head_is_remote_tip
}

# --- the bundle --------------------------------------------------------------------------------

plist_value() { # <Info.plist> <key>
  /usr/libexec/PlistBuddy -c "Print :$2" "$1" 2>/dev/null
}

# The main binary's path inside an .app, from its CFBundleExecutable (never hard-coded in one case).
bundle_binary() { # <app>
  local exe
  exe="$(plist_value "$1/Contents/Info.plist" CFBundleExecutable)" || exe=""
  [ -n "$exe" ] || die "$1 has no CFBundleExecutable in Contents/Info.plist."
  printf '%s/Contents/MacOS/%s\n' "$1" "$exe"
}

hook_is_stale() { [ "$1" -ot "$2" ]; } # <hook binary> <main binary>, as in install.sh

# The app looks for dcv2-hook beside its main binary. Tauri normally bundles it; add it (as install.sh does)
# only when it is missing.
add_hook() { # <app> <hook binary>
  local app="$1" hook="$2" main
  [ -x "$app/Contents/MacOS/dcv2-hook" ] && return 0
  [ -x "$hook" ] || die "no hook forwarder at $hook."
  main="$(bundle_binary "$app")"
  if hook_is_stale "$hook" "$main"; then
    die "the hook forwarder at $hook is older than the app build."
  fi
  say "Adding the hook forwarder to the app"
  cp "$hook" "$app/Contents/MacOS/dcv2-hook"
  chmod 755 "$app/Contents/MacOS/dcv2-hook"
}

sign_app() { # <app>
  say "Signing (ad hoc)"
  codesign --force --deep --sign - "$1" >/dev/null 2>&1 || die "codesign failed."
}

verify_app() { # <app> <what>
  codesign --verify --deep --strict "$1" >/dev/null 2>&1 || die "$2: the code signature does not verify (codesign --verify --deep --strict $1)."
  local main
  main="$(bundle_binary "$1")"
  [ -x "$main" ] || die "$2: the app binary $main is missing or not executable."
  [ -x "$1/Contents/MacOS/dcv2-hook" ] || die "$2: dcv2-hook is not in Contents/MacOS."
}

# --- the DMG -----------------------------------------------------------------------------------

STAGE_DIR=""
MOUNT_DIR=""
MOUNTED=0

detach_dmg() {
  if [ "$MOUNTED" = 1 ]; then
    MOUNTED=0
    hdiutil detach "$MOUNT_DIR" -quiet >/dev/null 2>&1 || hdiutil detach -force "$MOUNT_DIR" -quiet >/dev/null 2>&1 || printf 'release-mac.sh: could not detach %s; run: hdiutil detach -force "%s"\n' "$MOUNT_DIR" "$MOUNT_DIR" >&2
  fi
}

cleanup() {
  detach_dmg
  [ -z "$STAGE_DIR" ] || rm -rf "$STAGE_DIR"
  [ -z "$MOUNT_DIR" ] || rmdir "$MOUNT_DIR" 2>/dev/null || true
}

make_dmg() { # <app> <dmg>
  local app="$1" dmg="$2"
  STAGE_DIR="$(mktemp -d)"
  ditto "$app" "$STAGE_DIR/$APP_NAME" || die "could not copy the app into the DMG folder."
  ln -s /Applications "$STAGE_DIR/Applications"
  rm -f "$dmg"
  say "Making $(basename "$dmg")"
  hdiutil create -volname "$VOLUME_NAME" -srcfolder "$STAGE_DIR" -ov -format UDZO "$dmg" >/dev/null || die "hdiutil create failed."
}

verify_dmg() { # <dmg>
  MOUNT_DIR="$(mktemp -d)"
  say "Mounting the DMG read-only to check the app inside"
  hdiutil attach -readonly -nobrowse -noverify -mountpoint "$MOUNT_DIR" "$1" >/dev/null || die "hdiutil attach failed."
  MOUNTED=1
  verify_app "$MOUNT_DIR/$APP_NAME" "the app inside the DMG"
  detach_dmg
}

write_sha() { # <dmg>
  local sum
  sum="$(shasum -a 256 "$1" | awk '{ print $1 }')"
  printf '%s  %s\n' "$sum" "$(basename "$1")" > "$1.sha256"
  say "SHA-256: $sum  $(basename "$1")"
}

publish_release() { # <tag> <dmg>
  local tag="$1" dmg="$2" head
  require_head_is_remote_tip
  head="$(git -C "$ROOT" rev-parse HEAD)"
  say "Creating the GitHub release $tag at $head"
  gh release create "$tag" --target "$head" --title "DCTerminalv2 $tag" --generate-notes "$dmg" "$dmg.sha256" || die "gh release create failed."
}

main() {
  [ "$(uname -s)" = "Darwin" ] || die "macOS only."
  local publish=0 universal=0 arg
  for arg in "$@"; do
    case "$arg" in
      --publish) publish=1 ;;
      -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
      *) die "unknown option: $arg" ;;
    esac
  done

  # Refusals first: nothing is built until every one passes.
  require_clean_tree
  require_versions_match
  require_host_arch "$universal"
  local version tag
  version="$(app_version)"
  tag="v$version"
  [ "$publish" = 0 ] || require_publish_ready "$tag"

  trap cleanup EXIT
  local bundle_root="$ROOT/src-tauri/target/release/bundle"
  local app="$bundle_root/macos/$APP_NAME"
  local hook="$ROOT/src-tauri/target/release/dcv2-hook"
  local dmg="$bundle_root/dmg/DCTerminalv2_${version}_aarch64.dmg"

  say "Building the app"
  (cd "$ROOT" && npx tauri build --bundles app) || die "the app build failed."
  say "Building the hook forwarder"
  cargo build --release --manifest-path "$ROOT/src-tauri/Cargo.toml" --bin dcv2-hook || die "the hook forwarder build failed."
  [ -d "$app" ] || die "no build at $app."

  add_hook "$app" "$hook"
  sign_app "$app"
  verify_app "$app" "the built app"
  mkdir -p "$(dirname "$dmg")"
  make_dmg "$app" "$dmg"
  verify_dmg "$dmg"
  write_sha "$dmg"
  if [ "$publish" = 1 ]; then
    publish_release "$tag" "$dmg"
  else
    say "Done: $dmg (nothing was published; pass --publish to create the GitHub release)"
  fi
}

# Sourced (by a test), the functions load and nothing runs.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
