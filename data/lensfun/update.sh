#!/usr/bin/env bash
# Refreshes the built-in lensfun files from upstream (see README.md).
# Usage, from the repository root: bash data/lensfun/update.sh [commit]
set -euo pipefail
dest="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
git clone --quiet https://github.com/lensfun/lensfun "$tmp/lensfun"
if [ $# -ge 1 ]; then git -C "$tmp/lensfun" checkout --quiet "$1"; fi
for f in "$dest"/*.xml; do
  name="$(basename "$f")"
  if [ -f "$tmp/lensfun/data/db/$name" ]; then
    cp "$tmp/lensfun/data/db/$name" "$f"
  else
    echo "warning: $name no longer exists upstream" >&2
  fi
done
cp "$tmp/lensfun/data/COPYING.CC_BY-SA_3.0" "$dest/"
echo "lensfun commit: $(git -C "$tmp/lensfun" log -1 --format='%H (%cs)')"
echo "Record it in data/lensfun/README.md, then run: cargo test -p lens-db"
