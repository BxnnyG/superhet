#!/bin/sh
# Restore an ops backup made by scripts/backup.sh into a docker compose install.
#
#   scripts/restore.sh BACKUP.tar [--yes] [--restore-env] [--start] [--project NAME]
#
# Run it in the install directory (compose.yaml + .env), or set SUPERHET_DIR. The restore is
# destructive: it stops the stack (volumes are kept), REPLACES the database and the volumes that
# are in the archive, and leaves the stack stopped (--start starts it again). Without --yes it
# asks first. .env is left alone unless --restore-env (the old one is saved as .env.pre-restore).
# The archive is checked against its SHA256SUMS before anything is touched; only restore archives
# you made yourself.
set -eu

say() { printf '%s\n' "$*"; }
die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

file=''
yes=0
restore_env=0
start=0
project="${COMPOSE_PROJECT_NAME:-}"
while [ $# -gt 0 ]; do
    case "$1" in
    --yes) yes=1 ;;
    --restore-env) restore_env=1 ;;
    --start) start=1 ;;
    --project)
        [ $# -ge 2 ] || die "--project needs a name"
        project="$2"
        shift
        ;;
    -h | --help)
        sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    -*) die "unknown argument: $1 (see --help)" ;;
    *)
        [ -z "$file" ] || die "only one backup file"
        file="$1"
        ;;
    esac
    shift
done
[ -n "$file" ] || die "usage: restore.sh BACKUP.tar [--yes] [--restore-env] [--start]"
case "$file" in /*) ;; *) file="$(pwd)/$file" ;; esac
[ -f "$file" ] || die "$file not found"

cd "${SUPERHET_DIR:-.}"
[ -f compose.yaml ] || die "no compose.yaml in $(pwd); run this in the install directory or set SUPERHET_DIR"
command -v docker >/dev/null 2>&1 || die "docker not found"
docker compose version >/dev/null 2>&1 || die "docker compose v2 plugin not found"
[ -z "$project" ] || export COMPOSE_PROJECT_NAME="$project"
project="$(docker compose config --format json 2>/dev/null | sed -n 's/^  "name": "\([^"]*\)".*/\1/p' | head -n 1)"
[ -n "$project" ] || project="${COMPOSE_PROJECT_NAME:-superhet}"

umask 077
work="$(mktemp -d "${TMPDIR:-/tmp}/superhet-restore.XXXXXX")"
trap 'rm -rf "$work"' EXIT INT TERM

# Refuse paths that could leave the work directory, then unpack and verify.
if tar -tf "$file" | grep -Eq '(^|/)\.\.(/|$)|^/'; then
    die "the archive contains unsafe paths"
fi
tar -C "$work" -xf "$file"
[ -f "$work/MANIFEST" ] && [ -f "$work/db.dump" ] && [ -f "$work/SHA256SUMS" ] || die "not a Superhet ops backup (MANIFEST, db.dump or SHA256SUMS missing)"
(cd "$work" && sha256sum -c --quiet SHA256SUMS) || die "checksum mismatch: the archive is damaged or was modified"
fmt="$(sed -n 's/^format=//p' "$work/MANIFEST")"
[ "$fmt" = 1 ] || die "unsupported backup format '$fmt'"
say "backup from $(sed -n 's/^created=//p' "$work/MANIFEST") ($(sed -n 's/^postgres=//p' "$work/MANIFEST"))"
vols="$(sed -n 's/^volumes=//p' "$work/MANIFEST")"

if [ "$yes" = 0 ]; then
    printf 'This stops the stack and REPLACES the database%s of project "%s". Type "restore" to continue: ' "${vols:+ and the volumes $vols}" "$project"
    read -r answer
    [ "$answer" = restore ] || die "aborted"
fi

say "stopping the stack"
docker compose down >/dev/null 2>&1 || docker compose stop >/dev/null
docker compose up -d --wait postgres >/dev/null
pg="$(docker compose ps -q postgres | head -n 1)"
[ -n "$pg" ] || die "could not start postgres"
image="$(docker inspect -f '{{.Config.Image}}' "$pg")"

say "restoring the database"
docker compose exec -T postgres psql -q -U superhet -d postgres -v ON_ERROR_STOP=1 \
    -c 'DROP DATABASE IF EXISTS superhet WITH (FORCE)' -c 'CREATE DATABASE superhet OWNER superhet'
docker compose exec -T postgres pg_restore -U superhet -d superhet --no-owner --exit-on-error <"$work/db.dump"

for v in $vols; do
    [ -f "$work/vol-$v.tar" ] || die "vol-$v.tar is listed but missing"
    real="$(docker volume ls -q --filter "label=com.docker.compose.project=$project" --filter "label=com.docker.compose.volume=$v" | head -n 1)"
    if [ -z "$real" ]; then
        real="${project}_$v"
        docker volume create --label "com.docker.compose.project=$project" --label "com.docker.compose.volume=$v" "$real" >/dev/null
    fi
    say "restoring volume $v"
    docker run --rm -v "$real:/v" -v "$work:/in:ro" --entrypoint sh "$image" -c \
        'find /v -mindepth 1 -delete && tar -C /v -xf /in/vol-'"$v"'.tar'
done

if [ "$restore_env" = 1 ]; then
    if [ -f "$work/env.copy" ]; then
        [ ! -f .env ] || cp .env .env.pre-restore
        cp "$work/env.copy" .env
        say ".env restored (the previous one is .env.pre-restore)"
    else
        say "the backup has no .env: left as it is"
    fi
fi

if [ "$start" = 1 ]; then
    docker compose up -d >/dev/null
    say "restored; the stack is starting"
else
    docker compose stop postgres >/dev/null
    say "restored. Start the stack with: docker compose up -d"
fi
