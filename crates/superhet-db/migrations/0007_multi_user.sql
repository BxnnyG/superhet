-- Multi-user: roles, invites, settings and the append-only audit log (docs/adr/0011, 0012).

-- The account that exists today (the former single admin) keeps working: the column default
-- backfills it as `admin`, sessions are untouched. New rows must name their role explicitly.
ALTER TABLE superhet.users
    ADD COLUMN role                 text        NOT NULL DEFAULT 'admin'
        CHECK (role IN ('admin', 'operator', 'listener')),
    ADD COLUMN disabled_at          timestamptz,
    ADD COLUMN must_change_password boolean     NOT NULL DEFAULT false,
    ADD COLUMN last_login_at        timestamptz;
ALTER TABLE superhet.users ALTER COLUMN role DROP DEFAULT;

-- Several accounts are allowed now; "at least one active admin" is enforced by the queries.
DROP INDEX superhet.users_single_admin_key;

-- One-time invite links. Only the SHA-256 of the token is stored.
CREATE TABLE superhet.invites (
    id               bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    token_hash       bytea       NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    role             text        NOT NULL CHECK (role IN ('admin', 'operator', 'listener')),
    note             text        CHECK (char_length(note) <= 200),
    created_by       bigint      REFERENCES superhet.users (id) ON DELETE SET NULL,
    created_by_name  text        NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    expires_at       timestamptz NOT NULL,
    used_at          timestamptz,
    used_by          bigint      REFERENCES superhet.users (id) ON DELETE SET NULL,
    revoked_at       timestamptz
);

-- Small key/value documents edited on the admin page (guest policy, audit settings, limits).
CREATE TABLE superhet.settings (
    key        text        PRIMARY KEY CHECK (char_length(key) BETWEEN 1 AND 64),
    value      jsonb       NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by text
);

-- Who did what. Written by the API (accounts, guests) and by the worker (system events).
CREATE TABLE superhet.audit_events (
    id         bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ts         timestamptz NOT NULL DEFAULT now(),
    actor_kind text        NOT NULL CHECK (actor_kind IN ('user', 'guest', 'system')),
    actor_id   text        CHECK (char_length(actor_id) <= 128),
    actor_name text        CHECK (char_length(actor_name) <= 128),
    role       text        CHECK (role IN ('admin', 'operator', 'listener')),
    ip         inet,
    user_agent text        CHECK (char_length(user_agent) <= 256),
    action     text        NOT NULL CHECK (char_length(action) BETWEEN 1 AND 128),
    target     text        CHECK (char_length(target) <= 256),
    outcome    text        NOT NULL CHECK (outcome IN ('ok', 'denied', 'error')),
    details    jsonb       NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX audit_events_ts_idx ON superhet.audit_events (ts DESC, id DESC);
CREATE INDEX audit_events_action_idx ON superhet.audit_events (action, ts DESC);
CREATE INDEX audit_events_actor_idx ON superhet.audit_events (actor_kind, actor_id, ts DESC);

-- Append-only guard against bugs: UPDATE, DELETE and TRUNCATE are refused unless the session
-- opts in with `set_config('superhet.audit_maintenance', 'on', true)` inside a transaction.
-- Only retention pruning (worker) and pseudonymising a deleted user's name do that. Production
-- setups should additionally run the API under a role that has INSERT/SELECT only on this
-- table (docs/api-audit.md), which a compromised API cannot lift.
CREATE FUNCTION superhet.audit_events_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('superhet.audit_maintenance', true) = 'on' THEN
        IF TG_OP = 'DELETE' THEN
            RETURN OLD;
        END IF;
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'audit_events is append-only' USING ERRCODE = 'insufficient_privilege';
END;
$$;

CREATE TRIGGER audit_events_append_only
    BEFORE UPDATE OR DELETE ON superhet.audit_events
    FOR EACH ROW EXECUTE FUNCTION superhet.audit_events_guard();
CREATE TRIGGER audit_events_no_truncate
    BEFORE TRUNCATE ON superhet.audit_events
    FOR EACH STATEMENT EXECUTE FUNCTION superhet.audit_events_guard();

UPDATE superhet.schema_version SET version = 7, applied_at = now();
