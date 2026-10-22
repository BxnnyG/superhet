#!/bin/sh
# Superhet installer (POSIX sh). Creates ./superhet (or $SUPERHET_DIR) with compose.yaml and .env,
# then pulls and starts the stack. Never overwrites an existing .env, never deletes volumes.
#
#   curl -fsSL https://github.com/BxnnyG/superhet/releases/download/@TAG@/install.sh | sh
#   ... | sh -s -- --no-start        # only prepare the directory, do not start
#
# Environment: SUPERHET_VERSION (tag, default @TAG@), SUPERHET_DIR (default ./superhet),
# SUPERHET_PORT (written to .env), SUPERHET_KEEP_VOLUMES=1 (do not stop on leftover volumes),
# SUPERHET_BASE_URL (download location, default: the GitHub release of that tag).
set -eu

default_version='@TAG@'
repo_url='https://github.com/BxnnyG/superhet'
start=1

for arg in "$@"; do
    case "$arg" in
    --no-start) start=0 ;;
    -h | --help)
        sed -n '2,10p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//' || true
        exit 0
        ;;
    *)
        echo "unknown argument: $arg (supported: --no-start)" >&2
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

version="${SUPERHET_VERSION:-$default_version}"
case "$version" in
*@* | '') die "no release version known; set SUPERHET_VERSION, e.g. SUPERHET_VERSION=v0.1.0-alpha.2" ;;
v*) ;;
*) version="v$version" ;;
esac
base_url="${SUPERHET_BASE_URL:-$repo_url/releases/download/$version}"
dir="${SUPERHET_DIR:-superhet}"
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

say "Superhet $version -> $dir_abs"
tmp="$(mktemp "compose.yaml.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
fetch "$base_url/compose.yaml" "$tmp" || die "could not download $base_url/compose.yaml (wrong version or no network?)"
if [ -e compose.yaml ] && ! cmp -s "$tmp" compose.yaml; then
    cp compose.yaml compose.yaml.bak
    say "compose.yaml updated (previous one saved as compose.yaml.bak)"
fi
chmod 0644 "$tmp"
mv "$tmp" compose.yaml
trap - EXIT

# --- .env ----------------------------------------------------------------------------------
if [ -e .env ]; then
    say ".env exists, keeping it untouched"
    grep -q '^SUPERHET_USB_GID=' .env || say "note: .env has no SUPERHET_USB_GID; detected $usb_gid ($usb_note)"
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
