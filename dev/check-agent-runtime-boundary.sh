#!/bin/sh
set -eu

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

command -v rg >/dev/null 2>&1 || {
  echo "ripgrep is required for the Agent runtime boundary check." >&2
  exit 1
}

for retired_entry in archive/zig-cli build.zig build.zig.zon src install.sh dev/install-cli-macos.sh; do
  if [ -e "$retired_entry" ]; then
    echo "Retired Zig entry returned to the active repository: $retired_entry" >&2
    exit 1
  fi
done

if rg -n \
  'zig build|zig-out/bin/clumsies|Application Support/ai\.clumsies/bin/clumsies|~/.clumsies/bin/clumsies|clumsies-darwin|install:cli:macos' \
  .github/workflows \
  apps/macos/Scripts \
  apps/macos/project.yml \
  packages/clumsies \
  package.json
then
  echo "An active build, package, or Adapter surface still references the retired Zig CLI." >&2
  exit 1
fi
