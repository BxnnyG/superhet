#!/usr/bin/env bash
# Build the ready-to-run compose bundle attached to GitHub Releases.
# Usage: scripts/package-compose.sh <tag, e.g. v0.1.0> [output-dir, default: dist]
# Produces <out>/superhet-<tag>-compose.tar.gz and a .sha256 next to it.
# The bundled compose.yaml pulls ghcr.io/bxnnyg/superhet:<version> instead of building.
set -euo pipefail
cd "$(dirname "$0")/.."

tag="${1:?usage: $0 <tag> [output-dir]}"
out="${2:-dist}"
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "invalid tag: $tag" >&2; exit 1; }
version="${tag#v}"
image="${IMAGE:-ghcr.io/bxnnyg/superhet}:${version}"
name="superhet-${tag}-compose"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
root="$stage/$name"
mkdir -p "$root/scripts" "$root/docs" "$root/deploy"

# compose.yaml: drop the build block, switch the default image to the published one.
awk '
    /^  build:$/ { skip = 1; next }
    skip && /^    / { next }
    { skip = 0; print }
' compose.yaml | sed "s|\${SUPERHET_IMAGE:-superhet:dev}|\${SUPERHET_IMAGE:-${image}}|" >"$root/compose.yaml"
if grep -q -e 'build:' -e 'superhet:dev' "$root/compose.yaml" || ! grep -qF "$image" "$root/compose.yaml"; then
    echo "compose.yaml transformation failed (did the x-superhet block change?)" >&2
    exit 1
fi

cp -r deploy/nats deploy/udev deploy/modprobe "$root/deploy/"
install -m 0755 scripts/gen-secrets.sh "$root/scripts/gen-secrets.sh"
cp .env.example "$root/.env.example"
cp docs/setup-host.md "$root/docs/setup-host.md"
cp LICENSE NOTICE "$root/"
sed -e "s|@TAG@|${tag}|g" -e "s|@IMAGE@|${image}|g" deploy/bundle/INSTALL.md >"$root/INSTALL.md"

mkdir -p "$out"
archive="$out/$name.tar.gz"
# Reproducible-ish: fixed order, no owner info, mtime from the commit when available.
mtime="$(git log -1 --format=%cI 2>/dev/null || date -u +%FT%TZ)"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="$mtime" -C "$stage" -czf "$archive" "$name"
(cd "$out" && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
echo "$archive"
