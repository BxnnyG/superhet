-- Public receiver share links (docs/adr/0029-share-links.md).
--
-- A link is an admin-made capability: whoever holds the token may open the public receiver page
-- (/r/<token>) or the embeddable player (/embed/<token>) without an account, limited to what the
-- row allows. Only the SHA-256 of the token is stored, so a leaked dump yields no usable links
-- (the full link is shown once, at creation or rotation).
--   follow          true: listeners hear what the operator is tuned to (no VFO of their own);
--                   false: every audio listener gets a VFO at freq_hz / mode
--   tune_min_hz/max the range a listener of a fixed link may retune within (both or neither)
--   features        waterfall, audio, decoder_events, plugin_audio
--   embed_origins   origins that may frame /embed/<token> (CSP frame-ancestors); empty = none
CREATE TABLE superhet.share_links (
    id            bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    token_hash    bytea       NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    token_hint    text        NOT NULL CHECK (char_length(token_hint) <= 16),
    label         text        NOT NULL CHECK (char_length(label) BETWEEN 1 AND 80),
    title         text        NOT NULL DEFAULT '' CHECK (char_length(title) <= 120),
    description   text        NOT NULL DEFAULT '' CHECK (char_length(description) <= 1000),
    location      text        NOT NULL DEFAULT '' CHECK (char_length(location) <= 200),
    device        text        NOT NULL CHECK (char_length(device) BETWEEN 1 AND 64),
    follow        boolean     NOT NULL DEFAULT true,
    freq_hz       bigint      CHECK (freq_hz BETWEEN 1 AND 1000000000000),
    mode          text        CHECK (mode IN ('wfm', 'nfm', 'am', 'usb', 'lsb', 'cw')),
    tune_min_hz   bigint      CHECK (tune_min_hz BETWEEN 1 AND 1000000000000),
    tune_max_hz   bigint      CHECK (tune_max_hz BETWEEN 1 AND 1000000000000),
    features      text[]      NOT NULL DEFAULT '{}'
        CHECK (features <@ ARRAY['waterfall', 'audio', 'decoder_events', 'plugin_audio']::text[]),
    max_listeners integer     NOT NULL DEFAULT 5 CHECK (max_listeners BETWEEN 1 AND 1000),
    embed_origins text[]      NOT NULL DEFAULT '{}' CHECK (cardinality(embed_origins) <= 20),
    expires_at    timestamptz,
    revoked_at    timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    text,
    CHECK (
        (follow AND freq_hz IS NULL AND mode IS NULL
            AND tune_min_hz IS NULL AND tune_max_hz IS NULL)
        OR (NOT follow AND freq_hz IS NOT NULL AND mode IS NOT NULL
            AND (tune_min_hz IS NULL) = (tune_max_hz IS NULL))
    ),
    CHECK (tune_min_hz IS NULL OR (tune_min_hz <= tune_max_hz
        AND freq_hz BETWEEN tune_min_hz AND tune_max_hz))
);

UPDATE superhet.schema_version SET version = 19, applied_at = now();
