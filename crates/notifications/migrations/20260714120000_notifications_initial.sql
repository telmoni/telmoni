CREATE SCHEMA IF NOT EXISTS notifications;

-- ══════════════════════════════════════════════════════════════════════════════
-- ROLE BOOTSTRAP & SCHEMA GRANTS
-- ══════════════════════════════════════════════════════════════════════════════
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'notifications_maintenance') THEN
        CREATE ROLE notifications_maintenance NOLOGIN;
    END IF;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN unique_violation THEN NULL;
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'notifications_maintenance not created here (no CREATEROLE) — role_hardening.sql owns it in prod';
END $$;

DO $$
BEGIN
    GRANT notifications_maintenance TO notifications WITH INHERIT FALSE, SET TRUE;
EXCEPTION
    WHEN undefined_object THEN NULL;
    WHEN insufficient_privilege THEN NULL;
END $$;

GRANT USAGE ON SCHEMA notifications TO notifications_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 1. FEED
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE notifications.feed (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id      TEXT,
    organization_id TEXT        NOT NULL,
    subject_user_id TEXT,
    kind            TEXT        NOT NULL,
    title           TEXT        NOT NULL,
    body            TEXT        NOT NULL,
    metadata        JSONB       NOT NULL DEFAULT '{}'::jsonb,
    read_at         TIMESTAMPTZ,
    dedup_key       TEXT,
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT feed_kind_check CHECK (kind IN (
        'organization_alert', 'member_added',
        'connector_connected', 'connector_disconnected'
    ))
);

CREATE INDEX feed_organization_idx    ON notifications.feed (organization_id, project_id);
CREATE INDEX feed_project_created_idx ON notifications.feed (project_id, created_at DESC, id DESC);
CREATE INDEX feed_retention_idx       ON notifications.feed (created_at);
CREATE INDEX feed_subject_user_idx    ON notifications.feed (subject_user_id)
    WHERE subject_user_id IS NOT NULL;
-- NULLS NOT DISTINCT, or an organization-level row (no project) would never collide with its replay.
CREATE UNIQUE INDEX feed_dedup_key ON notifications.feed (organization_id, dedup_key, project_id) NULLS NOT DISTINCT
    WHERE dedup_key IS NOT NULL;

ALTER TABLE notifications.feed ENABLE ROW LEVEL SECURITY;
ALTER TABLE notifications.feed FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON notifications.feed
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

-- `project_id IS NULL` is load-bearing: without it an organization scope reaches every project's rows.
CREATE POLICY organization_level ON notifications.feed
    USING      (project_id IS NULL AND organization_id = current_setting('app.organization_id', true))
    WITH CHECK (project_id IS NULL AND organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON notifications.feed
    TO notifications_maintenance
    USING      (current_user = 'notifications_maintenance')
    WITH CHECK (current_user = 'notifications_maintenance');

-- INSERT for the disconnect notice a retirement writes, in the same transaction as the retirement.
GRANT SELECT, INSERT, UPDATE, DELETE ON notifications.feed TO notifications_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 2. CONNECTIONS
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE notifications.connections (
    id                       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id               TEXT        NOT NULL,
    organization_id          TEXT        NOT NULL,
    provider                 TEXT        NOT NULL,
    external_workspace_id    TEXT        NOT NULL,
    external_workspace_name  TEXT,
    channel_id               TEXT        NOT NULL,
    channel_name             TEXT        NOT NULL,
    target_ciphertext        BYTEA       NOT NULL,
    target_nonce             BYTEA       NOT NULL,
    key_version              SMALLINT    NOT NULL,
    wrapped_dek              BYTEA       NOT NULL,
    token_ciphertext         BYTEA,
    token_nonce              BYTEA,
    -- A rotated webhook secret that still signs until its expiry, so a
    -- receiver can move to the new one without dropping a delivery.
    prior_token_ciphertext   BYTEA,
    prior_token_nonce        BYTEA,
    prior_token_expires_at   TIMESTAMPTZ,
    refresh_token_ciphertext BYTEA,
    expires_at               TIMESTAMPTZ,
    scopes                   TEXT        NOT NULL,
    -- The notice kinds a webhook receives; NULL is every kind, including
    -- kinds added after it was connected.
    event_kinds              TEXT[],
    installed_by             TEXT        NOT NULL,
    status                   TEXT        NOT NULL DEFAULT 'active',
    last_error               TEXT,
    last_delivery_at         TIMESTAMPTZ,
    consecutive_failures     INTEGER     NOT NULL DEFAULT 0,
    shard_key                UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at               TIMESTAMPTZ,
    CONSTRAINT connections_provider_check CHECK (provider IN ('slack', 'discord', 'webhook')),
    CONSTRAINT connections_status_check CHECK (status IN ('active', 'errored', 'revoked')),
    CONSTRAINT connections_token_pair_check CHECK ((token_ciphertext IS NULL) = (token_nonce IS NULL)),
    CONSTRAINT connections_prior_token_check CHECK (
        (prior_token_ciphertext IS NULL) = (prior_token_nonce IS NULL)
        AND (prior_token_ciphertext IS NULL) = (prior_token_expires_at IS NULL)
    ),
    CONSTRAINT connections_event_kinds_check CHECK (
        event_kinds IS NULL OR (cardinality(event_kinds) > 0 AND event_kinds <@ ARRAY[
            'organization_alert', 'member_added',
            'connector_connected', 'connector_disconnected'
        ]::text[])
    ),
    CONSTRAINT connections_event_kinds_webhook_check CHECK (event_kinds IS NULL OR provider = 'webhook'),
    CONSTRAINT connections_tenant_key UNIQUE (id, project_id, organization_id)
) WITH (fillfactor = 90);

CREATE UNIQUE INDEX connections_project_channel_key
    ON notifications.connections (project_id, provider, external_workspace_id, channel_id);
CREATE INDEX connections_workspace_idx    ON notifications.connections (provider, external_workspace_id);
CREATE INDEX connections_organization_idx ON notifications.connections (organization_id);
-- The sweep's predicate for rotated secrets past their overlap; almost every
-- row is outside it, so the index holds only rows mid-rotation.
CREATE INDEX connections_prior_token_idx ON notifications.connections (prior_token_expires_at)
    WHERE prior_token_expires_at IS NOT NULL;

ALTER TABLE notifications.connections ENABLE ROW LEVEL SECURITY;
ALTER TABLE notifications.connections FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON notifications.connections
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

CREATE POLICY organization_read ON notifications.connections
    FOR SELECT
    USING (organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON notifications.connections
    TO notifications_maintenance
    USING      (current_user = 'notifications_maintenance')
    WITH CHECK (current_user = 'notifications_maintenance');

GRANT SELECT, UPDATE, DELETE ON notifications.connections TO notifications_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 3. DELIVERIES
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE notifications.deliveries (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    connection_id   UUID        NOT NULL,
    project_id      TEXT        NOT NULL,
    organization_id TEXT        NOT NULL,
    kind            TEXT        NOT NULL,
    subject         TEXT        NOT NULL,
    body            TEXT        NOT NULL,
    subject_user_id TEXT,
    status          TEXT        NOT NULL DEFAULT 'pending',
    attempts        INTEGER     NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until     TIMESTAMPTZ,
    last_error      TEXT,
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT deliveries_status_check CHECK (status IN ('pending', 'delivered', 'failed')),
    CONSTRAINT deliveries_kind_check CHECK (kind IN (
        'organization_alert', 'member_added',
        'connector_connected', 'connector_disconnected'
    )),
    -- The whole tenant, not the id alone: the enqueue policies check only the
    -- bound key, so a delivery could otherwise carry one tenant's text to another's channel.
    CONSTRAINT deliveries_connection_fkey FOREIGN KEY (connection_id, project_id, organization_id)
        REFERENCES notifications.connections (id, project_id, organization_id) ON DELETE CASCADE,
    CONSTRAINT deliveries_tenant_key UNIQUE (id, project_id, organization_id)
) WITH (fillfactor = 90);

-- `id` is v7, so this also serves the delivery log's newest-first page.
CREATE INDEX deliveries_connection_idx ON notifications.deliveries (connection_id, id);
CREATE INDEX deliveries_lease_idx      ON notifications.deliveries (next_attempt_at)
    WHERE status = 'pending';
-- The predicate is spelled exactly as `db::sweep_connector_rows` spells it, or the planner will not use it.
CREATE INDEX deliveries_terminal_idx ON notifications.deliveries (updated_at)
    WHERE status IN ('delivered', 'failed');
CREATE INDEX deliveries_subject_user_idx ON notifications.deliveries (subject_user_id)
    WHERE subject_user_id IS NOT NULL;

ALTER TABLE notifications.deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE notifications.deliveries FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON notifications.deliveries
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

CREATE POLICY organization_read ON notifications.deliveries
    FOR SELECT
    USING (organization_id = current_setting('app.organization_id', true));

CREATE POLICY organization_enqueue ON notifications.deliveries
    FOR INSERT
    WITH CHECK (organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON notifications.deliveries
    TO notifications_maintenance
    USING      (current_user = 'notifications_maintenance')
    WITH CHECK (current_user = 'notifications_maintenance');

GRANT SELECT, INSERT, UPDATE, DELETE ON notifications.deliveries TO notifications_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 3a. DELIVERY ATTEMPTS
-- ══════════════════════════════════════════════════════════════════════════════
-- One row per send that reached the far end or failed on the way, for the
-- delivery log. A handed-back attempt (the key was unreachable) sent nothing
-- and is not one. Goes with its delivery.
CREATE TABLE notifications.delivery_attempts (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    delivery_id     UUID        NOT NULL,
    project_id      TEXT        NOT NULL,
    organization_id TEXT        NOT NULL,
    trigger         TEXT        NOT NULL,
    outcome         TEXT        NOT NULL,
    status_code     INTEGER,
    duration_ms     INTEGER     NOT NULL,
    error           TEXT,
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT delivery_attempts_trigger_check CHECK (trigger IN ('scheduled', 'manual')),
    CONSTRAINT delivery_attempts_outcome_check CHECK (outcome IN ('delivered', 'failed')),
    CONSTRAINT delivery_attempts_status_code_check CHECK (status_code BETWEEN 100 AND 599),
    CONSTRAINT delivery_attempts_duration_check CHECK (duration_ms >= 0),
    CONSTRAINT delivery_attempts_error_check CHECK ((outcome = 'delivered') = (error IS NULL)),
    CONSTRAINT delivery_attempts_delivery_fkey FOREIGN KEY (delivery_id, project_id, organization_id)
        REFERENCES notifications.deliveries (id, project_id, organization_id) ON DELETE CASCADE
);

CREATE INDEX delivery_attempts_delivery_idx ON notifications.delivery_attempts (delivery_id, id);

ALTER TABLE notifications.delivery_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE notifications.delivery_attempts FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON notifications.delivery_attempts
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

CREATE POLICY maintenance_access ON notifications.delivery_attempts
    TO notifications_maintenance
    USING      (current_user = 'notifications_maintenance')
    WITH CHECK (current_user = 'notifications_maintenance');

-- INSERT for the loop, which writes the log; nothing in the lane reads it
-- back, since the log is read project by project and rows go with their
-- delivery. The one column-wide exception is erasure: `redact_person` replaces
-- the `error` of a delivery that named the person, because a receiver may
-- echo the notice back in its refusal, and finds those rows by `delivery_id`
-- and `outcome` — never by reading the error text it is replacing.
GRANT INSERT ON notifications.delivery_attempts TO notifications_maintenance;
GRANT SELECT (delivery_id, outcome), UPDATE (error) ON notifications.delivery_attempts
    TO notifications_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 4. OAUTH STATES
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE notifications.oauth_states (
    state_hash      TEXT        PRIMARY KEY,
    project_id      TEXT        NOT NULL,
    organization_id TEXT        NOT NULL,
    user_id         TEXT        NOT NULL,
    provider        TEXT        NOT NULL,
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL,
    consumed_at     TIMESTAMPTZ,
    CONSTRAINT oauth_states_provider_check CHECK (provider IN ('slack', 'discord'))
);

CREATE INDEX oauth_states_organization_idx ON notifications.oauth_states (organization_id);
CREATE INDEX oauth_states_expiry_idx       ON notifications.oauth_states (expires_at);

ALTER TABLE notifications.oauth_states ENABLE ROW LEVEL SECURITY;
ALTER TABLE notifications.oauth_states FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON notifications.oauth_states
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

CREATE POLICY maintenance_access ON notifications.oauth_states
    TO notifications_maintenance
    USING      (current_user = 'notifications_maintenance')
    WITH CHECK (current_user = 'notifications_maintenance');

GRANT SELECT, DELETE ON notifications.oauth_states TO notifications_maintenance;
