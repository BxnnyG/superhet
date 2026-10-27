-- Client-reported device details (docs/api-users.md, "Client report"): what the browser says about
-- itself, collected at session start when the admin policy allows it. Unverified by nature; the
-- shape is validated and size-limited by the API before it is stored here. Removed with the
-- session. Guests have no row (their report lives in memory with the guest session).
ALTER TABLE superhet.sessions ADD COLUMN client_report jsonb;

UPDATE superhet.schema_version SET version = 12, applied_at = now();
