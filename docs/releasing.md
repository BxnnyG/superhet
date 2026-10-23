# Releasing

Workflow: `.github/workflows/release.yml`. Registry: GitHub Packages (GHCR), image `ghcr.io/bxnnyg/superhet` (one image containing all service binaries and the web build; compose selects the binary via `command`).

## What gets published

| Trigger | Image tags | GitHub Release |
|---|---|---|
| push to `main` | `edge`, `sha-<short>` | no |
| tag `vX.Y.Z` | `X.Y.Z`, `X.Y`, `latest`, `next` | yes, generated notes, compose bundle |
| tag `vX.Y.Z-rc.1` (any `-suffix`) | `X.Y.Z-rc.1`, `next` (never `X.Y` / `latest`) | yes, marked pre-release, compose bundle |

Moving channels: `next` = newest tagged release including pre-releases, `latest` = newest stable release, `edge` = `main`. The release `compose.yaml` uses `ghcr.io/bxnnyg/superhet:${SUPERHET_TAG:-<channel>}` with the default `next` for a pre-release tag and `latest` for a stable one; `install.sh` writes `SUPERHET_TAG=<channel>` into a new `.env`. Users update with `install.sh --update` (fresh `compose.yaml` incl. the embedded NATS config, then pull and up; a plain pull only refreshes images) and pin with `SUPERHET_TAG=<version>`. `next` moves with every pushed `vX` tag, so push tags in version order (re-tagging an older version would move it back). While only pre-releases exist `latest` does not exist yet, hence the default `next`. When the first stable release ships, installs made from pre-releases stay on `next` until they switch to `SUPERHET_TAG=latest`.

Images are `linux/amd64` only, carry OCI labels (source, `AGPL-3.0-only`) and include SBOM and max-mode provenance attestations.

## Cutting a release

1. Bump the version in `Cargo.toml` (`[workspace.package]`) and `web/package.json` (and refresh `Cargo.lock`) via a normal PR. The README "Quick start" uses the version-independent `raw.githubusercontent.com/.../main/scripts/install.sh`, which resolves the newest release (pre-releases included) through the GitHub API, so no README bump is needed. Prereleases (`-alpha.N`) are excluded from GitHub's `releases/latest`; once a stable release exists, `releases/latest/download/install.sh` works too. Merge the PR to `main`.
2. Wait until CI is green on that `main` commit.
3. `git tag vX.Y.Z && git push origin vX.Y.Z`.
4. The workflow checks that the tag equals both versions (pre-release suffix ignored), pushes the image and creates the release with the compose bundle attached.

**GHCR package visibility:** a package created by a workflow is private by default. After the first publish set it to **public** (GitHub profile or org -> Packages -> `superhet` -> Package settings -> Change visibility) and link it to the repository. Until then `docker compose pull` of the bundle fails with `denied`/`unauthorized` for everyone without a token. This is a one-time manual step.

## Release assets

Each release carries `install.sh` and a standalone `compose.yaml` (plus `.sha256` each) and the compose bundle tarball. `compose.yaml` is generated from the repository's dev `compose.yaml` by `scripts/package-compose.sh` (single source, so no drift): build block removed, default image `ghcr.io/bxnnyg/superhet:${SUPERHET_TAG:-next|latest}`, and `deploy/nats/nats.conf` embedded as a top-level `configs:` entry (`content:`, `$` escaped as `$$` because compose interpolates it). `install.sh` is `scripts/install.sh` with the release tag baked in as default version (the unbaked copy on `main` asks `api.github.com/repos/BxnnyG/superhet/releases?per_page=1` for the newest release instead; the unauthenticated API limit is 60 requests/hour/IP). See the README for the user-facing commands. The script fails if the dev compose file no longer has the expected shape.

## Compose bundle

Each release carries `superhet-<tag>-compose.tar.gz` (plus `.sha256`), built by `scripts/package-compose.sh <tag> [outdir]` in the `github-release` job. Contents: the standalone `compose.yaml` (image channel via `SUPERHET_TAG`, full override via `SUPERHET_IMAGE`), `deploy/{udev,modprobe}`, `scripts/gen-secrets.sh`, `.env.example`, `docs/setup-host.md`, `LICENSE`, `NOTICE` and `INSTALL.md` (template: `deploy/bundle/INSTALL.md`). Dry run locally: `scripts/package-compose.sh v0.1.0 /tmp/out`, then in an empty directory `SUPERHET_BASE_URL=file:///tmp/out SUPERHET_VERSION=v0.1.0 SUPERHET_DIR=/tmp/try sh /tmp/out/install.sh --no-start` and `docker compose config` there. A re-run of the workflow for an existing release replaces the assets (`--clobber`).

## Design notes

- Publishing is **not** gated on the CI workflow (a `workflow_run` gate would complicate tag handling). The tag-only-green-commits rule in step 2 is the gate; `edge` may therefore briefly contain a commit whose CI later fails.
- arm64 is not built: the Rust stage would run under QEMU emulation (very slow). A native `ubuntu-24.04-arm` job plus a merged manifest is the planned path.
- The release step uses the preinstalled `gh` CLI (no third-party release action) and is idempotent.
