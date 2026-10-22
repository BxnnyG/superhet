# Superhet @TAG@ – compose bundle

Runs the published image `@IMAGE@` (no local build). Needs Docker with the compose plugin and,
for a real receiver, an RTL-SDR dongle plugged into this host. Single user, self-hosted.

1. Extract and enter the directory: `tar xzf superhet-@TAG@-compose.tar.gz && cd superhet-@TAG@-compose`
2. Create `.env` with random passwords: `scripts/gen-secrets.sh` (needs `openssl`; never overwrites an existing `.env`)
3. Prepare the host once for the dongle (kernel driver blacklist, udev rules, group id): follow `docs/setup-host.md`.
   Skip this step to run without hardware: add `SUPERHET_DEVICE=` (empty) to `.env`.
4. Start: `docker compose up -d` (pulls the image; if the pull is denied the GHCR package is not public yet, see the project's release notes)
5. Open `http://<host>:8073` (change the port with `SUPERHET_PORT` in `.env`).
6. Create the admin account. The one-time setup token is in the log:
   `docker compose logs superhet-api | grep -i "setup token"`
   (or pin your own with `SUPERHET_SETUP_TOKEN`, at least 16 characters).

Check the receiver: `docker compose logs superhet-sdrd` shows `RTL-SDR opened`.

Update: set the new version in `.env` (`SUPERHET_IMAGE=ghcr.io/bxnnyg/superhet:<version>`) or use the
bundle of the new release, then `docker compose pull && docker compose up -d`. Data lives in the
named volumes `postgres-data` and `nats-data`.

Plain HTTP is meant for a trusted LAN. For access from the internet put a TLS reverse proxy in front,
set `SUPERHET_COOKIE_SECURE=true`, `SUPERHET_TRUST_PROXY=true` and `SUPERHET_ALLOWED_ORIGINS` (see `.env.example`).

Source, docs and issues: https://github.com/BxnnyG/superhet – licensed AGPL-3.0-only (see `LICENSE`, `NOTICE`).
