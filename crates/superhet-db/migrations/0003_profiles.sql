-- Profiles: named presets (tuning, gain, VFOs, ...) as versioned JSON (docs/api-profiles.md).
CREATE TABLE superhet.profiles (
    id         uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text        NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    data       jsonb       NOT NULL CHECK (jsonb_typeof(data) = 'object'),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- Names are unique regardless of case.
CREATE UNIQUE INDEX profiles_name_key ON superhet.profiles (lower(name));

UPDATE superhet.schema_version SET version = 3, applied_at = now();
