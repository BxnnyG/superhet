#!/usr/bin/env bash
# Run the Playwright smoke tests (multi-user flow, plugin start to event) against a throwaway stack: PostgreSQL and
# NATS in temporary containers on random loopback ports, the api/sdrd/worker debug binaries and
# the built web UI. Never touches an instance on :8073.
#
#   scripts/e2e.sh            build everything, run web/e2e
#   E2E_SKIP_BUILD=1 ...      reuse target/debug and web/dist
#   scripts/e2e.sh --headed   extra arguments go to `playwright test`
#
# CI starts its own services and calls `pnpm -C web e2e` directly (see .github/workflows/ci.yml).
set -euo pipefail
cd "$(dirname "$0")/.."

tag="sh-e2e-$$"
cleanup() {
    docker rm -f "$tag-pg" "$tag-nats" >/dev/null 2>&1 || true
}
trap cleanup EXIT

if [ -z "${E2E_SKIP_BUILD:-}" ]; then
    cargo build -q -p superhet-api -p superhet-sdrd -p superhet-worker -p superhet-pluginhost
    pnpm -C web install --frozen-lockfile --silent
    pnpm -C web build
fi
pnpm -C web exec playwright install chromium >/dev/null

docker run -d --name "$tag-pg" -e POSTGRES_PASSWORD=e2e -e POSTGRES_DB=superhet \
    -p 127.0.0.1::5432 postgres:18.6-alpine >/dev/null
docker run -d --name "$tag-nats" -p 127.0.0.1::4222 nats:2.15.1-alpine -js >/dev/null
pg_port="$(docker port "$tag-pg" 5432/tcp | head -n1 | sed 's/.*://')"
nats_port="$(docker port "$tag-nats" 4222/tcp | head -n1 | sed 's/.*://')"
for _ in $(seq 1 60); do
    docker exec "$tag-pg" pg_isready -U postgres -d superhet >/dev/null 2>&1 && break
    sleep 1
done
sleep 2

export SUPERHET_DATABASE_URL="postgres://postgres:e2e@127.0.0.1:$pg_port/superhet"
export SUPERHET_NATS_URL="nats://127.0.0.1:$nats_port"
pnpm -C web e2e "$@"
