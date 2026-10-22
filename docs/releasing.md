# Releasing

Workflow: `.github/workflows/release.yml`. Registry: GitHub Packages (GHCR), image `ghcr.io/bxnnyg/superhet` (one image containing all service binaries and the web build; compose selects the binary via `command`).

## What gets published

| Trigger | Image tags | GitHub Release |
|---|---|---|
| push to `main` | `edge`, `sha-<short>` | no |
| tag `vX.Y.Z` | `X.Y.Z`, `X.Y`, `latest` | yes, generated notes, compose bundle |
| tag `vX.Y.Z-rc.1` (any `-suffix`) | `X.Y.Z-rc.1` only | yes, marked pre-release, compose bundle |

Images are `linux/amd64` only, carry OCI labels (source, `AGPL-3.0-only`) and include SBOM and max-mode provenance attestations.

## Cutting a release

1. Bump the version in `Cargo.toml` (`[workspace.package]`) and `web/package.json` (and refresh `Cargo.lock`) and the tag in the README "Quick start" (install URL and manual steps) via a normal PR. Prereleases (`-alpha.N`) are excluded from GitHub's `releases/latest`, so the README must name the explicit tag; once a stable release exists, `releases/latest/download/install.sh` works too. Merge the PR; merge to `main`.
2. Wait until CI is green on that `main` commit.
3. `git tag vX.Y.Z && git push origin vX.Y.Z`.
4. The workflow checks that the tag equals both versions (pre-release suffix ignored), pushes the image and creates the release with the compose bundle attached.

**GHCR package visibility:** a package created by a workflow is private by default. After the first publish set it to **public** (GitHub profile or org -> Packages -> `superhet` -> Package settings -> Change visibility) and link it to the repository. Until then `docker compose pull` of the bundle fails with `denied`/`unauthorized` for everyone without a token. This is a one-time manual step.

## Release assets

Each release carries `install.sh` and a standalone `compose.yaml` (plus `.sha256` each) and the compose bundle tarball. `compose.yaml` is generated from the repository's dev `compose.yaml` by `scripts/package-compose.sh` (single source, so no drift): build block removed, default image `ghcr.io/bxnnyg/superhet:<version>`, and `deploy/nats/nats.conf` embedded as a top-level `configs:` entry (`content:`, `$` escaped as `$$` because compose interpolates it). `install.sh` is `scripts/install.sh` with the release tag baked in as default version. See the README for the user-facing commands. The script fails if the dev compose file no longer has the expected shape.

## Compose bundle

Each release carries `superhet-<tag>-compose.tar.gz` (plus `.sha256`), built by `scripts/package-compose.sh <tag> [outdir]` in the `github-release` job. Contents: the standalone `compose.yaml` (image still overridable via `SUPERHET_IMAGE`), `deploy/{udev,modprobe}`, `scripts/gen-secrets.sh`, `.env.example`, `docs/setup-host.md`, `LICENSE`, `NOTICE` and `INSTALL.md` (template: `deploy/bundle/INSTALL.md`). Dry run locally: `scripts/package-compose.sh v0.1.0 /tmp/out`, then in an empty directory `SUPERHET_BASE_URL=file:///tmp/out SUPERHET_VERSION=v0.1.0 SUPERHET_DIR=/tmp/try sh /tmp/out/install.sh --no-start` and `docker compose config` there. A re-run of the workflow for an existing release replaces the assets (`--clobber`).

## Design notes

- Publishing is **not** gated on the CI workflow (a `workflow_run` gate would complicate tag handling). The tag-only-green-commits rule in step 2 is the gate; `edge` may therefore briefly contain a commit whose CI later fails.
- arm64 is not built: the Rust stage would run under QEMU emulation (very slow). A native `ubuntu-24.04-arm` job plus a merged manifest is the planned path.
- The release step uses the preinstalled `gh` CLI (no third-party release action) and is idempotent.
