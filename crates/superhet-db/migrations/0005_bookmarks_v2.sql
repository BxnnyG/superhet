-- Bookmarks v2: full receiver state per bookmark (squelch, step, CTCSS/DCS, digital channel
-- parameters), ordered groups (folders), favourites and scan settings. Additive: existing rows
-- keep their values and get NULL / defaults for the new columns (docs/api-bookmarks.md).
CREATE TABLE superhet.bookmark_groups (
    id         uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text        NOT NULL CHECK (char_length(name) BETWEEN 1 AND 60),
    position   integer     NOT NULL DEFAULT 0 CHECK (position >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX bookmark_groups_name_idx ON superhet.bookmark_groups (lower(name));
CREATE INDEX bookmark_groups_position_idx ON superhet.bookmark_groups (position, id);

ALTER TABLE superhet.bookmarks
    ADD COLUMN group_id          uuid REFERENCES superhet.bookmark_groups (id) ON DELETE SET NULL,
    ADD COLUMN squelch_db        smallint CHECK (squelch_db BETWEEN -120 AND 0),
    ADD COLUMN squelch_auto      boolean NOT NULL DEFAULT false,
    ADD COLUMN step_hz           integer CHECK (step_hz BETWEEN 1 AND 10000000),
    ADD COLUMN tone_mode         text CHECK (tone_mode IN ('none', 'ctcss', 'dcs')),
    ADD COLUMN ctcss_hz          double precision CHECK (ctcss_hz BETWEEN 30 AND 300),
    ADD COLUMN dcs_code          smallint CHECK (dcs_code BETWEEN 1 AND 777),
    ADD COLUMN dcs_polarity      text CHECK (dcs_polarity IN ('N', 'R')),
    ADD COLUMN digital_protocol  text CHECK (digital_protocol IN ('tetra', 'dmr', 'dpmr', 'p25')),
    ADD COLUMN digital_timeslot  smallint CHECK (digital_timeslot BETWEEN 1 AND 4),
    ADD COLUMN digital_talkgroup bigint CHECK (digital_talkgroup BETWEEN 0 AND 16777215),
    ADD COLUMN digital_colour_code smallint CHECK (digital_colour_code BETWEEN 0 AND 15),
    ADD COLUMN digital_mcc       smallint CHECK (digital_mcc BETWEEN 0 AND 999),
    ADD COLUMN digital_mnc       smallint CHECK (digital_mnc BETWEEN 0 AND 9999),
    ADD COLUMN digital_decoder   text CHECK (char_length(digital_decoder) BETWEEN 1 AND 64),
    ADD COLUMN favourite         boolean NOT NULL DEFAULT false,
    ADD COLUMN scan_include      boolean NOT NULL DEFAULT false,
    ADD COLUMN scan_priority     smallint CHECK (scan_priority BETWEEN 0 AND 9),
    ADD COLUMN scan_dwell_ms     integer CHECK (scan_dwell_ms BETWEEN 100 AND 60000);

CREATE INDEX bookmarks_group_idx ON superhet.bookmarks (group_id);

UPDATE superhet.schema_version SET version = 5, applied_at = now();
