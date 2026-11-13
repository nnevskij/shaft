#!/usr/bin/env bash
# Prepare a release: bump the workspace version, commit and tag.
#
#   scripts/release.sh 0.2.0
#
# Nothing is pushed. Push the commit and the tag yourself; the tag triggers the
# CI job that builds, signs, notarizes and publishes the DMG (docs/RELEASING.md).
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo >/dev/null 2>&1; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

NEW=${1:-}
if [[ ! "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
  echo "usage: scripts/release.sh <major.minor.patch[-pre]>" >&2
  exit 1
fi
OLD=$(/usr/bin/sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if [[ "$NEW" == "$OLD" ]]; then
  echo "error: version is already $OLD" >&2
  exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: the working tree has uncommitted changes" >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/v$NEW" >/dev/null; then
  echo "error: tag v$NEW already exists" >&2
  exit 1
fi

echo "==> $OLD -> $NEW"
# Only the [workspace.package] version (the first `version = ` line).
/usr/bin/sed -i '' "1,/^version = \".*\"/s/^version = \".*\"/version = \"$NEW\"/" Cargo.toml
/usr/bin/sed -i '' "s/^  version \".*\"/  version \"$NEW\"/" packaging/homebrew/shaft.rb
cargo update --workspace --quiet
cargo test --workspace --quiet

git add Cargo.toml Cargo.lock packaging/homebrew/shaft.rb
git commit -m "Release $NEW"
git tag -a "v$NEW" -m "Shaft $NEW"

cat <<EOF

Tagged v$NEW. To publish:

  git push origin HEAD "v$NEW"

When the release job has finished, copy the sha256 from
Shaft-$NEW.dmg.sha256 into packaging/homebrew/shaft.rb and your tap.
EOF
