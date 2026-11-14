-- Bookmarks: the weak-signal modes (FT8, FT4, WSPR) and the VHF datalinks (AIS, ACARS, VDL2) of the
-- receiver's Digital picker can be saved too. Additive: existing rows are unchanged.
ALTER TABLE superhet.bookmarks DROP CONSTRAINT bookmarks_digital_protocol_check;
ALTER TABLE superhet.bookmarks
    ADD CONSTRAINT bookmarks_digital_protocol_check CHECK (digital_protocol IN
        ('tetra', 'dmr', 'dpmr', 'nxdn', 'p25', 'pocsag', 'aprs', 'ism', 'm17',
         'ft8', 'ft4', 'wspr', 'ais', 'acars', 'vdl2'));

UPDATE superhet.schema_version SET version = 21, applied_at = now();
