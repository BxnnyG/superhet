-- Local authentication: a single admin account and server-side sessions (docs/adr/0008).
CREATE TABLE superhet.users (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    username      text        NOT NULL CHECK (char_length(username) BETWEEN 1 AND 64),
    password_hash text        NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);

-- Case-insensitive unique names, and at most one row at all: Superhet is a single-user
-- system, and the constant index expression makes concurrent first-run setups race-safe.
CREATE UNIQUE INDEX users_username_lower_key ON superhet.users (lower(username));
CREATE UNIQUE INDEX users_single_admin_key ON superhet.users ((true));

-- `id` is the SHA-256 of the cookie token; the token itself is never stored.
CREATE TABLE superhet.sessions (
    id           bytea       PRIMARY KEY CHECK (octet_length(id) = 32),
    user_id      bigint      NOT NULL REFERENCES superhet.users (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    user_agent   text,
    ip           text
);

CREATE INDEX sessions_user_id_idx ON superhet.sessions (user_id);
CREATE INDEX sessions_created_at_idx ON superhet.sessions (created_at);
CREATE INDEX sessions_last_seen_at_idx ON superhet.sessions (last_seen_at);

UPDATE superhet.schema_version SET version = 2, applied_at = now();
