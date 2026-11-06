#!/usr/bin/env bash
# Regenerate the synthetic M17 reference recording used by the golden test of the M17 plugin
# (crates/superhet-plugin-adapters/tests/m17_plugin.rs): a 3 s voice stream from DL1ABC to DB0XYZ
# (CAN 3) as 4FSK baseband, 48 kHz s16le (what an NFM receiver delivers), written as SigMF to
# crates/superhet-plugin-adapters/tests/recordings/m17-voice.sigmf-{data,meta}.
#
# The modulator is `m17-mod` from the same project as the demodulator (m17-cxx-demod, GPL-3.0).
# The Dockerfile builds it in stage `m17`; either point M17_MOD at a binary or let the script
# build and use that stage (needs docker):
#   M17_MOD=/path/to/m17-mod scripts/gen-m17-recording.sh
#   scripts/gen-m17-recording.sh
# The voice is a deterministic synthetic signal (no recorded speech); the result is CC0.
set -euo pipefail
cd "$(dirname "$0")/.."
out=crates/superhet-plugin-adapters/tests/recordings

voice="$(mktemp)"
trap 'rm -f "$voice"' EXIT
python3 -I - >"$voice" <<'EOF'
import math, struct, sys
# 3 s at 8 kHz: two "vowel" bursts (formant-like sine mixes) with syllable-like envelopes.
out = bytearray()
for i in range(24000):
    t = i / 8000
    env = 0.5 * (1 - math.cos(2 * math.pi * 2.5 * t)) ** 1.0
    f0 = 120 + 20 * math.sin(2 * math.pi * 0.7 * t)
    x = sum(a * math.sin(2 * math.pi * f * t + p)
            for f, a, p in ((f0, 0.5, 0), (2 * f0, 0.3, 1), (700, 0.25, 2), (1200, 0.2, 3), (2600, 0.08, 4)))
    out += struct.pack("<h", int(9000 * env * x))
sys.stdout.buffer.write(out)
EOF

if [ -n "${M17_MOD:-}" ]; then
    mod=("$M17_MOD")
else
    docker build -q --target m17 -t superhet-m17-stage -f deploy/docker/Dockerfile . >/dev/null
    mod=(docker run --rm -i superhet-m17-stage /src/m17/build/apps/m17-mod)
fi

"${mod[@]}" -S DL1ABC -D DB0XYZ -C 3 <"$voice" 2>/dev/null >"$out/m17-voice.sigmf-data"
cat >"$out/m17-voice.sigmf-meta" <<'EOF'
{
  "global": {
    "core:datatype": "ri16_le",
    "core:sample_rate": 48000,
    "core:version": "1.2.0",
    "core:description": "Synthetic M17 voice stream DL1ABC to DB0XYZ, CAN 3, 3 s (4FSK baseband as an NFM receiver delivers it)",
    "core:author": "BxnnyG",
    "core:license": "CC0-1.0",
    "core:recorder": "scripts/gen-m17-recording.sh (m17-mod from m17-cxx-demod)"
  },
  "captures": [
    {
      "core:sample_start": 0
    }
  ],
  "annotations": []
}
EOF
ls -l "$out"/m17-voice.sigmf-data | awk '{print $5 " bytes"}'
