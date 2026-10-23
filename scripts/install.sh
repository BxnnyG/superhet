#!/bin/sh
# Superhet installer (POSIX sh). Creates ./superhet (or $SUPERHET_DIR) with compose.yaml and .env,
# then pulls and starts the stack. Never overwrites an existing .env, never deletes volumes.
#
#   curl -fsSL https://raw.githubusercontent.com/BxnnyG/superhet/main/scripts/install.sh | sh
#   ... | sh -s -- --no-start        # only prepare the directory, do not start
#   ... | sh -s -- --update          # update an existing install (run in the install dir)
#
# The raw URL always works: it fetches compose.yaml from the newest GitHub release (pre-releases
# included). The copy attached to a release has that release baked in instead (@TAG@ = the tag).
# The stack follows a moving image channel (SUPERHET_TAG in .env): next (newest release incl.
# pre-releases), latest (newest stable), edge (main), or a fixed version.
#
# --update is the supported way to update: it fetches the compose.yaml (which embeds the NATS
# config) of the release matching the channel in .env, verifies its sha256, keeps .env, saves the
# old file as compose.yaml.bak, then runs `docker compose pull && docker compose up -d`. A plain
# `docker compose pull` only refreshes images and leaves an outdated compose.yaml/NATS config in
# place, which can silently break newer images.
#
# Environment: SUPERHET_VERSION (release tag to fetch compose.yaml from; default: baked-in tag or
# the newest release), SUPERHET_TAG (channel/version written to .env; default: next for
# pre-releases, latest for stable), SUPERHET_DIR (default ./superhet), SUPERHET_PORT (written to
# .env), SUPERHET_KEEP_VOLUMES=1 (do not stop on leftover volumes), SUPERHET_BASE_URL (download
# location, default: the GitHub release of that tag).
set -eu

default_version='@TAG@'
repo_url='https://github.com/BxnnyG/superhet'
start=1
update=0

for arg in "$@"; do
    case "$arg" in
    --no-start) start=0 ;;
    --update) update=1 ;;
    -h | --help)
        sed -n '2,24p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//' || true
        exit 0
        ;;
    *)
        echo "unknown argument: $arg (supported: --no-start, --update)" >&2
        exit 2
        ;;
    esac
done

say() { printf '%s\n' "$*"; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }
die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

fetch_stdout() { # fetch_stdout URL
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O - "$1"
    else
        die "neither curl nor wget found"
    fi
}

dir="${SUPERHET_DIR:-}"
env_tag=''
if [ "$update" = 1 ]; then
    # Update mode: act on the existing install (current directory if it looks like one).
    if [ -z "$dir" ]; then
        if [ -f .env ] && [ -f compose.yaml ]; then dir=.; else dir=superhet; fi
    fi
    [ -f "$dir/.env" ] || die "no installation found in $dir (no .env); run this in the install directory or set SUPERHET_DIR, or omit --update for a fresh install"
    env_tag="$(sed -n 's/^SUPERHET_TAG=//p' "$dir/.env" | tail -n 1 | tr -d "\"' \r")"
    default_version=''
fi
dir="${dir:-superhet}"

latest_release() { # latest_release stable|any
    if [ "$1" = stable ]; then
        url="https://api.github.com/repos/BxnnyG/superhet/releases/latest"
    else
        url="https://api.github.com/repos/BxnnyG/superhet/releases?per_page=1"
    fi
    fetch_stdout "$url" 2>/dev/null |
        sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1 || true
}

version="${SUPERHET_VERSION:-$default_version}"
if [ -z "$version" ] && [ "$update" = 1 ]; then
    # The release matching the channel: latest -> newest stable, a fixed version -> that release,
    # next/edge/unset -> newest release (pre-releases included).
    case "$env_tag" in
    latest) version="$(latest_release stable)" ;;
    [0-9]* | v[0-9]*) version="$env_tag" ;;
    *) version="$(latest_release any)" ;;
    esac
    [ -n "$version" ] || die "could not determine the release for channel '${env_tag:-next}' (GitHub API unreachable or rate-limited?); set SUPERHET_VERSION, e.g. SUPERHET_VERSION=v0.1.0-alpha.3"
fi
case "$version" in
*@* | '')
    # Raw script from the repository: resolve the newest release (pre-releases included).
    if [ -n "${SUPERHET_BASE_URL:-}" ]; then
        die "SUPERHET_BASE_URL needs SUPERHET_VERSION too, e.g. SUPERHET_VERSION=v0.1.0-alpha.3"
    fi
    version="$(latest_release any)"
    [ -n "$version" ] || die "could not determine the newest release (GitHub API unreachable or rate-limited?); set SUPERHET_VERSION, e.g. SUPERHET_VERSION=v0.1.0-alpha.3"
    ;;
esac
case "$version" in
v*) ;;
*) version="v$version" ;;
esac
# Image channel written to .env: pre-release -> next (moves with every release), stable -> latest.
case "$version" in
*-*) channel=next ;;
*) channel=latest ;;
esac
channel="${SUPERHET_TAG:-$channel}"
[ "$update" = 0 ] || channel="${env_tag:-$channel}"
base_url="${SUPERHET_BASE_URL:-$repo_url/releases/download/$version}"
project="${COMPOSE_PROJECT_NAME:-superhet}"

# --- prerequisites -------------------------------------------------------------------------
command -v docker >/dev/null 2>&1 || die "docker not found; install Docker Engine: https://docs.docker.com/engine/install/"
docker compose version >/dev/null 2>&1 || die "docker compose v2 plugin not found: https://docs.docker.com/compose/install/linux/"
if ! docker info >/dev/null 2>&1; then
    die "cannot talk to the Docker daemon (not running, or no permission: add your user to the 'docker' group or run with sudo)"
fi

fetch() { # fetch URL DEST
    case "$1" in
    file://*) cp "${1#file://}" "$2" ;;
    *)
        if command -v curl >/dev/null 2>&1; then
            curl -fsSL "$1" -o "$2"
        elif command -v wget >/dev/null 2>&1; then
            wget -q -O "$2" "$1"
        else
            die "neither curl nor wget found"
        fi
        ;;
    esac
}

rand() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -hex 24
    else
        od -An -N24 -tx1 /dev/urandom | tr -d ' \n'
        echo
    fi
}

# --- RTL-SDR detection ---------------------------------------------------------------------
# Prints the numeric gid owning the first RTL-SDR's /dev/bus/usb node; nothing if none found.
detect_device_gid() {
    for d in /sys/bus/usb/devices/*; do
        [ -r "$d/idVendor" ] && [ -r "$d/idProduct" ] || continue
        [ "$(cat "$d/idVendor")" = 0bda ] || continue
        case "$(cat "$d/idProduct")" in 2838 | 2832) ;; *) continue ;; esac
        bus="$(cat "$d/busnum")"
        dev="$(cat "$d/devnum")"
        node="$(printf '/dev/bus/usb/%03d/%03d' "$bus" "$dev")"
        if [ -e "$node" ]; then
            stat -c %g "$node"
            return 0
        fi
    done
    return 1
}

# Group named in an installed RTL-SDR udev rule (GROUP="..."), as numeric gid.
detect_rule_gid() {
    for f in /etc/udev/rules.d/*rtl*sdr* /usr/lib/udev/rules.d/*rtl*sdr* /lib/udev/rules.d/*rtl*sdr*; do
        [ -r "$f" ] || continue
        g="$(sed -n 's/.*GROUP="\([^"]*\)".*/\1/p' "$f" | head -n 1)"
        [ -n "$g" ] || continue
        gid="$(getent group "$g" 2>/dev/null | cut -d: -f3)"
        if [ -n "$gid" ]; then
            printf '%s %s\n' "$gid" "$g"
            return 0
        fi
    done
    return 1
}

usb_gid=''
usb_note=''
if gid="$(detect_device_gid)" && [ -n "$gid" ]; then
    usb_gid="$gid"
    usb_note="owner group of the attached RTL-SDR's USB node"
    [ "$gid" != 0 ] || warn "the RTL-SDR node belongs to root; install the udev rules (see $repo_url/blob/main/docs/setup-host.md)"
elif rule="$(detect_rule_gid)"; then
    usb_gid="${rule%% *}"
    usb_note="group '${rule#* }' from the installed RTL-SDR udev rule (no dongle attached right now)"
else
    usb_gid=46
    usb_note="default (plugdev on Debian/Ubuntu); no RTL-SDR or udev rule found"
fi

# --- directory + compose.yaml --------------------------------------------------------------
mkdir -p "$dir"
cd "$dir"
dir_abs="$(pwd)"

say "Superhet $version (image channel: $channel) -> $dir_abs"
tmp="$(mktemp "compose.yaml.XXXXXX")"
trap 'rm -f "$tmp" "$tmp.sha256"' EXIT
fetch "$base_url/compose.yaml" "$tmp" || die "could not download $base_url/compose.yaml (wrong version or no network?)"

# Verify the download against the sha256 published next to it in the release.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        return 1
    fi
}
if fetch "$base_url/compose.yaml.sha256" "$tmp.sha256" 2>/dev/null; then
    expected="$(cut -d' ' -f1 "$tmp.sha256" | head -n 1)"
    if actual="$(sha256_of "$tmp")"; then
        [ "$expected" = "$actual" ] || die "sha256 mismatch for compose.yaml from $base_url (expected $expected, got $actual); nothing was changed"
        say "compose.yaml sha256 verified"
    else
        warn "no sha256sum/shasum available, skipping checksum verification"
    fi
else
    warn "no compose.yaml.sha256 published for $version, skipping checksum verification"
fi

compose_changed=0
if [ -e compose.yaml ] && ! cmp -s "$tmp" compose.yaml; then
    cp compose.yaml compose.yaml.bak
    compose_changed=1
    say "compose.yaml updated (previous one saved as compose.yaml.bak)"
elif [ -e compose.yaml ]; then
    say "compose.yaml already up to date"
fi
chmod 0644 "$tmp"
mv "$tmp" compose.yaml
trap - EXIT
rm -f "$tmp.sha256"

# --- .env ----------------------------------------------------------------------------------
if [ -e .env ]; then
    say ".env exists, keeping it untouched"
    grep -q '^SUPERHET_USB_GID=' .env || say "note: .env has no SUPERHET_USB_GID; detected $usb_gid ($usb_note)"
    if ! grep -q -e '^SUPERHET_TAG=' -e '^SUPERHET_IMAGE=' .env; then
        say "note: .env has no SUPERHET_TAG, so compose.yaml uses its default channel ($channel). Add SUPERHET_TAG=next|latest|edge|<version> to choose."
    fi
else
    # A leftover database volume was initialised with a password we no longer know: the api
    # would crash-loop with "password authentication failed" (28P01). Never delete it ourselves.
    if [ "${SUPERHET_KEEP_VOLUMES:-0}" != 1 ] &&
        docker volume ls -q 2>/dev/null | grep -qx "${project}_postgres-data"; then
        cat >&2 <<EOM
ERROR: a Docker volume '${project}_postgres-data' from an earlier installation exists, but there is
no .env here, so its database password is unknown. The stack would crash-loop with
"password authentication failed for user superhet".
  - Throw the old data away:  docker compose -p $project down -v   (from the old directory), or
                              docker volume rm ${project}_postgres-data ${project}_nats-data
  - Or restore the old .env into $dir_abs and run this installer again.
  - Or ignore this check:     SUPERHET_KEEP_VOLUMES=1 (only if you know what you do).
Nothing was changed except compose.yaml.
EOM
        exit 1
    fi
    umask 077
    {
        echo "# Generated by install.sh. Do not commit or share."
        for var in POSTGRES_PASSWORD NATS_API_PASSWORD NATS_SDRD_PASSWORD NATS_PLUGINHOST_PASSWORD NATS_WORKER_PASSWORD; do
            echo "$var=$(rand)"
        done
        echo "SUPERHET_USB_GID=$usb_gid"
        echo "# Image channel followed by 'docker compose pull && docker compose up -d':"
        echo "#   next   newest release, pre-releases included (default while there is no stable release)"
        echo "#   latest newest stable release"
        echo "#   edge   development build of main"
        echo "#   or a fixed version such as 0.1.0-alpha.3 to pin"
        echo "SUPERHET_TAG=$channel"
        [ -z "${SUPERHET_PORT:-}" ] || echo "SUPERHET_PORT=$SUPERHET_PORT"
    } >.env
    say "created .env with random passwords"
    say "SUPERHET_USB_GID=$usb_gid ($usb_note)"
fi

# --- host hints ----------------------------------------------------------------------------
if [ -r /proc/modules ] && grep -q '^dvb_usb_rtl28xxu ' /proc/modules; then
    warn "kernel module dvb_usb_rtl28xxu is loaded and will claim the dongle. Blacklist it:"
    say "  see $repo_url/blob/main/docs/setup-host.md (modprobe blacklist + udev rules)"
fi
if [ -z "$(detect_device_gid || true)" ]; then
    say "No RTL-SDR attached. Plug it in (then restart: docker compose restart superhet-sdrd) or"
    say "run without hardware by adding an empty 'SUPERHET_DEVICE=' line to $dir_abs/.env."
fi

docker compose config -q || die "compose.yaml / .env do not validate"

if [ "$start" = 0 ]; then
    say "Prepared. Start with: cd $dir_abs && docker compose pull && docker compose up -d"
    exit 0
fi

# --- start ---------------------------------------------------------------------------------
docker compose pull
docker compose up -d

if [ "$update" = 1 ]; then
    say ""
    if [ "$compose_changed" = 1 ]; then
        say "Updated to $version: new compose.yaml (incl. NATS config) and images are active. Previous compose.yaml: compose.yaml.bak"
    else
        say "Updated images for $version; compose.yaml was already current."
    fi
    say "Check: docker compose ps"
    exit 0
fi

port="$(sed -n 's/^SUPERHET_PORT=//p' .env | tail -n 1)"
port="${port:-8073}"
ip="$(hostname -I 2>/dev/null | cut -d' ' -f1 || true)"
say ""
say "Superhet is starting. Open http://${ip:-localhost}:$port"
token=''
esc="$(printf '\033')"
i=0
while [ "$i" -lt 30 ] && [ -z "$token" ]; do
    token="$(docker compose logs superhet-api 2>/dev/null | sed "s/$esc\[[0-9;]*m//g" |
        sed -n 's/.*one-time setup token: \([^ ]*\).*/\1/p' | tail -n 1 || true)"
    [ -n "$token" ] || sleep 2
    i=$((i + 1))
done
if [ -n "$token" ]; then
    say "One-time setup token for the first admin account: $token"
else
    say "Setup token for the first admin account:"
    say "  cd $dir_abs && docker compose logs superhet-api | grep -i 'setup token'"
fi
say "Receiver check: docker compose logs superhet-sdrd | grep 'RTL-SDR opened'"
