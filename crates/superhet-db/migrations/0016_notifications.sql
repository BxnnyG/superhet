-- Notifications (docs/adr/0026-notifications.md): rules that fire a webhook, an e-mail or a
-- browser push when a decoder event matches, plus the delivery outbox and log.

-- Where a notification goes. `config` holds the non-secret settings (URL, templates, addresses);
-- `secret` is the HMAC key / token of a webhook. It is write-only: never returned by the API,
-- never logged or audited.
CREATE TABLE superhet.notification_targets (
    id         bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name       text        NOT NULL UNIQUE CHECK (char_length(name) BETWEEN 1 AND 80),
    kind       text        NOT NULL CHECK (kind IN ('webhook', 'email', 'push')),
    config     jsonb       NOT NULL DEFAULT '{}'::jsonb,
    secret     text        CHECK (char_length(secret) BETWEEN 1 AND 512),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE superhet.notification_rules (
    id             bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name           text        NOT NULL UNIQUE CHECK (char_length(name) BETWEEN 1 AND 80),
    enabled        boolean     NOT NULL DEFAULT true,
    -- Plugin id or `core`; NULL = any source.
    source         text        CHECK (char_length(source) BETWEEN 1 AND 128),
    -- Event type (`pocsag.message`, `dtmf`); NULL = any.
    kind           text        CHECK (char_length(kind) BETWEEN 1 AND 64),
    -- [{field, op, value}], all must hold.
    conditions     jsonb       NOT NULL DEFAULT '[]'::jsonb,
    cooldown_secs  integer     NOT NULL DEFAULT 0 CHECK (cooldown_secs BETWEEN 0 AND 604800),
    max_per_hour   integer     CHECK (max_per_hour BETWEEN 1 AND 100000),
    -- Quiet hours as minutes after local midnight in the IANA zone `quiet_tz`.
    quiet_start    integer     CHECK (quiet_start BETWEEN 0 AND 1439),
    quiet_end      integer     CHECK (quiet_end BETWEEN 0 AND 1439),
    quiet_tz       text        CHECK (char_length(quiet_tz) BETWEEN 1 AND 64),
    title_template text        NOT NULL DEFAULT '' CHECK (char_length(title_template) <= 500),
    body_template  text        NOT NULL DEFAULT '' CHECK (char_length(body_template) <= 2000),
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    CHECK ((quiet_start IS NULL) = (quiet_end IS NULL) AND (quiet_start IS NULL) = (quiet_tz IS NULL))
);

CREATE TABLE superhet.notification_rule_targets (
    rule_id   bigint NOT NULL REFERENCES superhet.notification_rules (id) ON DELETE CASCADE,
    target_id bigint NOT NULL REFERENCES superhet.notification_targets (id) ON DELETE CASCADE,
    PRIMARY KEY (rule_id, target_id)
);
CREATE INDEX notification_rule_targets_target_idx ON superhet.notification_rule_targets (target_id);

-- Outbox and log in one: the matcher inserts a `pending` row per (rule, target, event), the
-- sender works them off with retries. The unique index makes a redelivered event a no-op
-- (at-least-once consumption, effectively-once enqueue). Test sends have no event id.
CREATE TABLE superhet.notification_deliveries (
    id              bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    rule_id         bigint      REFERENCES superhet.notification_rules (id) ON DELETE SET NULL,
    target_id       bigint      REFERENCES superhet.notification_targets (id) ON DELETE SET NULL,
    rule_name       text        NOT NULL CHECK (char_length(rule_name) <= 80),
    target_name     text        NOT NULL CHECK (char_length(target_name) <= 80),
    event_id        bigint,
    status          text        NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sent', 'failed')),
    attempts        integer     NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    last_error      text        CHECK (char_length(last_error) <= 500),
    -- Rendered {title, body, event}; kept so retries send exactly the same message.
    message         jsonb       NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    finished_at     timestamptz
);
CREATE UNIQUE INDEX notification_deliveries_dedupe_idx
    ON superhet.notification_deliveries (rule_id, target_id, event_id)
    WHERE event_id IS NOT NULL;
CREATE INDEX notification_deliveries_due_idx
    ON superhet.notification_deliveries (next_attempt_at) WHERE status = 'pending';
CREATE INDEX notification_deliveries_created_idx
    ON superhet.notification_deliveries (created_at DESC, id DESC);

-- The server's VAPID key pair for Web Push (one row). The private key never leaves the database.
CREATE TABLE superhet.notification_vapid (
    id            smallint    PRIMARY KEY CHECK (id = 1),
    private_pkcs8 text        NOT NULL,
    public_key    text        NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);

-- Browser push subscriptions, one per browser and account.
CREATE TABLE superhet.push_subscriptions (
    id         bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id    bigint      NOT NULL REFERENCES superhet.users (id) ON DELETE CASCADE,
    endpoint   text        NOT NULL UNIQUE CHECK (char_length(endpoint) BETWEEN 8 AND 2048),
    p256dh     text        NOT NULL CHECK (char_length(p256dh) BETWEEN 1 AND 256),
    auth       text        NOT NULL CHECK (char_length(auth) BETWEEN 1 AND 128),
    label      text        CHECK (char_length(label) <= 80),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX push_subscriptions_user_idx ON superhet.push_subscriptions (user_id);

UPDATE superhet.schema_version SET version = 16, applied_at = now();
