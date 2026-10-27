-- Address bans (docs/api-users.md "Address bans"). A ban names a host or a network, may expire
-- and is lifted by setting `lifted_at`; rows are kept as history until the retention pruning.
CREATE TABLE superhet.ip_bans (
    id              bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    network         cidr        NOT NULL,
    reason          text        CHECK (char_length(reason) <= 200),
    created_at      timestamptz NOT NULL DEFAULT now(),
    created_by      bigint      REFERENCES superhet.users (id) ON DELETE SET NULL,
    created_by_name text        NOT NULL,
    expires_at      timestamptz,
    lifted_at       timestamptz,
    lifted_by_name  text
);

CREATE INDEX ip_bans_active_idx ON superhet.ip_bans (network) WHERE lifted_at IS NULL;
CREATE INDEX ip_bans_created_idx ON superhet.ip_bans (created_at DESC);

UPDATE superhet.schema_version SET version = 8, applied_at = now();
