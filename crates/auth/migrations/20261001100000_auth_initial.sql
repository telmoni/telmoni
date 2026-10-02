CREATE SCHEMA IF NOT EXISTS auth;

-- ══════════════════════════════════════════════════════════════════════════════
-- ROLE BOOTSTRAP & SCHEMA GRANTS
-- ══════════════════════════════════════════════════════════════════════════════
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'auth_maintenance') THEN
        CREATE ROLE auth_maintenance NOLOGIN;
    END IF;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN unique_violation THEN NULL;
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'auth_maintenance not created here (no CREATEROLE) — role_hardening.sql owns it in prod';
END $$;

DO $$
BEGIN
    GRANT auth_maintenance TO auth WITH INHERIT FALSE, SET TRUE;
EXCEPTION
    WHEN undefined_object THEN NULL;
    WHEN insufficient_privilege THEN NULL;
END $$;

GRANT USAGE ON SCHEMA auth TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 1. IDENTITIES
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.identities (
    user_id         TEXT        PRIMARY KEY,
    -- Not UNIQUE: the provider owns uniqueness, and a UNIQUE here once locked
    -- out a person whose address changed at the provider first.
    email           TEXT        NOT NULL,
    email_verified  BOOLEAN     NOT NULL,
    first_name      TEXT,
    last_name       TEXT,
    display_name    TEXT,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    CONSTRAINT identities_email_lowercase_check CHECK (email = lower(email))
);

CREATE INDEX identities_email_idx ON auth.identities (email);
CREATE INDEX identities_shard_key_idx ON auth.identities (shard_key);

ALTER TABLE auth.identities ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.identities FORCE ROW LEVEL SECURITY;

CREATE POLICY person_isolation ON auth.identities
    USING      (user_id = current_setting('app.user_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true));

CREATE POLICY maintenance_access ON auth.identities
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT ON auth.identities TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 2. ACCOUNTS
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.accounts (
    user_id               TEXT        PRIMARY KEY REFERENCES auth.identities (user_id) ON DELETE CASCADE,
    analytics_opt_in      BOOLEAN     NOT NULL DEFAULT false,
    deletion_requested_at TIMESTAMPTZ,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key             UUID        NOT NULL DEFAULT gen_random_uuid()
);

CREATE INDEX accounts_shard_key_idx ON auth.accounts (shard_key);
CREATE INDEX accounts_pending_deletion_idx ON auth.accounts (deletion_requested_at)
    WHERE deletion_requested_at IS NOT NULL;

-- No roster policy, and that is the point of the table: the person and the lane, nobody else.
ALTER TABLE auth.accounts ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.accounts FORCE ROW LEVEL SECURITY;

CREATE POLICY person_isolation ON auth.accounts
    USING      (user_id = current_setting('app.user_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true));

CREATE POLICY maintenance_access ON auth.accounts
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT ON auth.accounts TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 3. ORGANIZATIONS
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.organizations (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    external_id           TEXT        NOT NULL UNIQUE,
    name                  TEXT,
    status                TEXT        NOT NULL DEFAULT 'active',
    deletion_requested_at TIMESTAMPTZ,
    erase_after           TIMESTAMPTZ,
    deletion_kind         TEXT,
    hook_purged_at        TIMESTAMPTZ,
    shard_key             UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT organizations_status_check CHECK (status IN ('active', 'pending_deletion', 'deleted')),
    CONSTRAINT organizations_deletion_kind_check
        CHECK (deletion_kind IS NULL OR deletion_kind IN ('owner', 'account', 'operator')),
    CONSTRAINT organizations_pending_deletion_shape_check CHECK (
        (status = 'pending_deletion')
        = (deletion_requested_at IS NOT NULL AND erase_after IS NOT NULL AND deletion_kind IS NOT NULL)
        AND (hook_purged_at IS NULL OR status = 'pending_deletion')
    )
);

CREATE INDEX organizations_shard_key_idx ON auth.organizations (shard_key);
CREATE INDEX organizations_pending_deletion_idx ON auth.organizations (erase_after)
    WHERE status = 'pending_deletion';

ALTER TABLE auth.organizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.organizations FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON auth.organizations
    USING      (external_id = current_setting('app.organization_id', true))
    WITH CHECK (external_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON auth.organizations
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT ON auth.organizations TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 4. ORGANIZATION MEMBERS
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.organization_members (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id     TEXT        NOT NULL REFERENCES auth.organizations (external_id) ON DELETE CASCADE,
    user_id             TEXT        NOT NULL REFERENCES auth.identities (user_id) ON DELETE RESTRICT,
    role                TEXT        NOT NULL,
    added_by            TEXT        NOT NULL,
    transfer_offered_at TIMESTAMPTZ,
    shard_key           UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT organization_members_role_check CHECK (role IN ('owner', 'admin', 'member')),
    CONSTRAINT organization_members_organization_member_key UNIQUE (organization_id, user_id),
    CONSTRAINT organization_members_offer_to_admin_check
        CHECK (transfer_offered_at IS NULL OR role = 'admin')
);

CREATE INDEX organization_members_organization_idx ON auth.organization_members (organization_id, created_at DESC);
CREATE INDEX organization_members_user_idx         ON auth.organization_members (user_id, created_at DESC);
CREATE INDEX organization_members_shard_key_idx    ON auth.organization_members (shard_key);
CREATE UNIQUE INDEX organization_members_one_owner_key ON auth.organization_members (organization_id)
    WHERE role = 'owner';
CREATE UNIQUE INDEX organization_members_one_offer_key ON auth.organization_members (organization_id)
    WHERE transfer_offered_at IS NOT NULL;

ALTER TABLE auth.organization_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.organization_members FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON auth.organization_members
    USING      (organization_id = current_setting('app.organization_id', true))
    WITH CHECK (organization_id = current_setting('app.organization_id', true));

CREATE POLICY member_read ON auth.organization_members
    FOR SELECT
    USING (user_id = current_setting('app.user_id', true));

CREATE POLICY maintenance_access ON auth.organization_members
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, INSERT, UPDATE, DELETE ON auth.organization_members TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 5. ORGANIZATION INVITES
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.organization_invites (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id TEXT        NOT NULL REFERENCES auth.organizations (external_id) ON DELETE CASCADE,
    email           TEXT        NOT NULL,
    role            TEXT        NOT NULL,
    token_hash      TEXT        NOT NULL UNIQUE,
    invited_by      TEXT        NOT NULL,
    expires_at      TIMESTAMPTZ NOT NULL,
    accepted_at     TIMESTAMPTZ,
    accepted_by     TEXT,
    revoked_at      TIMESTAMPTZ,
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT organization_invites_role_check CHECK (role IN ('admin', 'member')),
    CONSTRAINT organization_invites_email_lowercase_check CHECK (email = lower(email))
);

CREATE UNIQUE INDEX organization_invites_live_key ON auth.organization_invites (organization_id, email)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;
CREATE INDEX organization_invites_organization_idx ON auth.organization_invites (organization_id, created_at DESC);
-- `/me`'s incoming list is keyed on the address alone, across every organization.
CREATE INDEX organization_invites_live_email_idx ON auth.organization_invites (email)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;
CREATE INDEX organization_invites_accepted_by_idx ON auth.organization_invites (accepted_by)
    WHERE accepted_by IS NOT NULL;
CREATE INDEX organization_invites_shard_key_idx ON auth.organization_invites (shard_key);

ALTER TABLE auth.organization_invites ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.organization_invites FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON auth.organization_invites
    USING      (organization_id = current_setting('app.organization_id', true))
    WITH CHECK (organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON auth.organization_invites
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, UPDATE, DELETE ON auth.organization_invites TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 6. PROJECTS
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.projects (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    external_id           TEXT        NOT NULL UNIQUE,
    organization_id       TEXT        NOT NULL REFERENCES auth.organizations (external_id) ON DELETE CASCADE,
    name                  TEXT        NOT NULL,
    status                TEXT        NOT NULL DEFAULT 'active',
    deletion_requested_at TIMESTAMPTZ,
    shard_key             UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT projects_status_check CHECK (status IN ('active', 'pending_deletion', 'deleted')),
    CONSTRAINT projects_organization_project_key UNIQUE (organization_id, external_id)
);

CREATE INDEX projects_shard_key_idx ON auth.projects (shard_key);
CREATE UNIQUE INDEX projects_organization_name_key ON auth.projects (organization_id, lower(name));

ALTER TABLE auth.projects ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.projects FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON auth.projects
    USING      (organization_id = current_setting('app.organization_id', true))
    WITH CHECK (organization_id = current_setting('app.organization_id', true));

CREATE POLICY project_self_read ON auth.projects
    FOR SELECT
    USING (external_id = current_setting('app.project_id', true));

CREATE POLICY maintenance_access ON auth.projects
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, INSERT, UPDATE, DELETE ON auth.projects TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 7. PROJECT MEMBERS
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.project_members (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id          TEXT        NOT NULL REFERENCES auth.projects (external_id) ON DELETE CASCADE,
    user_id             TEXT        NOT NULL REFERENCES auth.identities (user_id) ON DELETE RESTRICT,
    role                TEXT        NOT NULL,
    added_by            TEXT        NOT NULL,
    -- The owner's live offer of the project to this admin, as on the
    -- organization's roster: the seat carries it, so removing or demoting the
    -- admin ends it with no bookkeeping of its own.
    transfer_offered_at TIMESTAMPTZ,
    shard_key           UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT project_members_role_check CHECK (role IN ('admin', 'member')),
    CONSTRAINT project_members_project_member_key UNIQUE (project_id, user_id),
    CONSTRAINT project_members_offer_to_admin_check
        CHECK (transfer_offered_at IS NULL OR role = 'admin')
);

CREATE INDEX project_members_project_idx ON auth.project_members (project_id, created_at DESC);
CREATE INDEX project_members_user_idx    ON auth.project_members (user_id, created_at DESC);
CREATE INDEX project_members_shard_key_idx ON auth.project_members (shard_key);
CREATE UNIQUE INDEX project_members_one_offer_key ON auth.project_members (project_id)
    WHERE transfer_offered_at IS NOT NULL;

ALTER TABLE auth.project_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.project_members FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON auth.project_members
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

CREATE POLICY member_read ON auth.project_members
    FOR SELECT
    USING (user_id = current_setting('app.user_id', true));

CREATE POLICY maintenance_access ON auth.project_members
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, INSERT, UPDATE, DELETE ON auth.project_members TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 8. MEMBER INVITES (PROJECTS)
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.member_invites (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  TEXT        NOT NULL REFERENCES auth.projects (external_id) ON DELETE CASCADE,
    email       TEXT        NOT NULL,
    role        TEXT        NOT NULL,
    token_hash  TEXT        NOT NULL UNIQUE,
    invited_by  TEXT        NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,
    accepted_by TEXT,
    revoked_at  TIMESTAMPTZ,
    shard_key   UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT member_invites_role_check CHECK (role IN ('admin', 'member')),
    CONSTRAINT member_invites_email_lowercase_check CHECK (email = lower(email)),
    CONSTRAINT member_invites_accepted_pair_check
        CHECK ((accepted_at IS NULL) = (accepted_by IS NULL))
);

CREATE UNIQUE INDEX member_invites_live_key ON auth.member_invites (project_id, email)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;
CREATE INDEX member_invites_project_idx ON auth.member_invites (project_id, created_at DESC);
CREATE INDEX member_invites_live_email_idx ON auth.member_invites (email)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;
CREATE INDEX member_invites_accepted_by_idx ON auth.member_invites (accepted_by)
    WHERE accepted_by IS NOT NULL;
CREATE INDEX member_invites_shard_key_idx ON auth.member_invites (shard_key);

ALTER TABLE auth.member_invites ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.member_invites FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON auth.member_invites
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

-- No policy for the invitee: a secret link names them, not a GUC, so the accept runs in the lane.
CREATE POLICY maintenance_access ON auth.member_invites
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, UPDATE, DELETE ON auth.member_invites TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 9. SESSIONS
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.sessions (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      TEXT        NOT NULL REFERENCES auth.identities (user_id) ON DELETE CASCADE,
    provider_sid TEXT,
    user_agent   TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at   TIMESTAMPTZ,
    shard_key    UUID        NOT NULL DEFAULT gen_random_uuid()
) WITH (fillfactor = 90);

-- Not partial: erasing a person cascades here on user_id alone.
CREATE INDEX sessions_user_idx ON auth.sessions (user_id, created_at DESC);
CREATE UNIQUE INDEX sessions_provider_sid_key ON auth.sessions (provider_sid)
    WHERE provider_sid IS NOT NULL;
CREATE INDEX sessions_revoked_idx ON auth.sessions (revoked_at)
    WHERE revoked_at IS NOT NULL;
CREATE INDEX sessions_shard_key_idx ON auth.sessions (shard_key);

ALTER TABLE auth.sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.sessions FORCE ROW LEVEL SECURITY;

CREATE POLICY person_isolation ON auth.sessions
    USING      (user_id = current_setting('app.user_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true));

CREATE POLICY maintenance_access ON auth.sessions
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, UPDATE, DELETE ON auth.sessions TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 10. SIGN-IN
-- The accounts auth holds itself (a person's password), the link to an
-- external provider's subject for a person who signs in there, and the
-- grants the issuer mints for every session whichever way it began.
-- ══════════════════════════════════════════════════════════════════════════════
-- The password, and an address change it has mailed a code for. `email` is
-- the copy uniqueness is enforced on: `auth.identities.email` is not unique,
-- because an external provider owns its addresses, and this one is. It is
-- kept in step with the identity's by the one lane that moves an address.
CREATE TABLE auth.credentials (
    user_id                  TEXT        PRIMARY KEY REFERENCES auth.identities (user_id) ON DELETE CASCADE,
    email                    TEXT        NOT NULL UNIQUE,
    password_hash            TEXT        NOT NULL,
    failed_attempts          INTEGER     NOT NULL DEFAULT 0,
    locked_until             TIMESTAMPTZ,
    pending_email            TEXT,
    pending_email_code_hash  TEXT,
    pending_email_expires_at TIMESTAMPTZ,
    pending_email_attempts   INTEGER     NOT NULL DEFAULT 0,
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key                UUID        NOT NULL DEFAULT gen_random_uuid(),
    CONSTRAINT credentials_email_lowercase_check CHECK (email = lower(email)),
    CONSTRAINT credentials_pending_email_lowercase_check
        CHECK (pending_email IS NULL OR pending_email = lower(pending_email)),
    CONSTRAINT credentials_pending_email_shape_check CHECK (
        (pending_email IS NULL) = (pending_email_code_hash IS NULL)
        AND (pending_email IS NULL) = (pending_email_expires_at IS NULL)
    )
);

CREATE INDEX credentials_shard_key_idx ON auth.credentials (shard_key);

-- No maintenance policy: a sign-in finds the person by address through
-- auth.identities, and reads the hash as them. Nothing cross-tenant reads a
-- password hash.
ALTER TABLE auth.credentials ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.credentials FORCE ROW LEVEL SECURITY;

CREATE POLICY person_isolation ON auth.credentials
    USING      (user_id = current_setting('app.user_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true));

-- Which person here an external provider's subject is. The provider's own
-- name for them (`sub`) is the one thing a provider promises to keep stable,
-- so it is the key; the address is what the provider asserts and may change.
-- Read by the exchange lane, which knows no person yet; written by the person
-- the sign-in resolved to.
CREATE TABLE auth.external_identities (
    user_id    TEXT        NOT NULL REFERENCES auth.identities (user_id) ON DELETE CASCADE,
    provider   TEXT        NOT NULL,
    subject    TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key  UUID        NOT NULL DEFAULT gen_random_uuid(),
    PRIMARY KEY (provider, subject)
);

CREATE UNIQUE INDEX external_identities_person_key ON auth.external_identities (user_id, provider);
CREATE INDEX external_identities_shard_key_idx ON auth.external_identities (shard_key);

ALTER TABLE auth.external_identities ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.external_identities FORCE ROW LEVEL SECURITY;

CREATE POLICY person_isolation ON auth.external_identities
    USING      (user_id = current_setting('app.user_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true));

CREATE POLICY maintenance_access ON auth.external_identities
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT ON auth.external_identities TO auth_maintenance;

-- The bearer the console relays on every person request: an opaque secret,
-- stored as its hash, good for fifteen minutes. Every grant mints one beside
-- the refresh token, so a session holds several while a refresh overlaps the
-- one before it; a sign-out deletes every one of its session (`sid`). There
-- is nothing to sign and so no key: the person lane reads this row, and the
-- session's, on every request anyway.
CREATE TABLE auth.access_tokens (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    TEXT        NOT NULL REFERENCES auth.identities (user_id) ON DELETE CASCADE,
    sid        TEXT        NOT NULL,
    token_hash TEXT        NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key  UUID        NOT NULL DEFAULT gen_random_uuid()
) WITH (fillfactor = 90);

CREATE INDEX access_tokens_sid_idx       ON auth.access_tokens (sid);
CREATE INDEX access_tokens_user_idx      ON auth.access_tokens (user_id);
CREATE INDEX access_tokens_reap_idx      ON auth.access_tokens (expires_at);
CREATE INDEX access_tokens_shard_key_idx ON auth.access_tokens (shard_key);

ALTER TABLE auth.access_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.access_tokens FORCE ROW LEVEL SECURITY;

CREATE POLICY person_isolation ON auth.access_tokens
    USING      (user_id = current_setting('app.user_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true));

-- Every person request resolves its bearer here before it knows the person,
-- and the refresh and logout lanes carry a token or a `sid` and no person.
CREATE POLICY maintenance_access ON auth.access_tokens
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, INSERT, DELETE ON auth.access_tokens TO auth_maintenance;

-- A refresh grant, one row per token: a refresh spends the row and mints the
-- next for the same session (`sid`). A spent row is kept until the sweep so a
-- second presentation of it, which is a stolen token, ends the whole session.
CREATE TABLE auth.refresh_tokens (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    TEXT        NOT NULL REFERENCES auth.identities (user_id) ON DELETE CASCADE,
    sid        TEXT        NOT NULL,
    token_hash TEXT        NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at    TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key  UUID        NOT NULL DEFAULT gen_random_uuid()
) WITH (fillfactor = 90);

CREATE INDEX refresh_tokens_sid_idx       ON auth.refresh_tokens (sid);
CREATE INDEX refresh_tokens_user_idx      ON auth.refresh_tokens (user_id);
CREATE INDEX refresh_tokens_reap_idx      ON auth.refresh_tokens (expires_at);
CREATE INDEX refresh_tokens_shard_key_idx ON auth.refresh_tokens (shard_key);

ALTER TABLE auth.refresh_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.refresh_tokens FORCE ROW LEVEL SECURITY;

CREATE POLICY person_isolation ON auth.refresh_tokens
    USING      (user_id = current_setting('app.user_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true));

-- The refresh and logout lanes carry a token or a `sid` and no person.
CREATE POLICY maintenance_access ON auth.refresh_tokens
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, INSERT, UPDATE, DELETE ON auth.refresh_tokens TO auth_maintenance;

-- The one-time code a sign-in hands the console, which the console's callback
-- spends within seconds for the session's tokens. Spent by deletion.
CREATE TABLE auth.authorization_codes (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    TEXT        NOT NULL REFERENCES auth.identities (user_id) ON DELETE CASCADE,
    sid        TEXT        NOT NULL,
    code_hash  TEXT        NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key  UUID        NOT NULL DEFAULT gen_random_uuid()
);

CREATE INDEX authorization_codes_reap_idx      ON auth.authorization_codes (expires_at);
CREATE INDEX authorization_codes_shard_key_idx ON auth.authorization_codes (shard_key);

ALTER TABLE auth.authorization_codes ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.authorization_codes FORCE ROW LEVEL SECURITY;

CREATE POLICY person_isolation ON auth.authorization_codes
    USING      (user_id = current_setting('app.user_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true));

-- The exchange lane carries the code and no person; the person writes the row.
CREATE POLICY maintenance_access ON auth.authorization_codes
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, DELETE ON auth.authorization_codes TO auth_maintenance;

-- A device authorization (RFC 8628): the CLI's sign-in, waiting for a person
-- to approve its code in the console. Nobody's row until then, so it is
-- keyed on no person and every lane on it is the maintenance lane.
CREATE TABLE auth.device_codes (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    device_code_hash TEXT        NOT NULL UNIQUE,
    user_code        TEXT        NOT NULL,
    status           TEXT        NOT NULL DEFAULT 'pending',
    approved_by      TEXT        REFERENCES auth.identities (user_id) ON DELETE CASCADE,
    sid              TEXT,
    interval_secs    INTEGER     NOT NULL,
    last_polled_at   TIMESTAMPTZ,
    expires_at       TIMESTAMPTZ NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key        UUID        NOT NULL DEFAULT gen_random_uuid(),
    CONSTRAINT device_codes_status_check CHECK (status IN ('pending', 'approved', 'denied')),
    CONSTRAINT device_codes_approved_shape_check
        CHECK ((status = 'approved') = (approved_by IS NOT NULL AND sid IS NOT NULL))
);

CREATE UNIQUE INDEX device_codes_live_user_code_key ON auth.device_codes (user_code)
    WHERE status = 'pending';
CREATE INDEX device_codes_reap_idx      ON auth.device_codes (expires_at);
CREATE INDEX device_codes_shard_key_idx ON auth.device_codes (shard_key);

ALTER TABLE auth.device_codes ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.device_codes FORCE ROW LEVEL SECURITY;

CREATE POLICY maintenance_access ON auth.device_codes
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, INSERT, UPDATE, DELETE ON auth.device_codes TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 11. API TOKENS
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.api_tokens (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id TEXT        NOT NULL REFERENCES auth.organizations (external_id) ON DELETE CASCADE,
    project_id      TEXT        NOT NULL,
    name            TEXT        NOT NULL,
    description     TEXT,
    token_hash      TEXT        NOT NULL UNIQUE,
    created_by      TEXT        NOT NULL,
    expires_at      TIMESTAMPTZ,
    last_used_at    TIMESTAMPTZ,
    revoked_at      TIMESTAMPTZ,
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The pair, not each alone: a token naming another organization's project would act for this one.
    -- ON UPDATE CASCADE is what carries the keys with a project handed to
    -- another organization: the one UPDATE of `projects.organization_id`
    -- re-keys them, and nothing else may.
    CONSTRAINT api_tokens_project_fkey FOREIGN KEY (organization_id, project_id)
        REFERENCES auth.projects (organization_id, external_id)
        ON UPDATE CASCADE ON DELETE CASCADE
) WITH (fillfactor = 90);

CREATE INDEX api_tokens_project_id_idx ON auth.api_tokens (project_id);
CREATE INDEX api_tokens_organization_id_idx ON auth.api_tokens (organization_id);
CREATE INDEX api_tokens_shard_key_idx ON auth.api_tokens (shard_key);

CREATE INDEX api_tokens_reap_idx ON auth.api_tokens (revoked_at)
    WHERE revoked_at IS NOT NULL;
CREATE INDEX api_tokens_expiry_idx ON auth.api_tokens (expires_at)
    WHERE expires_at IS NOT NULL;

ALTER TABLE auth.api_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.api_tokens FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON auth.api_tokens
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

CREATE POLICY maintenance_access ON auth.api_tokens
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT, INSERT, UPDATE, DELETE ON auth.api_tokens TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 12. CONFIRMATION CODES
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.confirmation_codes (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     TEXT        NOT NULL REFERENCES auth.identities (user_id) ON DELETE CASCADE,
    purpose     TEXT        NOT NULL,
    subject     TEXT        REFERENCES auth.organizations (external_id) ON DELETE CASCADE,
    code_hash   TEXT        NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    -- The brute-force guard: a caller holding the service secret skips the
    -- console's rate limit, so wrong guesses are counted on the row itself.
    attempts    INTEGER     NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key   UUID        NOT NULL DEFAULT gen_random_uuid(),
    CONSTRAINT confirmation_codes_purpose_check
        CHECK (purpose IN ('organization_deletion', 'account_deletion', 'email_change',
                           'email_verification', 'password_reset')),
    CONSTRAINT confirmation_codes_subject_matches_purpose_check
        CHECK ((purpose = 'organization_deletion') = (subject IS NOT NULL))
);

CREATE INDEX confirmation_codes_user_id_idx ON auth.confirmation_codes (user_id);
CREATE INDEX confirmation_codes_subject_idx ON auth.confirmation_codes (subject)
    WHERE subject IS NOT NULL;
-- The guess counter is per row, so a second live code would be a second budget;
-- it is also the live lookup's index, since every read names the person and purpose.
CREATE UNIQUE INDEX confirmation_codes_one_live_key ON auth.confirmation_codes (user_id, purpose)
    WHERE consumed_at IS NULL;
CREATE INDEX confirmation_codes_shard_key_idx ON auth.confirmation_codes (shard_key);

ALTER TABLE auth.confirmation_codes ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.confirmation_codes FORCE ROW LEVEL SECURITY;

CREATE POLICY person_isolation ON auth.confirmation_codes
    USING      (user_id = current_setting('app.user_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true));


-- ══════════════════════════════════════════════════════════════════════════════
-- 13. FEATURE FLAGS
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.feature_flags (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    key        TEXT        NOT NULL,
    enabled    BOOLEAN     NOT NULL,
    note       TEXT        NOT NULL,
    actor      TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key  UUID        NOT NULL DEFAULT gen_random_uuid(),
    CONSTRAINT feature_flags_key_shape_check CHECK (key ~ '^[a-z][a-z_]{1,62}$'),
    CONSTRAINT feature_flags_note_len_check  CHECK (char_length(note) BETWEEN 1 AND 500)
);

CREATE INDEX feature_flags_latest_idx ON auth.feature_flags (key, created_at DESC, id DESC);

GRANT SELECT ON auth.feature_flags TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 14. ORGANIZATION FLAGS
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE auth.organization_flags (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id TEXT        NOT NULL REFERENCES auth.organizations (external_id) ON DELETE CASCADE,
    key             TEXT        NOT NULL,
    enabled         BOOLEAN     NOT NULL,
    note            TEXT        NOT NULL,
    actor           TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    CONSTRAINT organization_flags_key_shape_check CHECK (key ~ '^[a-z][a-z_]{1,62}$'),
    CONSTRAINT organization_flags_note_len_check  CHECK (char_length(note) BETWEEN 1 AND 500)
);

CREATE INDEX organization_flags_latest_idx ON auth.organization_flags (organization_id, key, created_at DESC, id DESC);

ALTER TABLE auth.organization_flags ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth.organization_flags FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON auth.organization_flags
    USING      (organization_id = current_setting('app.organization_id', true))
    WITH CHECK (organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON auth.organization_flags
    TO auth_maintenance
    USING      (current_user = 'auth_maintenance')
    WITH CHECK (current_user = 'auth_maintenance');

GRANT SELECT ON auth.organization_flags TO auth_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 15. CROSS-ENTITY POLICIES
-- Each reads a table created after the one it guards, so it must come last.
-- ══════════════════════════════════════════════════════════════════════════════
CREATE POLICY organization_member_read ON auth.identities
    FOR SELECT
    USING (EXISTS (
        SELECT 1 FROM auth.organization_members m
         WHERE m.user_id = auth.identities.user_id
           AND m.organization_id = current_setting('app.organization_id', true)
    ));

CREATE POLICY member_read ON auth.organizations
    FOR SELECT
    USING (EXISTS (
        SELECT 1 FROM auth.organization_members m
         WHERE m.organization_id = auth.organizations.external_id
           AND m.user_id = current_setting('app.user_id', true)
    ));

CREATE POLICY project_read ON auth.organizations
    FOR SELECT
    USING (EXISTS (
        SELECT 1 FROM auth.projects p
         WHERE p.organization_id = auth.organizations.external_id
           AND p.external_id = current_setting('app.project_id', true)
    ));

CREATE POLICY project_member_read ON auth.projects
    FOR SELECT
    USING (EXISTS (
        SELECT 1 FROM auth.project_members m
         WHERE m.project_id = auth.projects.external_id
           AND m.user_id = current_setting('app.user_id', true)
    ));
