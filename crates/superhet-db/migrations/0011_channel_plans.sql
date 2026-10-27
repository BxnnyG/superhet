-- Custom channel plans: lists of fixed channels added by operators (docs/api-channel-plans.md).
-- The built-in plans ship with the web UI; this table only holds what was imported. The API
-- validates the document, the CHECKs are the last line of defence.
CREATE TABLE superhet.channel_plans (
    id         uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text        NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    doc        jsonb       NOT NULL CHECK (jsonb_typeof(doc) = 'object'),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX channel_plans_name_idx ON superhet.channel_plans (lower(name), id);

UPDATE superhet.schema_version SET version = 11, applied_at = now();
