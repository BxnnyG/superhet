-- Decoder event history: DTMF bursts, CTCSS/DCS tone changes, RDS PS/RT/PI changes
-- (docs/api-decoder-events.md). Written by superhet-worker, read and cleared through the API.
CREATE TABLE superhet.decoder_events (
    id      bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ts      timestamptz NOT NULL,
    device  text        NOT NULL CHECK (char_length(device) BETWEEN 1 AND 64),
    vfo     integer     NOT NULL CHECK (vfo >= 0),
    freq_hz bigint      CHECK (freq_hz BETWEEN 0 AND 10000000000000),
    mode    text        CHECK (char_length(mode) <= 16),
    kind    text        NOT NULL CHECK (kind IN ('dtmf', 'ctcss', 'dcs', 'rds_ps', 'rds_rt', 'rds_pi')),
    value   text        NOT NULL CHECK (char_length(value) <= 256),
    meta    jsonb       NOT NULL DEFAULT '{}'::jsonb
);

-- History pages walk newest first; pruning and range filters use ts alone.
CREATE INDEX decoder_events_ts_idx ON superhet.decoder_events (ts DESC, id DESC);
CREATE INDEX decoder_events_kind_ts_idx ON superhet.decoder_events (kind, ts DESC, id DESC);

UPDATE superhet.schema_version SET version = 6, applied_at = now();
