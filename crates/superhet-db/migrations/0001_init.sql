-- Initial schema: namespace and a version marker only. Real tables arrive with features.
CREATE SCHEMA IF NOT EXISTS superhet;

CREATE TABLE superhet.schema_version (
    id          smallint PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    version     integer     NOT NULL,
    applied_at  timestamptz NOT NULL DEFAULT now()
);

INSERT INTO superhet.schema_version (version) VALUES (1);
