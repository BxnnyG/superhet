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
# M17: the plugin wraps m17-demod (compiled in the Dockerfile, with a one-line patch that adds the
# TYPE field). The synthetic stream of tests/recordings/m17-voice.* must come out as a call from
# DL1ABC to DB0XYZ with decoded audio (the tool needs about a second to lock, so not all 3 s).
if docker run --rm --entrypoint sh "$image" -c '! ldd /usr/bin/m17-demod | grep -q "not found"'; then
    m17_out="$(docker run --rm -i --entrypoint sh "$image" -c \
        'm17-demod -l 2>/tmp/m17.err | wc -c; tr "\r" "\n" </tmp/m17.err | grep -m1 "^SRC:"' \
        <"$rec/m17-voice.sigmf-data" || true)"
    m17_bytes="$(printf '%s\n' "$m17_out" | sed -n 1p)"
    m17_call="$(printf '%s\n' "$m17_out" | sed -n 2p)"
    if [ "$m17_call" = "SRC: DL1ABC, DEST: DB0XYZ, STR:V/V CAN:03, TYPE: 0x0185, NONCE: 0000000000000000000000000000, CRC: 167a" ] && [ "$m17_bytes" -ge 24000 ]; then
        echo "ok   m17-demod decodes a synthetic M17 call ($m17_bytes bytes of audio)"
    else
        echo "FAIL m17-demod did not decode the synthetic M17 call (got: ${m17_call:-nothing}, $m17_bytes bytes of audio)"
        fail=1
    fi
else
    echo "FAIL m17-demod is missing or lacks a library"
    fail=1
fi
# FT8 / WSPR: jt9 and wsprd (WSJT-X, copied from the Debian package) must have all their libraries
# and decode a synthetic slot through the adapter. The stream clock is the sample count from
# 2026-11-14T12:00:00Z, so the slot is aligned without waiting. Needs python3 on the host.
for tool in jt9 wsprd; do
    if docker run --rm --entrypoint sh "$image" -c "! ldd /usr/bin/$tool | grep -q 'not found'"; then
        echo "ok   $tool links"
    else
        echo "FAIL $tool is missing or lacks a library"
        docker run --rm --entrypoint sh "$image" -c "ldd /usr/bin/$tool | grep 'not found'" || true
        fail=1
    fi
done
for case in "ft8 CQ DL1ABC JO62" "wspr K1ABC FN42 37"; do
    mode="${case%% *}"
    expected="${case#* }"
    got="$(python3 -I scripts/gen-ft8-audio.py "$mode" | docker run --rm -i --entrypoint superhet-plugin-adapter \
        -e "SUPERHET_CONFIG={\"mode\":\"$mode\"}" -e SUPERHET_FT8_STREAM_START_MS=1794657600000 \
        "$image" ft8 | jq -r '.data.message' 2>/dev/null || true)"
    if [ "$got" = "$expected" ]; then
        echo "ok   $mode: jt9 / wsprd decode a synthetic slot ($got)"
    else
        echo "FAIL $mode: expected '$expected', got '${got:-nothing}'"
        fail=1
    fi
done
exit "$fail"
