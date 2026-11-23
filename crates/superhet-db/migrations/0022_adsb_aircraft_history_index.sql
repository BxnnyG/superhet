-- "Seen before" in the ADS-B aircraft panel (ADR-0033): first and last time an aircraft was
-- heard, how often and under which callsigns, read from the decoder history. The plugin's
-- events carry the ICAO address inside `meta.data`; this partial expression index keeps that
-- lookup instant however large the history is (without it every lookup scans all ADS-B events).
CREATE INDEX IF NOT EXISTS decoder_events_adsb_icao_idx
    ON superhet.decoder_events ((meta->'data'->>'icao'), ts DESC)
    WHERE source = 'org.superhet.decoder.adsb' AND kind = 'adsb.aircraft';

UPDATE superhet.schema_version SET version = 22, applied_at = now();
