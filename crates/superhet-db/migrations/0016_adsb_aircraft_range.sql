-- ADS-B aircraft database (registration, type, operator names) and receiver range statistics
-- (docs/plugins-adsb.md, ADR-0026).
--
-- The aircraft tables hold a public database the admin imported (never shipped with Superhet);
-- the API replaces their content in one transaction. The 24-bit ICAO address is stored as an
-- integer. The range tables are filled by the worker from the plugin's `adsb.range` events:
-- maxima per bearing sector and altitude band (idempotent GREATEST upserts) and one activity
-- row per minute.

CREATE TABLE superhet.aircraft_db (
    icao         integer  PRIMARY KEY CHECK (icao BETWEEN 0 AND 16777215),
    registration text     NOT NULL DEFAULT '' CHECK (char_length(registration) <= 32),
    type_code    text     NOT NULL DEFAULT '' CHECK (char_length(type_code) <= 16),
    -- Bit 0 military, bit 1 interesting, bit 2 PIA, bit 3 LADD (Mictronics database flags).
    flags        smallint NOT NULL DEFAULT 0
);

CREATE TABLE superhet.aircraft_types (
    code        text PRIMARY KEY CHECK (char_length(code) <= 16),
    description text NOT NULL DEFAULT '' CHECK (char_length(description) <= 128),
    -- ICAO Doc 8643 class, e.g. L2J (land, two engines, jet) or H1T (helicopter).
    class       text NOT NULL DEFAULT '' CHECK (char_length(class) <= 8),
    -- Wake turbulence category L, M, H or J.
    wake        text NOT NULL DEFAULT '' CHECK (char_length(wake) <= 2)
);

CREATE TABLE superhet.aircraft_operators (
    -- ICAO airline designator (the first three letters of a callsign such as DLH123).
    code     text PRIMARY KEY CHECK (char_length(code) <= 16),
    name     text NOT NULL DEFAULT '' CHECK (char_length(name) <= 128),
    country  text NOT NULL DEFAULT '' CHECK (char_length(country) <= 64),
    -- Radio telephony callsign, e.g. LUFTHANSA.
    callsign text NOT NULL DEFAULT '' CHECK (char_length(callsign) <= 64)
);

-- One row: where the data came from and when it was imported. No row = no database.
CREATE TABLE superhet.aircraft_db_meta (
    singleton   boolean     PRIMARY KEY DEFAULT true CHECK (singleton),
    source      text        NOT NULL,
    source_url  text        NOT NULL DEFAULT '',
    licence     text        NOT NULL,
    licence_url text        NOT NULL DEFAULT '',
    version     text        NOT NULL DEFAULT '',
    origin      text        NOT NULL CHECK (origin IN ('download', 'file')),
    aircraft    integer     NOT NULL,
    types       integer     NOT NULL,
    operators   integer     NOT NULL,
    imported_at timestamptz NOT NULL DEFAULT now()
);

-- Receiver position the statistics of a device belong to; a move of more than a few km starts
-- over (the worker clears the cells).
CREATE TABLE superhet.adsb_range_meta (
    device text             PRIMARY KEY CHECK (char_length(device) BETWEEN 1 AND 64),
    lat    double precision NOT NULL CHECK (lat BETWEEN -90 AND 90),
    lon    double precision NOT NULL CHECK (lon BETWEEN -180 AND 180),
    since  timestamptz      NOT NULL DEFAULT now()
);

-- Farthest position heard per 5-degree bearing sector (0 = north) and altitude band.
CREATE TABLE superhet.adsb_range_cells (
    device     text        NOT NULL CHECK (char_length(device) BETWEEN 1 AND 64),
    sector     smallint    NOT NULL CHECK (sector BETWEEN 0 AND 71),
    band       smallint    NOT NULL CHECK (band BETWEEN 0 AND 15),
    max_km     real        NOT NULL CHECK (max_km >= 0),
    icao       text        NOT NULL DEFAULT '',
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (device, sector, band)
);

-- One row per minute: decoder message rate and aircraft counts.
CREATE TABLE superhet.adsb_activity (
    device        text     NOT NULL CHECK (char_length(device) BETWEEN 1 AND 64),
    ts_ms         bigint   NOT NULL,
    msgs_per_s    real     NOT NULL DEFAULT 0,
    aircraft      smallint NOT NULL DEFAULT 0,
    with_position smallint NOT NULL DEFAULT 0,
    PRIMARY KEY (device, ts_ms)
);

UPDATE superhet.schema_version SET version = 16, applied_at = now();
