#!/usr/bin/env bash
# Throwaway stack for the browser smoke tests (web/e2e): superhet-api + superhet-sdrd (file
# backend with a noise capture) + superhet-worker + superhet-pluginhost on loopback test ports,
# serving web/dist. The plugin host seeds the first-party plugins (plugins/pocsag, aprs, rtl433, dab;
# their tools are not needed until an instance starts) and gets a private plugin directory;
# the plugin smoke test installs plugins/example-echo (python3) from its path.
# Runs in the foreground until it gets a signal (Playwright's webServer does that), then stops
# everything it started.
#
# Needs: a FRESH PostgreSQL database and a NATS server (JetStream not required), given as
# SUPERHET_DATABASE_URL / SUPERHET_NATS_URL (optionally SUPERHET_NATS_USER/PASSWORD; JetStream
# enabled for the plugin events), the four debug binaries (cargo build -p superhet-api
# -p superhet-sdrd -p superhet-worker -p superhet-pluginhost -p superhet-plugin-adapters) and web/dist (pnpm -C web build).
# scripts/e2e.sh sets all of that up locally.
#
# Ports (loopback only): api E2E_PORT (default 18073), sdrd +1, pluginhost +2, worker +3.
# Never 8073.
set -euo pipefail
cd "$(dirname "$0")/.."

: "${SUPERHET_DATABASE_URL:?set SUPERHET_DATABASE_URL to a fresh throwaway database}"
: "${SUPERHET_NATS_URL:?set SUPERHET_NATS_URL}"
port="${E2E_PORT:-18073}"
if [ "$port" = 8073 ]; then
    echo "refusing to use port 8073 (a real instance)" >&2
    exit 1
fi
bins="$(cd "${E2E_BIN_DIR:-target/debug}" && pwd)"
web="${E2E_WEB_DIR:-web/dist}"
for f in superhet-api superhet-sdrd superhet-worker superhet-pluginhost superhet-plugin-adapter; do
    [ -x "$bins/$f" ] || { echo "missing $bins/$f (cargo build -p $f)" >&2; exit 1; }
done
[ -f "$web/index.html" ] || { echo "missing $web/index.html (pnpm -C web build)" >&2; exit 1; }

work="$(mktemp -d)"
pids=()
cleanup() {
    trap - EXIT INT TERM
    for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT INT TERM

# The bundled plugins as the image has them: copies of the first-party plugin directories.
mkdir -p "$work/bundled" "$work/plugins"
for name in pocsag aprs rtl433 dab; do cp -r "plugins/$name" "$work/bundled/$name"; done

# The DAB plugin runs the real adapter against a stand-in welle-cli (python3): blocks 5C and 6A
# carry the "DR Deutschland" ensemble, a tone plays as audio, the slide is a tiny PNG.
mkdir -p "$work/bin"
# E2E_WELLE=/path/to/welle-cli (or a wrapper) swaps in a real decoder for manual checks.
ln -s "${E2E_WELLE:-$(pwd)/crates/superhet-plugin-adapters/tests/dab/fake-welle-cli}" "$work/bin/welle-cli"

# 1 s of random cu8 samples at 2.048 MS/s: valid IQ, plays in a loop.
if [ -n "${E2E_IQ_FILE:-}" ]; then
    ln -s "$E2E_IQ_FILE" "$work/noise.cu8"
else
    head -c 4096000 /dev/urandom >"$work/noise.cu8"
fi

export SUPERHET_NATS_URL
export RUST_LOG="${RUST_LOG:-error}"

SUPERHET_LISTEN="127.0.0.1:$((port + 1))" \
    SUPERHET_DEVICE_ID=dev0 \
    SUPERHET_DEVICE="file:$work/noise.cu8?format=cu8&rate=2048000&freq=${E2E_IQ_FREQ:-100000000}&block=4096" \
    "$bins/superhet-sdrd" &
pids+=($!)

SUPERHET_LISTEN="127.0.0.1:$port" \
    SUPERHET_SETUP_TOKEN="${E2E_SETUP_TOKEN:-e2e-setup-token-0123456789}" \
    SUPERHET_COOKIE_SECURE=false \
    SUPERHET_WEB_DIR="$web" \
    "$bins/superhet-api" &
pids+=($!)

# The api applies the migrations; the worker starts once it answers.
for _ in $(seq 1 120); do
    curl -fsS "http://127.0.0.1:$port/healthz" >/dev/null 2>&1 && break
    sleep 0.5
done

SUPERHET_LISTEN="127.0.0.1:$((port + 3))" "$bins/superhet-worker" &
pids+=($!)

SUPERHET_LISTEN="127.0.0.1:$((port + 2))" \
    SUPERHET_PLUGIN_DIR="$work/plugins" \
    SUPERHET_BUNDLED_PLUGIN_DIR="$work/bundled" \
    PATH="$work/bin:$bins:$PATH" \
    "$bins/superhet-pluginhost" &
pids+=($!)

# Stop as soon as any process dies; the caller sees a non-zero exit.
wait -n
echo "e2e stack: a process exited" >&2
exit 1
