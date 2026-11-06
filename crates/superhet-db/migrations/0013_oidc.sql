-- Optional OpenID Connect login (docs/adr/0019-oidc-login.md).

-- Contact address of an account. Used only to link an OIDC login to an existing local account
-- when the admin enables that (verified email only); never for authentication by itself.
ALTER TABLE superhet.users
    ADD COLUMN email text CHECK (char_length(email) BETWEEN 3 AND 254);
CREATE UNIQUE INDEX users_email_lower_key ON superhet.users (lower(email)) WHERE email IS NOT NULL;

-- Which identity at which provider belongs to which account. `subject` is the provider's stable
-- `sub` claim; the email is informational (as last seen). An account has at most one identity
-- per issuer, and an identity belongs to exactly one account.
CREATE TABLE superhet.oidc_identities (
    id            bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id       bigint      NOT NULL REFERENCES superhet.users (id) ON DELETE CASCADE,
    issuer        text        NOT NULL CHECK (char_length(issuer) BETWEEN 1 AND 512),
    subject       text        NOT NULL CHECK (char_length(subject) BETWEEN 1 AND 255),
    email         text        CHECK (char_length(email) <= 254),
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_login_at timestamptz,
    UNIQUE (issuer, subject),
    UNIQUE (user_id, issuer)
);

UPDATE superhet.schema_version SET version = 13, applied_at = now();
