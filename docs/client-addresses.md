# Client addresses, reverse proxies and Docker

Superhet records two addresses for every session and audit event:

| Field | Meaning |
|---|---|
| `peer_addr` | The TCP peer: whoever opened the connection to `superhet-api`. Behind a reverse proxy this is the proxy, behind Docker's port forwarding it can be the **bridge gateway** (for example `172.22.0.1`). |
| `ip` (client address) | The visitor. Equal to the peer unless the peer is a **trusted proxy** that forwarded the real address. Bans, per-address guest limits, the login rate limiter and the audit filters use this one. |

The admin console shows both, plus the address type, reverse DNS, an optional GeoIP location and
the parsed browser (Admin console, Live sessions and Audit log, also in the CSV/JSON export).

## How the client address is chosen

1. The peer is not trusted: the client is the peer. `X-Forwarded-For`, `Forwarded` and
   `X-Real-IP` are ignored (any client can send them).
2. The peer is trusted, either because `SUPERHET_TRUST_PROXY=true` (every peer) or because it lies
   in `SUPERHET_TRUSTED_PROXIES`: the first non-empty of `X-Forwarded-For`, `Forwarded` (`for=`)
   and `X-Real-IP` is read.
3. That chain is walked **from the right**. Hops inside `SUPERHET_TRUSTED_PROXIES` are skipped; the
   first other address is the client. With only `SUPERHET_TRUST_PROXY=true` (no networks) the
   rightmost entry is the client: it is the one your single proxy appended, everything to its left
   came from the visitor and is not believed.
4. A header that is not an address (`unknown`, an obfuscated token) cannot be followed: the peer is
   used.

The forwarded chain is kept in the audit event `details.forwarded` (truncated like the address).
`SUPERHET_TRUSTED_PROXIES` is a comma or space separated list of addresses and networks, for example
`172.16.0.0/12, 10.0.0.5`. Prefer it to `SUPERHET_TRUST_PROXY=true`: it only trusts the proxy.
Your proxy must **set or append** `X-Forwarded-For` (Caddy, nginx `$proxy_add_x_forwarded_for`,
Traefik and HAProxy `option forwardfor` do).

## Why the admin console shows 172.x.0.1 for everybody

Docker publishes `8073` through NAT rules and, for some connections, a userland proxy
(`docker-proxy`). Connections that the kernel NAT handles keep the source address of the visitor.
Connections that go through `docker-proxy` appear to come from the bridge gateway. That is what you
get when you:

- open the UI from the **same machine** (`http://localhost:8073`, or the host's own LAN address),
- reach it through a **reverse proxy running on the host** that talks to the published port
  (the proxy's address is the peer; fix: trust it, see below),
- run Docker **rootless**, in a VM with port forwarding, or on Docker Desktop (the forwarder is
  always in between).

A browser on another machine in your LAN with a normal rootful Docker should show its real
address. When it does not, use one of the fixes.

The admin console warns (yellow banner in *Live sessions* and *Audit log*) when every recent
address is a Docker gateway address.

## Fixes

### Behind a reverse proxy (recommended for internet exposure)

Terminate TLS in the proxy and let it forward the client address. In `.env`:

```
SUPERHET_TRUSTED_PROXIES=172.16.0.0/12      # or the proxy's exact address / the gateway
SUPERHET_COOKIE_SECURE=auto                 # becomes secure through X-Forwarded-Proto
```

then `docker compose up -d`. If the proxy runs on the Docker host and connects to the published
port, its address *is* the bridge gateway, so list the gateway network (`docker network inspect
superhet_edge` shows it).

### Without a proxy: host networking for the API (opt-in)

`deploy/compose.hostnet.yaml` puts only `superhet-api` on the host network, so the TCP peer is the
real client. Download it next to `compose.yaml` (it is also a release asset) and enable it; the
setting lives in `.env`, so `install.sh --update` keeps it:

```
curl -fsSLO https://raw.githubusercontent.com/BxnnyG/superhet/main/deploy/compose.hostnet.yaml
echo 'COMPOSE_FILE=compose.yaml:compose.hostnet.yaml' >> .env
docker compose up -d
```

Trade-offs: Linux only; the API listens on the host port directly (firewall it); PostgreSQL and NATS
get loopback-only host ports (`127.0.0.1:15432`, `127.0.0.1:14222`, password protected; change with
`SUPERHET_HOSTNET_PG_PORT` / `SUPERHET_HOSTNET_NATS_PORT`) so the API can reach them. Undo: remove
the `COMPOSE_FILE` line and `docker compose up -d`.

### Docker daemon: `userland-proxy: false`

In `/etc/docker/daemon.json` set `{"userland-proxy": false}` and restart Docker. Connections from
other machines then always use kernel NAT and keep their source address. Connections from the host
itself (`localhost`) are not covered by this and still show the gateway. It is a daemon-wide setting
(all containers); test your other containers before relying on it. Docker's own documentation has
the details and caveats for your version.

## Address privacy

Address enrichment is computed from the *stored* values, so the IP mode in *Audit settings* limits
it: a truncated address (`a.b.c.0`, guests by default) gets no reverse DNS and no city, only country
and network. With mode `none` nothing is stored or shown. The peer address follows the same mode.
Reverse DNS sends the address to your resolver; turn it off with `SUPERHET_REVERSE_DNS=false`.

## What is shown

- **Address type**: loopback, private LAN, Docker gateway, CGNAT (`100.64.0.0/10`), link-local,
  multicast, reserved, public; IPv6 is labelled as such.
- **Browser**: browser and major version, operating system, device class (desktop, mobile, tablet,
  bot, tool) from the `User-Agent`. It is client supplied text: for display, never for decisions.
- **Reverse DNS**: PTR record via the nameservers of the container's `/etc/resolv.conf`; asynchronous,
  cached (1 h, "no record" 15 min, failures 1 min), time limited; a first list view may miss names
  that appear on the next refresh.
- **GeoIP (optional, offline)**: set `SUPERHET_GEOIP_DB` to one or more MMDB files (comma
  separated) that you supply and mount into the `superhet-api` container, for example
  `/geoip/dbip-city-lite.mmdb,/geoip/dbip-asn-lite.mmdb`. Nothing is bundled or downloaded and no
  lookup leaves the machine. Fields used: `country.iso_code`, `country.names.en`, `city.names.en`,
  `autonomous_system_number`, `autonomous_system_organization`, so DB-IP Lite and MaxMind GeoLite2
  (Country, City, ASN) all work. Country and ASN files are small; City files are tens of MiB and
  are read into memory. Mount example in a `compose.override.yaml`:

  ```
  services:
    superhet-api:
      volumes:
        - ./geoip:/geoip:ro
  ```

  Licenses: **DB-IP Lite** is CC BY 4.0 and needs the attribution "IP Geolocation by DB-IP"
  (https://db-ip.com) wherever the data is shown to others; **MaxMind GeoLite2** needs a free account
  and its EULA applies (no redistribution; keep the database current). Superhet does not ship either.
  GeoIP accuracy is limited (city level is often wrong, VPNs and mobile networks hide users).

## Client-reported information

The server only sees the address a connection came from. For people on the LAN or behind Docker
that is a private or gateway address, and the browser's `User-Agent` says little about the
device. A browser can tell more, so Superhet can ask it (admin console, *Client device info*; off
by default; API: `docs/api-users.md`, "Client report"). Once per page load the browser sends:

- **Device**: User-Agent Client Hints (`platform`, `platformVersion`, `model`, `architecture`,
  `bitness`, `fullVersionList`, `mobile`; Chromium only, Firefox and Safari send what they have),
  screen size and pixel ratio, languages, time zone, touch points, `hardwareConcurrency`,
  `deviceMemory` and the network connection type where the browser exposes them.
- **Public IP (second switch, off by default)**: *Ask clients for their public IP* makes the
  browser fetch its address from a lookup service (default `https://api.ipify.org?format=json`,
  configurable) and report it. The list then shows it as "Public IP (client-reported)" next to the
  address the server sees.

What this means:

- **Everything is claimed by the client and unverified.** Anybody can send any values or run a
  modified client, and a browser can be told a different "public" address by a proxy or VPN. The UI
  marks it as such; Superhet never uses it for bans, rate limits, per-address guest limits or access.
- **A third party sees the visitor's address** when their browser contacts the lookup service
  (and that the visitor has *some* page open that calls it; Superhet sends no referrer, no cookies).
  The service's own logging and privacy policy apply. Name it in the guest privacy notice (guests
  also get a line about it on the guest page), pick a service you trust, or run your own (any
  HTTPS endpoint that returns `{"ip":"..."}` or the bare address works). The page's CSP allows
  exactly that one origin while the switch is on.
- **Address privacy still applies.** The claimed public address is stored in the address mode of
  the actor class (*Audit settings*): truncated to /24 or /48 for guests by default, not kept at all
  with mode `none`. Reports of accounts live with the session and in audit rows under your audit
  retention; guest reports live in memory until the guest session ends, plus their audit row under
  the (short) guest retention. No reverse DNS or GeoIP is done for a claimed address.
- **Blocked or failed lookups are normal** (ad blockers, offline, corporate proxies): the device
  details still arrive and the row shows why no address is available.
