-- Bookmarks: saved frequencies / memory channels (docs/api-bookmarks.md).
CREATE TABLE superhet.bookmarks (
    id           uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    name         text        NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    freq_hz      bigint      NOT NULL CHECK (freq_hz BETWEEN 1 AND 10000000000),
    mode         text        CHECK (mode IN ('wfm', 'nfm', 'am', 'usb', 'lsb', 'cw')),
    bandwidth_hz integer     CHECK (bandwidth_hz BETWEEN 1 AND 10000000),
    tags         text[]      NOT NULL DEFAULT '{}' CHECK (cardinality(tags) <= 16),
    color        text        CHECK (color ~ '^#[0-9a-f]{6}$'),
    notes        text        CHECK (char_length(notes) <= 1000),
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX bookmarks_freq_idx ON superhet.bookmarks (freq_hz, id);

UPDATE superhet.schema_version SET version = 4, applied_at = now();
