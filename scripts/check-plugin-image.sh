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
exit "$fail"
