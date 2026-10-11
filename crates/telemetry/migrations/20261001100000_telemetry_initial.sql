CREATE SCHEMA IF NOT EXISTS telemetry;

-- What telemetry keeps in Postgres: what must be transactional or private.
-- The spans themselves are ClickHouse's (`../clickhouse/`), which holds no
-- content field in any mode.

-- ══════════════════════════════════════════════════════════════════════════════
-- ROLE BOOTSTRAP & SCHEMA GRANTS
-- ══════════════════════════════════════════════════════════════════════════════
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'telemetry_maintenance') THEN
        CREATE ROLE telemetry_maintenance NOLOGIN;
    END IF;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN unique_violation THEN NULL;
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'telemetry_maintenance not created here (no CREATEROLE) — role_hardening.sql owns it in prod';
END $$;

DO $$
BEGIN
    GRANT telemetry_maintenance TO telemetry WITH INHERIT FALSE, SET TRUE;
EXCEPTION
    WHEN undefined_object THEN NULL;
    WHEN insufficient_privilege THEN NULL;
END $$;

GRANT USAGE ON SCHEMA telemetry TO telemetry_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 1. PROJECT SETTINGS
-- ══════════════════════════════════════════════════════════════════════════════
-- A project's own choices about what it keeps. A project with no row reads as
-- the defaults, so content stays off until an owner turns it on, and nothing
-- has to be written when a project is made.
CREATE TABLE telemetry.project_settings (
    project_id              TEXT        PRIMARY KEY,
    organization_id         TEXT        NOT NULL,
    content_mode            TEXT        NOT NULL DEFAULT 'off',
    -- Whether a span's user id is stored as its HMAC-SHA-256 under the
    -- project's own key, or as sent where the project keeps it plain.
    user_id_hashed          BOOLEAN     NOT NULL DEFAULT true,
    -- That key, sealed under a data key of the row's own, which TELEMETRY_KEK
    -- wraps, as a connector's grant is sealed under CONNECTOR_KEK. Made by the
    -- project's first export, so a row the content switch made has none yet.
    hashing_key_ciphertext  BYTEA,
    hashing_key_nonce       BYTEA,
    hashing_key_wrapped_dek BYTEA,
    hashing_key_version     SMALLINT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT project_settings_content_mode_check CHECK (content_mode IN ('off', 'on', 'sealed')),
    -- The key is there whole or not at all.
    CONSTRAINT project_settings_hashing_key_check CHECK (
        (hashing_key_ciphertext IS NULL) = (hashing_key_nonce IS NULL)
        AND (hashing_key_ciphertext IS NULL) = (hashing_key_wrapped_dek IS NULL)
        AND (hashing_key_ciphertext IS NULL) = (hashing_key_version IS NULL))
);

CREATE INDEX project_settings_organization_idx ON telemetry.project_settings (organization_id);

ALTER TABLE telemetry.project_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.project_settings FORCE ROW LEVEL SECURITY;

-- A write names the organization too, which `organization_read` trusts, so
-- the scope that writes binds both: a project's scope alone cannot file its
-- settings under another organization.
CREATE POLICY tenant_isolation ON telemetry.project_settings
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true)
                AND organization_id = current_setting('app.organization_id', true));

-- An organization reads its projects' settings, and writes none: each is
-- changed on its own project.
CREATE POLICY organization_read ON telemetry.project_settings
    FOR SELECT
    USING (organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON telemetry.project_settings
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The purges delete an organization's or a deleted project's rows.
GRANT SELECT, DELETE ON telemetry.project_settings TO telemetry_maintenance;
-- A transfer moves a row to the organization that now holds its project.
GRANT UPDATE (organization_id) ON telemetry.project_settings TO telemetry_maintenance;
