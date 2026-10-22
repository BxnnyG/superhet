# Host setup for RTL-SDR (Docker)

One-time preparation of the machine that runs the compose stack. Without it the container cannot
claim the dongle (`usb_claim_interface error -6` or `No supported devices found` in the
`superhet-sdrd` logs). Commands need root. Paths are relative to the repository checkout or the
extracted release bundle.

1. **Keep the kernel DVB driver away from the dongle.**
   `cp deploy/modprobe/blacklist-rtlsdr.conf /etc/modprobe.d/` and reboot (or
   `modprobe -r dvb_usb_rtl28xxu rtl2832_sdr rtl2832 rtl2830`).
2. **Allow device access.** `cp deploy/udev/20-rtlsdr.rules /etc/udev/rules.d/`, make sure the
   group `plugdev` exists (`getent group plugdev || groupadd -r plugdev`), then
   `udevadm control --reload-rules && udevadm trigger` and replug the dongle. Check with
   `ls -l /dev/bus/usb/<bus>/<dev>` (see `lsusb -d 0bda:`): group `plugdev`, mode `0660`.
3. **Tell compose which group owns the device node.** (`install.sh` detects and sets this automatically.) The services run as an unprivileged user and
   get the group via `group_add`. The default is gid 46 (`plugdev` on Debian/Ubuntu). Elsewhere put
   the real numeric gid into `.env`: `echo "SUPERHET_USB_GID=$(getent group plugdev | cut -d: -f3)" >> .env`
   (use `stat -c %g /dev/bus/usb/<bus>/<dev>` if your distro's own rules already set another group).
4. `docker compose up -d`. `docker compose logs superhet-sdrd` must show `RTL-SDR opened` and
   `starting spectrum pipeline`.

## How the container gets the dongle

`compose.yaml` bind-mounts `/dev/bus/usb` into `superhet-sdrd` and allows USB character devices
(major 189) through the device cgroup (`device_cgroup_rules`). No `privileged`, and a replugged
dongle works without recreating the container (restart `superhet-sdrd` if it exited meanwhile).

Environment (`.env`, see `.env.example`):

- `SUPERHET_DEVICE`: device spec, default `rtlsdr:0` (index, or EEPROM serial such as
  `rtlsdr:00000001`; optional `?rate=...&freq=...&gain=auto|<dB>&ppm=<n>`). Set it empty to run the
  stack without a dongle. If the device cannot be opened the daemon exits and compose restarts it.
- `SUPERHET_DEVICE_ID`: id used in API paths and bus subjects, default `dev0`.
- `SUPERHET_USB_GID`: see step 3.

More than one dongle: add another service block (copy of `superhet-sdrd` with its own
`SUPERHET_DEVICE`/`SUPERHET_DEVICE_ID`).

Notes: the RTL-SDR v5 has about 2.4 MS/s usable bandwidth and no usable HF without an upconverter.
Rootless Docker/Podman may not be able to pass USB devices through; use the rootful daemon.
