-- Per-device access policy (docs/adr/0023-device-access.md).
--
-- A device without a row behaves like the defaults below, which are today's behaviour: every
-- role sees it, nobody is held back from controlling it beyond the role their route needs, and
-- guests (if guest mode is on) see it. A row only ever narrows that.
--   view_role      lowest role that may see and listen (spectrum, audio, state, decoder output)
--   control_role   lowest role that may change anything (tune, lock, VFOs, profiles, plugins)
--   guests_visible guests may see the device (and, with control_role = 'listener', open VFOs)
--   allowed_users  when not empty, only these accounts (and admins) may use the device at all;
--                  guests are then kept out too
-- Admins always pass. Account ids are not foreign keys: a deleted account just leaves a dead
-- number behind, which matches nobody.
CREATE TABLE superhet.device_policies (
    device         text        PRIMARY KEY CHECK (char_length(device) BETWEEN 1 AND 64),
    view_role      text        NOT NULL DEFAULT 'listener'
        CHECK (view_role IN ('listener', 'operator', 'admin')),
    control_role   text        NOT NULL DEFAULT 'listener'
        CHECK (control_role IN ('listener', 'operator', 'admin')),
    guests_visible boolean     NOT NULL DEFAULT true,
    allowed_users  bigint[]    NOT NULL DEFAULT '{}'
        CHECK (cardinality(allowed_users) <= 200),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    updated_by     text
);

UPDATE superhet.schema_version SET version = 15, applied_at = now();
