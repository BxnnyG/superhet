#!/bin/sh
# Superhet ops backup of a docker compose install (docs/backup.md, ADR-0030).
#
#   scripts/backup.sh [--dir DIR] [--no-env] [--stop] [--project NAME] [--with-nats] [--with-recordings]
#
# Run it in the install directory (the one with compose.yaml and .env), or set SUPERHET_DIR.
# Writes one tar file, superhet-ops-backup-<UTC timestamp>.tar (mode 0600, default into ./backups),
# holding:
#   db.dump        pg_dump custom format of the database (a consistent snapshot, taken online)
#   vol-<name>.tar the compose volumes (plugins, ...) except postgres-data (inside db.dump),
#                  tile-cache (cache), backups (scheduled archives), nats-data (JetStream state,
#                  rebuilt by the services; --with-nats adds it) and recordings (can be huge;
#                  --with-recordings adds it)
#   env.copy, compose.yaml.copy   the install's .env and compose.yaml (--no-env leaves them out;
#                  .env holds the secrets, so protect or encrypt the archive)
#   MANIFEST, SHA256SUMS
# --stop stops the services (not PostgreSQL) while the volumes are copied, for a fully quiescent
# copy of the plugin and recordings volumes. Restore with scripts/restore.sh.
# Needs docker compose v2 and sh; the volume copy reuses the postgres image (no extra pull).
set -eu

say() { printf '%s\n' "$*"; }
die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

out_dir=''
keep_env=1
stop=0
with_nats=0
with_recordings=0
project="${COMPOSE_PROJECT_NAME:-}"
while [ $# -gt 0 ]; do
    case "$1" in
    --dir)
        [ $# -ge 2 ] || die "--dir needs a directory"
        out_dir="$2"
        shift
        ;;
    --no-env) keep_env=0 ;;
    --stop) stop=1 ;;
    --with-nats) with_nats=1 ;;
    --with-recordings) with_recordings=1 ;;
    --project)
        [ $# -ge 2 ] || die "--project needs a name"
        project="$2"
        shift
        ;;
    -h | --help)
        sed -n '2,/^# Needs docker/p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *) die "unknown argument: $1 (see --help)" ;;
    esac
    shift
done

install_dir="${SUPERHET_DIR:-.}"
cd "$install_dir"
[ -f compose.yaml ] || die "no compose.yaml in $(pwd); run this in the install directory or set SUPERHET_DIR"
command -v docker >/dev/null 2>&1 || die "docker not found"
docker compose version >/dev/null 2>&1 || die "docker compose v2 plugin not found"
[ -z "$project" ] || export COMPOSE_PROJECT_NAME="$project"
project="$(docker compose config --format json 2>/dev/null | sed -n 's/^  "name": "\([^"]*\)".*/\1/p' | head -n 1)"
[ -n "$project" ] || project="${COMPOSE_PROJECT_NAME:-superhet}"

umask 077
out_dir="${out_dir:-./backups}"
mkdir -p "$out_dir"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
work="$(mktemp -d "${TMPDIR:-/tmp}/superhet-backup.XXXXXX")"
stopped=''
cleanup() {
    rc=$?
    rm -rf "$work"
    if [ -n "$stopped" ]; then
        say "restarting: $stopped"
        # shellcheck disable=SC2086
        docker compose start $stopped >/dev/null 2>&1 || true
    fi
    exit "$rc"
}
trap cleanup EXIT INT TERM

# PostgreSQL has to be up for pg_dump; start only it if the stack is down.
pg="$(docker compose ps -q postgres 2>/dev/null | head -n 1)"
if [ -z "$pg" ] || [ "$(docker inspect -f '{{.State.Running}}' "$pg" 2>/dev/null)" != true ]; then
    say "PostgreSQL is not running: starting it (and only it)"
    docker compose up -d --wait postgres >/dev/null
    pg="$(docker compose ps -q postgres | head -n 1)"
fi
[ -n "$pg" ] || die "could not find the postgres container"
image="$(docker inspect -f '{{.Config.Image}}' "$pg")"

if [ "$stop" = 1 ]; then
    stopped="$(docker compose ps --services --status running | grep -vx postgres | tr '\n' ' ')"
    # shellcheck disable=SC2086
    [ -z "$stopped" ] || docker compose stop $stopped >/dev/null
fi

say "dumping the database"
docker compose exec -T postgres pg_dump -U superhet -d superhet -Fc --no-owner >"$work/db.dump"
[ -s "$work/db.dump" ] || die "pg_dump produced no data"

vols=''
for v in $(docker compose config --volumes); do
    case "$v" in
    postgres-data | tile-cache | backups) continue ;;
    recordings) [ "$with_recordings" = 1 ] || continue ;;
    nats-data) [ "$with_nats" = 1 ] || continue ;;
    esac
    real="$(docker volume ls -q --filter "label=com.docker.compose.project=$project" --filter "label=com.docker.compose.volume=$v" | head -n 1)"
    [ -n "$real" ] || {
        say "volume $v does not exist yet: skipped"
        continue
    }
    say "copying volume $v"
    docker run --rm -v "$real:/v:ro" -v "$work:/out" --entrypoint tar "$image" -C /v -cf "/out/vol-$v.tar" .
    vols="$vols $v"
done

if [ "$keep_env" = 1 ]; then
    [ ! -f .env ] || cp .env "$work/env.copy"
    cp compose.yaml "$work/compose.yaml.copy"
fi
{
    echo "format=1"
    echo "created=$stamp"
    echo "project=$project"
    echo "volumes=${vols# }"
    echo "postgres=$(docker compose exec -T postgres postgres --version | tr -d '\r')"
    echo "image=$image"
} >"$work/MANIFEST"
(cd "$work" && sha256sum -- * >SHA256SUMS)

dest="$out_dir/superhet-ops-backup-$stamp.tar"
tar -C "$work" -cf "$dest.partial" .
mv "$dest.partial" "$dest"
chmod 600 "$dest"
say "backup written: $dest ($(wc -c <"$dest") bytes)"
[ "$keep_env" = 0 ] || say "note: it contains .env (secrets); store it like a password file"
