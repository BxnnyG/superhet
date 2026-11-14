# ADR-0030: Backup and restore

**Status:** accepted (2026-11-13)

## Context

A Superhet install accumulates configuration that is tedious to recreate: accounts and roles,
guest and device policies, bookmarks, profiles, channel plans, notification rules, share links,
plugin instances, the recordings library. Until now the only copy was the PostgreSQL volume.
Admins need to move an install, clone it, undo a bad change and recover after a disk failure -
without trusting a stranger's file (a backup upload is attacker-controllable input to an admin
endpoint) and without leaking password hashes into files that get e-mailed around.

## Decision

Two layers.

**Logical backup/restore**, in a new crate `superhet-backup` used by the API (admin endpoints)
and the worker (schedule):

- *Format*: one zip with `manifest.json` (format 1, app version, schema version = newest applied
  migration, creator, areas, sha256 and row count per file) and `data/<table>.jsonl`. Zip, not
  tar.zst: the API already depends on `zip`, no C toolchain for zstd, a central directory to check
  declared sizes before inflating, and it opens everywhere. Rows are produced with
  `to_jsonb(row)` and restored with `jsonb_populate_recordset` over the columns that exist in
  both the archive and the live table, so the format follows the schema without per-column code
  and older archives load (missing columns take their defaults). A static registry of tables
  (`tables.rs`) is the only source of table names in SQL.
- *References by name*: ids that differ between installs are not exported; accounts are
  referenced by username (OIDC links, push subscriptions, device-policy allow lists), and the
  importer resolves them against the target. Identity ids that other rows reference
  (notification targets/rules, share links, recordings) are kept and their sequences advanced.
- *Credentials are opt-in*: password hashes, invites, notification secrets, the VAPID key, push
  subscriptions and share-link token hashes only travel with the admin's explicit tick. Accounts
  imported without a hash get an invalid placeholder and cannot sign in until reset. The OIDC
  client secret is environment-only and never exported.
- *Encryption*: optional passphrase, own container (`SHBKENC1`) built from `ring` AES-256-GCM in
  64 KiB STREAM-style chunks (counter and last flag in the nonce, header as AAD) and Argon2id
  (the `argon2` crate the API already uses for passwords; header-supplied costs are capped). No
  new dependency: `ring` is already in the lock file through rustls, `age` would add a large
  tree for a format nobody else reads here. Documented well enough to decrypt with a few lines.
- *Import*: a dry run executes the real import in a transaction and rolls it back, so the report
  (per-table removed/written/skipped, warnings, conflicts) cannot drift from reality. Apply is the
  same code, committed. Newer format or schema is refused with an explanation; older schemas
  load with a warning. The importing admin's own account is kept unless they confirm
  overwriting it, and an import that would leave no active admin is refused. Areas replace;
  the audit log and decoder history append with duplicate suppression (the audit table stays
  append-only: no deletes). Uploads are streamed into anonymous temp files under a hard cap; the
  archive is opened by `superhet-backup::archive::open`, which accepts only `manifest.json`,
  `data/<registered table>.jsonl` and `recordings/<kind>/<safe stem>.<ext>`, caps entry count,
  sizes, row length and total, and verifies every hash before the database sees a row. Nothing
  is extracted by archive name; recording files get paths rebuilt from the validated kind/stem
  through `superhet-recording::files::resolve`. One operation at a time; each action is audited.
- *Recordings*: metadata is always included (running recordings become `interrupted`); the files
  only on request, streamed from disk into the archive and back, stored uncompressed.
- *Schedule*: a worker job (`SUPERHET_BACKUP_DIR`, `_SCHEDULE`, `_KEEP`, optional passphrase and
  credential/history switches) writes archives atomically with mode 0600, rotates, and publishes
  its status in the settings document `backup_status` for the admin page. No migration needed.

**Ops backup** (`scripts/backup.sh`, `restore.sh`, `install.sh --backup/--restore`): `pg_dump -Fc`
plus tar of the compose volumes, with checksums, restored into a stopped stack. This covers what
the logical archive deliberately leaves out (sessions, bans, aircraft data, plugin files) and a
whole-machine recovery.

## Consequences

- One more crate and one more admin page; the table registry must be extended when a migration
  adds configuration that should survive a restore (documented in the crate header).
- A restore replaces data under running services; their short caches (5 s) catch up, other
  accounts are signed out, plugin instances keep their old config until the plugin host restarts.
- Unencrypted archives are only integrity-checked, not authenticated: someone who can modify the
  file can also fix its manifest. Authentication comes from the passphrase; an admin should
  restore only files they trust. With credentials in the archive, a tampered file could install
  a password hash: another reason to seal archives that contain them.
- Memory stays small (rows stream in batches of 500); disk needs room for one temporary copy per
  stage (upload, decrypt, export, encrypt).
- Not covered: backing up the Docker host itself, off-site transport of archives, incremental
  backups. Scheduled archives never contain recording files.
