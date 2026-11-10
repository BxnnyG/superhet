-- OIDC logout (docs/adr/0024-oidc-logout.md): sessions that came from an OIDC login remember
-- the provider (`oidc_issuer`), the provider's session id (`oidc_sid`, the ID token's `sid`
-- claim, if any) and the ID token itself (`oidc_id_token`, sent back as `id_token_hint` when the
-- user signs out and the provider is asked to end its session as well). A back-channel logout
-- token from the provider names `sid` and/or `sub` and ends the matching rows.
ALTER TABLE superhet.sessions
    ADD COLUMN oidc_issuer   text CHECK (char_length(oidc_issuer) BETWEEN 1 AND 512),
    ADD COLUMN oidc_sid      text CHECK (char_length(oidc_sid) BETWEEN 1 AND 512),
    ADD COLUMN oidc_id_token text CHECK (char_length(oidc_id_token) <= 16384);
CREATE INDEX sessions_oidc_sid_idx ON superhet.sessions (oidc_issuer, oidc_sid)
    WHERE oidc_sid IS NOT NULL;

UPDATE superhet.schema_version SET version = 14, applied_at = now();
