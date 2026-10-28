#!/usr/bin/env bash
# Run the golden recordings of the first-party decoder plugins through a built Superhet image
# (the real multimon-ng / direwolf / rtl_433 of the runtime image) and compare with the
# expected events. Usage: scripts/check-plugin-image.sh [image]   (default: superhet:ci)
# Needs docker and jq.
set -euo pipefail
cd "$(dirname "$0")/.."

image="${1:-superhet:ci}"
rec=crates/superhet-plugin-adapters/tests/recordings
fail=0

# plugin directory, adapter mode, recording
cases=(
    "pocsag pocsag pocsag-mixed"
    "pocsag pocsag flex-1600"
    "aprs aprs aprs-afsk1200"
    "rtl433 rtl433 rtl433-nexus"
)

for entry in "${cases[@]}"; do
    read -r plugin mode name <<<"$entry"
    actual="$(docker run --rm -i --entrypoint superhet-plugin-adapter \
        -w "/usr/share/superhet/plugins/$plugin" "$image" "$mode" \
        <"$rec/$name.sigmf-data" | jq -S -c '{type, data}')"
    expected="$(jq -S -c . "$rec/$name.expected.jsonl")"
    if [ "$actual" = "$expected" ]; then
        echo "ok   $name ($(printf '%s\n' "$actual" | wc -l) events)"
    else
        echo "FAIL $name"
        diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") | head -n 10 || true
        fail=1
    fi
done

# DAB: the plugin wraps welle-cli, which must start and have all its libraries (a missing one
# would only show when somebody tunes a block).
if docker run --rm --entrypoint sh "$image" -c 'welle-cli -v >/dev/null 2>&1 && ! ldd /usr/bin/welle-cli | grep -q "not found"'; then
    echo "ok   welle-cli starts and links"
else
    echo "FAIL welle-cli is missing or lacks a library"
    docker run --rm --entrypoint sh "$image" -c 'ldd /usr/bin/welle-cli | grep "not found"' || true
    fail=1
fi

# The shim must confine welle-cli's web interface to loopback (it binds 0.0.0.0 on its own).
listening="$(docker run --rm --entrypoint sh -e LD_PRELOAD=/usr/local/lib/superhet/libsuperhet-loopback-bind.so "$image" -c \
    'welle-cli -c 5C -f /dev/zero -w 18555 -T >/dev/null 2>&1 & sleep 2; awk "NR>1 && \$4==\"0A\" {print \$2}" /proc/net/tcp')"
if [ "$listening" = "0100007F:487B" ]; then
    echo "ok   welle-cli web interface listens on 127.0.0.1 only"
else
    echo "FAIL welle-cli web interface listens on: ${listening:-nothing}"
    fail=1
fi
# ADS-B: the plugin wraps readsb (compiled in the Dockerfile). Feed it the same way the adapter
# does (IQ on stdin, aircraft.json in a directory) with a synthetic frame and look for the
# aircraft. Needs python3 on the host.
adsb="$(python3 -I scripts/gen-adsb-iq.py 2 | docker run --rm -i --entrypoint sh "$image" -c \
    'd=$(mktemp -d); (cat; sleep 3) | readsb --device-type ifile --ifile - --iformat UC8 --devel=ifile_no_synthetic --quiet --write-json "$d" >/dev/null 2>&1; cat "$d/aircraft.json"' |
    jq -r '.aircraft[] | select(.hex == "4840d6") | .flight' 2>/dev/null || true)"
if [ "$adsb" = "KLM1023 " ]; then
    echo "ok   readsb decodes a synthetic ADS-B frame"
else
    echo "FAIL readsb did not decode the synthetic ADS-B frame (got: ${adsb:-nothing})"
    fail=1
fi
exit "$fail"
