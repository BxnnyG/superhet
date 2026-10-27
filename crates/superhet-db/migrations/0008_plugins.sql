-- Plugin host persistence and plugin events in the decoder history (docs/plugin-contract.md).
--
-- plugin_instances is the *desired* state of the plugin host: one row per started instance,
-- written when an instance is started and removed when it is stopped. The host starts every row
-- again after a restart.
CREATE TABLE superhet.plugin_instances (
    id         text        PRIMARY KEY CHECK (char_length(id) BETWEEN 1 AND 64),
    plugin_id  text        NOT NULL CHECK (char_length(plugin_id) BETWEEN 3 AND 128),
    device     text        CHECK (char_length(device) BETWEEN 1 AND 64),
    vfo        integer     CHECK (vfo >= 0),
    freq_hz    bigint      CHECK (freq_hz BETWEEN 0 AND 10000000000000),
    config     jsonb       NOT NULL DEFAULT 'null'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Plugin events share the decoder history: `source` is the plugin id ('core' for the built-in
-- detectors), `kind` the plugin's event type (for example 'pocsag.message'). Events of an
-- instance that is not bound to a VFO have no `vfo`; an instance without a device is stored with
-- the device '-'.
ALTER TABLE superhet.decoder_events
    ADD COLUMN source text NOT NULL DEFAULT 'core' CHECK (char_length(source) BETWEEN 1 AND 128);
ALTER TABLE superhet.decoder_events ALTER COLUMN vfo DROP NOT NULL;
ALTER TABLE superhet.decoder_events DROP CONSTRAINT decoder_events_kind_check;
ALTER TABLE superhet.decoder_events ADD CONSTRAINT decoder_events_kind_check CHECK (
    (source = 'core' AND kind IN ('dtmf', 'ctcss', 'dcs', 'rds_ps', 'rds_rt', 'rds_pi'))
    OR (source <> 'core' AND char_length(kind) <= 64
        AND kind ~ '^[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*){0,7}$')
);
CREATE INDEX decoder_events_source_ts_idx ON superhet.decoder_events (source, ts DESC, id DESC);

UPDATE superhet.schema_version SET version = 8, applied_at = now();
