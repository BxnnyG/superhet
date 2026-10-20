#!/usr/bin/env bash
# One-time developer setup: enable the repo's git hooks and install web dependencies.
set -euo pipefail
cd "$(dirname "$0")/.."

git config core.hooksPath .githooks
echo "git hooks enabled (.githooks)"

if command -v pnpm >/dev/null; then
    pnpm -C web install --frozen-lockfile --silent
    echo "web dependencies installed"
else
    echo "pnpm not found; skipping web dependencies" >&2
fi
