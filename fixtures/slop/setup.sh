#!/bin/sh
# Build the slop fixture repo in the folder given as $1 (prints the base commit sha): the base tree is one commit, the change tree a second one,
# and uncommitted/ a working-tree edit on top. The expected findings are in expected.txt (sorted `path:line:rule`).
set -e
here="$(cd "$(dirname "$0")" && pwd)"
dest="$1"
mkdir -p "$dest"
git -C "$dest" init -q -b main
git -C "$dest" config user.name t
git -C "$dest" config user.email t@t
cp -R "$here/base/." "$dest/"
git -C "$dest" add -A
git -C "$dest" commit -q -m base
base="$(git -C "$dest" rev-parse HEAD)"
cp -R "$here/change/." "$dest/"
git -C "$dest" add -A
git -C "$dest" commit -q -m change
cp -R "$here/uncommitted/." "$dest/"
echo "$base"
