#!/usr/bin/env bash
# Repository-local self-check. Runs in the git root.
set -eu
root=$(git rev-parse --show-toplevel)
cd "$root"

if git ls-files | grep -Eq '\.(py|rs|toml|lock)$'; then
  echo "source files are present; update README.md status before committing"
  exit 1
fi
grep -q "Placeholder repository" README.md || {
  echo "README.md lost its placeholder notice"
  exit 1
}
echo "server self-check passed (placeholder repository)"
