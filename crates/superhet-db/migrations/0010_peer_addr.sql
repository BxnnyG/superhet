-- Peer vs. client address (docs/api-audit.md). `ip` stays the resolved client address (forwarded
-- header behind a trusted proxy, else the peer); `peer_ip` is the TCP peer, which behind Docker port
-- forwarding is the bridge gateway. Existing rows keep NULL: the peer was not recorded then.
ALTER TABLE superhet.sessions ADD COLUMN peer_ip text;
ALTER TABLE superhet.audit_events ADD COLUMN peer_ip inet;

UPDATE superhet.schema_version SET version = 10, applied_at = now();
