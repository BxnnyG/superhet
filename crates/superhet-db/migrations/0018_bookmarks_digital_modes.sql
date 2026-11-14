-- Bookmarks: every digital mode of the receiver's Digital picker can be saved (POCSAG, APRS, ISM,
-- M17, NXDN next to TETRA, DMR, dPMR, P25). Additive: existing rows are unchanged.
ALTER TABLE superhet.bookmarks DROP CONSTRAINT bookmarks_digital_protocol_check;
ALTER TABLE superhet.bookmarks
    ADD CONSTRAINT bookmarks_digital_protocol_check CHECK (digital_protocol IN
        ('tetra', 'dmr', 'dpmr', 'nxdn', 'p25', 'pocsag', 'aprs', 'ism', 'm17'));

UPDATE superhet.schema_version SET version = 18, applied_at = now();
