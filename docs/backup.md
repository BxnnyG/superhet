# Backup and restore

Design and reasons: ADR-0030. Two layers that complement each other:

1. **Logical backup** (admin console, *Backup*; API below): one zip archive with the things you
   configured, restorable into any install of the same or a newer Superhet, selectable per area,
   optionally encrypted. Use it to move, to clone, to roll back a mistake and for scheduled
   copies.
2. **Ops backup** (`scripts/backup.sh`, `scripts/restore.sh`): `pg_dump` of the whole database plus
   the Docker volumes of a compose install. Use it for disaster recovery of the whole machine.

Neither replaces the other: the logical archive is portable and reviewable, the ops backup is
complete (it also holds sessions, bans, the aircraft database, the full history).

## What the logical archive contains

| Area | Content | With "include credentials" |
|---|---|---|
| Settings | guest policy, limits, audit retention, client-report policy, OIDC settings | - (the OIDC client secret is environment-only and never stored) |
| Accounts | accounts, roles, e-mail, OIDC links | password hashes, open invites |
| Device policies | per-device view/control rules, guest visibility, account allow lists (by account name) | - |
| Bookmarks | bookmarks and groups, scanner flags | - |
| Profiles | saved receiver profiles | - |
| Channel plans | custom plans (also the scanner's channel lists) | - |
| Notifications | rules and targets | target secrets, Web Push key, browser subscriptions |
| Share links | public receiver links (ADR-0029) | the token hashes; without them nothing is exported (a link cannot work without its token) |
| Recordings (metadata) | library entries; a recording that was still running is stored as interrupted | - |
| Plugin instances | started instances and their configuration | - |
| Audit log (optional) | the audit trail | - |
| Decoder history (optional) | stored decoder events | - |
| Recording files (optional) | the audio/IQ files of finished recordings, streamed, stored uncompressed | - |

Never included: sessions, address bans, the aircraft database (download it again), ADS-B range
statistics, notification delivery history, the plugin files themselves (they live in the plugin
volume; use the ops backup), tile cache.

**Credentials are opt-in.** Without the tick, restored accounts get an unusable password hash:
they exist, but cannot sign in until you reset the password or send an invite. Ticking it puts
password hashes and secrets into the file, so set a passphrase too.

### Format

A zip file: `manifest.json` (format version, app version, database schema version, creation time,
creator, areas, and a sha256 plus row count per file) and `data/<table>.jsonl` (one JSON document
per row). With recording files there are also `recordings/<audio|iq>/<stem>.<ext>` entries. Any
unzip tool opens it; the rows are plain JSON. Zip was chosen over tar.zst because the API already
depends on it (no new compression dependency, no C build), its central directory lets the
importer check every entry's declared size before inflating a byte, and it needs no special tool.

Encrypted archives (`*.zip.enc`, magic `SHBKENC1`): AES-256-GCM in 64 KiB chunks, key from
Argon2id (64 MiB, 3 passes) and a random salt; chunk counter and a last-chunk flag are in the
nonce, the header is authenticated, so reordering, cutting or editing fails. A wrong passphrase
and a damaged file look the same on purpose. There is no recovery: lose the passphrase, lose the
backup. Passphrases need at least 8 characters.

## Restore

*Admin console, Backup, Restore a backup.*

1. Choose the file (and the passphrase if it is encrypted) and press **Check backup**. This is a
   dry run: the very same import code runs and is rolled back, so the table shows exactly what
   would be removed and written, plus warnings (older schema, accounts without credentials,
   skipped references) and conflicts.
2. Press **Restore** and confirm. Everything happens in one transaction: if anything fails,
   nothing changed.

Rules:

- The archive is untrusted input. Size cap (`SUPERHET_BACKUP_MAX_UPLOAD_MB`, default 2048), at most
  4096 entries, only `manifest.json`, `data/<known table>.jsonl` and recording files the manifest
  lists, row length cap, per-file and total uncompressed caps, every sha256 verified before the
  database is touched. Nothing is extracted by name; recording files are written to paths rebuilt
  from the validated kind and stem.
- A backup from a **newer** database schema is refused with a clear message (update first). Older
  backups load; columns they do not know keep their defaults.
- Areas replace: the tables of every area in the archive are emptied and refilled. The audit log
  and decoder history are added to what exists (duplicates skipped).
- **Your own account** (the admin doing the restore) is never removed. If the backup's version
  differs (role, e-mail, password), it is listed as a conflict and left alone unless you switch on
  *Overwrite my own account*. The restore also refuses to finish with no active admin.
- Other accounts are replaced; their sessions end. Running plugin instances keep running with
  their old configuration until the plugin host restarts.
- Every export, dry run and restore writes an audit event (`backup.export`,
  `backup.import_dry_run`, `backup.import`).

## API

All admin-only; browser requests need an allowed `Origin` like every state-changing call.

- `GET /api/v1/admin/backup` - the areas, upload limit, passphrase minimum and the schedule status.
- `POST /api/v1/admin/backup/export` - JSON `{areas?, include_credentials?, passphrase?}`; answers
  with the file (`Content-Disposition: attachment`). No `areas` = the default set.
- `POST /api/v1/admin/backup/import?dry_run=true&overwrite_self=true&areas=a,b` - the raw archive
  as body (`application/octet-stream`), passphrase percent-encoded in `X-Backup-Passphrase`.
  Answers `{"v":1,"report":{...}}`. Errors: `bad_backup`, `backup_newer` (409),
  `passphrase_required`, `wrong_passphrase`, `conflict` (409), `database_refused` (422),
  `too_large` (413), `busy` (409; one backup operation at a time).

## Scheduled backups

The worker writes archives into a directory and keeps the newest N. Set on the worker (compose
passes them through, `.env.example` lists them):

| variable | default | meaning |
|---|---|---|
| `SUPERHET_BACKUP_DIR` | unset = off | target directory (compose volume `backups` at `/data/backups`) |
| `SUPERHET_BACKUP_SCHEDULE` | `daily 03:30` | `hourly`, `daily [HH:MM]`, `weekly [mon..sun] [HH:MM]`, UTC |
| `SUPERHET_BACKUP_KEEP` | 7 | archives to keep (1-1000) |
| `SUPERHET_BACKUP_PASSPHRASE` | none | seals the archives (`*.zip.enc`) |
| `SUPERHET_BACKUP_CREDENTIALS` | false | include password hashes and secrets |
| `SUPERHET_BACKUP_HISTORY` | false | include audit log and decoder history |

Files are written as `superhet-backup-YYYYMMDD-HHMMSS.zip` with mode 0600, atomically. The
admin page shows next run, last result and the files. Scheduled archives never contain recording
files. Copy them off the machine: a backup next to the data it protects survives only software
mistakes. Get one out of the volume with
`docker compose cp superhet-worker:/data/backups/<file> .` (or mount a host path at
`/data/backups` in your own compose override).

Other variables: `SUPERHET_BACKUP_MAX_UPLOAD_MB` (api, default 2048), `SUPERHET_BACKUP_TMP` (api and
worker: where the anonymous temporary files go; default the system temp directory; export,
decryption and upload each need space for one copy of the archive).

## Ops backup of a compose install

```
sh scripts/backup.sh [--dir DIR] [--stop] [--no-env] [--with-nats] [--with-recordings]
sh scripts/restore.sh backups/superhet-ops-backup-<time>.tar [--yes] [--restore-env] [--start]
sh install.sh --backup ...        # the same, from the installer (fetches the release's copy)
sh install.sh --restore FILE ...
```

Run in the install directory (or set `SUPERHET_DIR`). `backup.sh` takes a consistent online
`pg_dump` (custom format) and tars the Docker volumes (plugin data; `--with-recordings` adds the
recordings volume, `--with-nats` the JetStream state; the tile cache and the scheduled-backup
volume are never copied). `--stop` stops the other services while the volumes are copied. The
result is one 0600 tar with checksums; it contains your `.env` (secrets) unless `--no-env`.

`restore.sh` verifies the checksums, asks for confirmation (`--yes` skips), **stops the stack**,
recreates the database from the dump, replaces the volumes in the archive and leaves the stack
stopped (`--start` starts it). `.env` is only replaced with `--restore-env` (the old one is kept as
`.env.pre-restore`). Restore only archives you made yourself: volumes are unpacked as root.

To move to a new machine: install Superhet there (`install.sh --no-start`), copy the tar over,
`restore.sh FILE --restore-env --start`.
