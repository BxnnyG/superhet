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

1. Bump the version in `Cargo.toml` (`[workspace.package]`) and `web/package.json` (and refresh `Cargo.lock`) via a normal PR; merge to `main`.
2. Wait until CI is green on that `main` commit.
3. `git tag vX.Y.Z && git push origin vX.Y.Z`.
4. The workflow checks that the tag equals both versions (pre-release suffix ignored), pushes the image and creates the release with the compose bundle attached.

**GHCR package visibility:** a package created by a workflow is private by default. After the first publish set it to **public** (GitHub profile or org -> Packages -> `superhet` -> Package settings -> Change visibility) and link it to the repository. Until then `docker compose pull` of the bundle fails with `denied`/`unauthorized` for everyone without a token. This is a one-time manual step.

## Compose bundle

Each release carries `superhet-<tag>-compose.tar.gz` (plus `.sha256`), built by `scripts/package-compose.sh <tag> [outdir]` in the `github-release` job. Contents: `compose.yaml` (build block removed, default image `ghcr.io/bxnnyg/superhet:<version>`, still overridable via `SUPERHET_IMAGE`), `deploy/{nats,udev,modprobe}`, `scripts/gen-secrets.sh`, `.env.example`, `docs/setup-host.md`, `LICENSE`, `NOTICE` and `INSTALL.md` (template: `deploy/bundle/INSTALL.md`). The script fails if the `x-superhet` block in `compose.yaml` no longer matches the expected shape, so changes there must keep it working. Dry run locally: `scripts/package-compose.sh v0.1.0 /tmp/out`, then extract it, run `scripts/gen-secrets.sh` and `docker compose config`. A re-run of the workflow for an existing release replaces the assets (`--clobber`).

## Design notes

- Publishing is **not** gated on the CI workflow (a `workflow_run` gate would complicate tag handling). The tag-only-green-commits rule in step 2 is the gate; `edge` may therefore briefly contain a commit whose CI later fails.
- arm64 is not built: the Rust stage would run under QEMU emulation (very slow). A native `ubuntu-24.04-arm` job plus a merged manifest is the planned path.
- The release step uses the preinstalled `gh` CLI (no third-party release action) and is idempotent.
