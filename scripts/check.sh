#!/usr/bin/env bash
# Run all checks (Rust + web). Quiet on success, prints output of the failing step only.
set -uo pipefail
cd "$(dirname "$0")/.."

fail=0
step() {
    local name="$1"
    shift
    local log
    log="$(mktemp)"
    if "$@" >"$log" 2>&1; then
        echo "ok   $name"
    else
        echo "FAIL $name"
        tail -n 40 "$log"
        fail=1
    fi
    rm -f "$log"
}

step "cargo fmt" cargo fmt --all --check
step "cargo clippy" cargo clippy --workspace --all-targets -q -- -D warnings
if command -v cargo-nextest >/dev/null; then
    step "cargo test" cargo nextest run --workspace --failure-output final
else
    step "cargo test" cargo test --workspace -q
fi
step "cargo deny" cargo deny check
step "web install" pnpm -C web install --frozen-lockfile --silent
step "web lint" pnpm -C web lint
step "web typecheck" pnpm -C web typecheck
step "web build" pnpm -C web build
step "web test" pnpm -C web test

exit "$fail"
