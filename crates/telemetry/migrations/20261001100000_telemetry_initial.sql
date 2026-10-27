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
    -- The saved layout its sessions and traces open in, which stands until
    -- it is deleted and the console's own default holds again (the key,
    -- below the views); none for the default.
    session_view_id         UUID,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT project_settings_content_mode_check CHECK (content_mode IN ('off', 'on', 'sealed')),
    -- The key is there whole or not at all.
    CONSTRAINT project_settings_hashing_key_check CHECK (
        (hashing_key_ciphertext IS NULL) = (hashing_key_nonce IS NULL)
        AND (hashing_key_ciphertext IS NULL) = (hashing_key_wrapped_dek IS NULL)
        AND (hashing_key_ciphertext IS NULL) = (hashing_key_version IS NULL))
);

-- The rule for every table here: one the organization's purge deletes by
-- `organization_id` carries an index on it; one a cascade or a `SET NULL`
-- takes carries an index on the key it follows.
CREATE INDEX project_settings_organization_idx ON telemetry.project_settings (organization_id);
-- The key a deleted layout's SET NULL follows.
CREATE INDEX project_settings_session_view_idx ON telemetry.project_settings (session_view_id)
    WHERE session_view_id IS NOT NULL;

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


-- ══════════════════════════════════════════════════════════════════════════════
-- 2. SAVED VIEWS
-- ══════════════════════════════════════════════════════════════════════════════
-- A project's trace list as somebody left it, under a name: its filters, sort
-- and window as the list's query takes them, and the columns it showed
-- (`kind` = 'list'); or a layout of a session's page and a trace's, its
-- options and the columns a trace shows (`kind` = 'layout'). Every role on
-- the project applies one; its owners and admins save them. Filed under the
-- project alone: the purges find a project by its settings row, which the
-- first view saved makes where the project has none.
CREATE TABLE telemetry.trace_views (
    id          UUID        PRIMARY KEY,
    project_id  TEXT        NOT NULL,
    kind        TEXT        NOT NULL DEFAULT 'list',
    name        TEXT        NOT NULL,
    -- A list's query, each filter's values a list (`{"agent": ["…"],
    -- "sort": "tokens", "days": 30}`), or a layout's options (`{"depth":
    -- "calls", "open": "failed", …}`); the lane checks each as its page
    -- would read it.
    query       JSONB       NOT NULL,
    -- The columns shown, by `TraceColumn`'s words, in the list's order.
    columns     TEXT[]      NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT trace_views_kind_check CHECK (kind IN ('list', 'layout')),
    CONSTRAINT trace_views_name_check CHECK (char_length(name) BETWEEN 1 AND 60),
    CONSTRAINT trace_views_query_check CHECK (jsonb_typeof(query) = 'object')
);

-- One name per view of a kind in a project, whatever its case: a menu that
-- offered two "Errors" would make a person guess.
CREATE UNIQUE INDEX trace_views_project_name_idx
    ON telemetry.trace_views (project_id, kind, lower(name));

ALTER TABLE telemetry.trace_views ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.trace_views FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON telemetry.trace_views
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

CREATE POLICY maintenance_access ON telemetry.trace_views
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The purges delete a deleted project's views, and those of an organization's
-- projects. A transfer moves nothing: a view names no organization.
GRANT SELECT, DELETE ON telemetry.trace_views TO telemetry_maintenance;

-- A project's default layout, where it is a saved one: deleted, the project
-- opens in its preset again. The action runs as the table's owner, so no
-- role needs a grant on the column for it.
ALTER TABLE telemetry.project_settings
    ADD CONSTRAINT project_settings_session_view_fkey
    FOREIGN KEY (session_view_id) REFERENCES telemetry.trace_views (id) ON DELETE SET NULL;


-- ══════════════════════════════════════════════════════════════════════════════
-- 3. ALERTS
-- ══════════════════════════════════════════════════════════════════════════════
-- A project's alerts: each a measure of its spans of one kind, folded over
-- the last hour, day or week, against an alert threshold and an optional
-- warning one. Filed under the organization too, whose plan holds how many
-- it may have across its projects, paused ones counted (`paused` says by
-- whom: a person, or the plan, which lifts its own). The last evaluation is
-- the loop's to write, under a lease (`lease_id` until `leased_until`) so
-- replicas share the loop with no leader: its value, the severity it set and
-- since when, and when that severity was last told (`told_at`), which
-- re-notification counts from.
CREATE TABLE telemetry.rules (
    id                UUID             PRIMARY KEY,
    project_id        TEXT             NOT NULL,
    organization_id   TEXT             NOT NULL,
    name              TEXT             NOT NULL,
    kind              TEXT             NOT NULL,
    measure           TEXT             NOT NULL,
    aggregation       TEXT             NOT NULL,
    filters           JSONB            NOT NULL DEFAULT '{}',
    lookback          TEXT             NOT NULL,
    operator          TEXT             NOT NULL,
    alert_threshold   DOUBLE PRECISION NOT NULL,
    warning_threshold DOUBLE PRECISION,
    missing_data      TEXT             NOT NULL DEFAULT 'zero',
    renotify_minutes  INTEGER,
    paused            TEXT,
    paused_at         TIMESTAMPTZ,
    value             DOUBLE PRECISION,
    evaluated_at      TIMESTAMPTZ,
    severity          TEXT             NOT NULL DEFAULT 'unknown',
    severity_since    TIMESTAMPTZ      NOT NULL DEFAULT now(),
    told_at           TIMESTAMPTZ,
    lease_id          UUID,
    leased_until      TIMESTAMPTZ,
    created_at        TIMESTAMPTZ      NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ      NOT NULL DEFAULT now(),
    CONSTRAINT rules_name_check CHECK (char_length(name) BETWEEN 1 AND 60),
    CONSTRAINT rules_kind_check CHECK (kind IN ('run', 'step', 'model_call', 'tool_call')),
    CONSTRAINT rules_measure_check
        CHECK (measure IN ('count', 'failures', 'duration', 'tokens', 'cost')),
    CONSTRAINT rules_aggregation_check
        CHECK (aggregation IN ('count', 'avg', 'sum', 'max', 'p50', 'p90', 'p95', 'p99')),
    CONSTRAINT rules_filters_check CHECK (jsonb_typeof(filters) = 'object'),
    CONSTRAINT rules_lookback_check CHECK (lookback IN ('hour', 'day', 'week')),
    CONSTRAINT rules_operator_check CHECK (operator IN ('gt', 'gte', 'lt', 'lte', 'eq', 'ne')),
    CONSTRAINT rules_missing_data_check
        CHECK (missing_data IN ('zero', 'keep', 'no_data', 'notify_no_data')),
    CONSTRAINT rules_renotify_check CHECK (renotify_minutes BETWEEN 1 AND 10080),
    CONSTRAINT rules_paused_check CHECK (paused IN ('person', 'plan')),
    CONSTRAINT rules_severity_check
        CHECK (severity IN ('unknown', 'ok', 'warning', 'alert', 'no_data', 'paused')),
    CONSTRAINT rules_paused_at_check CHECK ((paused IS NULL) = (paused_at IS NULL)),
    CONSTRAINT rules_lease_check CHECK ((lease_id IS NULL) = (leased_until IS NULL))
);

-- One name per alert in a project, whatever its case.
CREATE UNIQUE INDEX rules_project_name_idx ON telemetry.rules (project_id, lower(name));
-- The organization's purge deletes by it, and its plan counts and orders by it.
CREATE INDEX rules_organization_idx ON telemetry.rules (organization_id, created_at);
-- What the loop leases: running alerts, the longest unevaluated first.
CREATE INDEX rules_due_idx ON telemetry.rules (evaluated_at NULLS FIRST, id) WHERE paused IS NULL;

ALTER TABLE telemetry.rules ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.rules FORCE ROW LEVEL SECURITY;

-- A write names the organization too, which `organization_read` trusts, as
-- a project's settings do.
CREATE POLICY tenant_isolation ON telemetry.rules
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true)
                AND organization_id = current_setting('app.organization_id', true));

-- An organization counts its alerts across its projects, and writes none of
-- another project's.
CREATE POLICY organization_read ON telemetry.rules
    FOR SELECT
    USING (organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON telemetry.rules
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The loop leases and settles alerts and holds them to their plan; the
-- purges delete a project's or an organization's; a transfer moves a
-- project's to the organization that takes it.
GRANT SELECT, DELETE ON telemetry.rules TO telemetry_maintenance;
GRANT UPDATE (organization_id, paused, paused_at, value, evaluated_at, severity, severity_since,
              told_at, lease_id, leased_until)
    ON telemetry.rules TO telemetry_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 4. INCIDENTS
-- ══════════════════════════════════════════════════════════════════════════════
-- Each breach of an alert: opened by the loop when its alert crosses a
-- threshold — or, where the alert asks, once its window has held no span
-- for as long as the window — at the severity it crossed, which follows the
-- alert's while it lasts; acknowledged by a member, which stops its
-- reminders; and resolved at the alert's recovery (`resolved_by` none) or by
-- a member's hand. One that breaches again within an hour of its recovery is
-- reopened (`reopened` counts how often) rather than a new one, so a
-- flapping alert opens one incident an hour at most. An alert holds one open
-- at a time; its incidents go with it. Each keeps where its notices went
-- (`told_to`, the connections they were queued to, the feed apart) and the
-- runs they showed (`runs`, the last 20, oldest first), which link back to it.
CREATE TABLE telemetry.incidents (
    id               UUID             PRIMARY KEY,
    rule_id          UUID             NOT NULL REFERENCES telemetry.rules (id) ON DELETE CASCADE,
    project_id       TEXT             NOT NULL,
    organization_id  TEXT             NOT NULL,
    status           TEXT             NOT NULL DEFAULT 'open',
    severity         TEXT             NOT NULL,
    value            DOUBLE PRECISION,
    opened_at        TIMESTAMPTZ      NOT NULL DEFAULT now(),
    acknowledged_at  TIMESTAMPTZ,
    acknowledged_by  TEXT,
    resolved_at      TIMESTAMPTZ,
    resolved_by      TEXT,
    reopened         INTEGER          NOT NULL DEFAULT 0,
    told_to          UUID[]           NOT NULL DEFAULT '{}',
    runs             UUID[]           NOT NULL DEFAULT '{}',
    updated_at       TIMESTAMPTZ      NOT NULL DEFAULT now(),
    CONSTRAINT incidents_status_check CHECK (status IN ('open', 'acknowledged', 'resolved')),
    CONSTRAINT incidents_severity_check CHECK (severity IN ('warning', 'alert', 'no_data')),
    CONSTRAINT incidents_acknowledged_check
        CHECK ((acknowledged_at IS NULL) = (acknowledged_by IS NULL)),
    CONSTRAINT incidents_resolved_check CHECK ((status = 'resolved') = (resolved_at IS NOT NULL)),
    CONSTRAINT incidents_reopened_check CHECK (reopened >= 0),
    CONSTRAINT incidents_runs_check CHECK (cardinality(runs) <= 20)
);

-- One open incident an alert, acknowledged or not.
CREATE UNIQUE INDEX incidents_one_open_idx ON telemetry.incidents (rule_id)
    WHERE status <> 'resolved';
-- The project's incidents, newest first, and an alert's last resolved one.
CREATE INDEX incidents_project_idx ON telemetry.incidents (project_id, opened_at DESC);
CREATE INDEX incidents_rule_idx ON telemetry.incidents (rule_id, resolved_at DESC);
-- The organization's purge and a transfer find them by it.
CREATE INDEX incidents_organization_idx ON telemetry.incidents (organization_id);

ALTER TABLE telemetry.incidents ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.incidents FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON telemetry.incidents
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

CREATE POLICY maintenance_access ON telemetry.incidents
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The loop opens, reopens, follows and resolves them; a transfer moves a
-- project's; the purges' deletes of alerts take theirs by the cascade. Where
-- their notices went is kept in the project's own scope.
GRANT SELECT ON telemetry.incidents TO telemetry_maintenance;
GRANT INSERT (id, rule_id, project_id, organization_id, severity, value, opened_at, updated_at)
    ON telemetry.incidents TO telemetry_maintenance;
GRANT UPDATE (organization_id, status, severity, value, resolved_at, resolved_by, reopened,
              updated_at)
    ON telemetry.incidents TO telemetry_maintenance;

-- ══════════════════════════════════════════════════════════════════════════════
-- 5. AN ALERT'S CONNECTORS
-- ══════════════════════════════════════════════════════════════════════════════
-- The project's connections an alert's notices go to, each hearing its
-- warnings and alerts or its alerts alone (`level`), in the hours it is set
-- to, or every hour (`hours`: the days, 1 Monday to 7 Sunday, from and to as
-- HH:MM at a fixed offset from UTC, checked by the lane). A connection the
-- project no longer has is ignored where a notice goes; whether one is
-- muted after five failed deliveries in a row is notifications' to keep.
CREATE TABLE telemetry.rule_connectors (
    rule_id       UUID  NOT NULL REFERENCES telemetry.rules (id) ON DELETE CASCADE,
    connection_id UUID  NOT NULL,
    project_id    TEXT  NOT NULL,
    level         TEXT  NOT NULL DEFAULT 'warning',
    hours         JSONB,
    PRIMARY KEY (rule_id, connection_id),
    CONSTRAINT rule_connectors_level_check CHECK (level IN ('warning', 'alert')),
    CONSTRAINT rule_connectors_hours_check CHECK (hours IS NULL OR jsonb_typeof(hours) = 'object')
);

CREATE INDEX rule_connectors_project_idx ON telemetry.rule_connectors (project_id);

ALTER TABLE telemetry.rule_connectors ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.rule_connectors FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON telemetry.rule_connectors
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

CREATE POLICY maintenance_access ON telemetry.rule_connectors
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The loop reads where each alert's notices go; they go with their alert.
GRANT SELECT ON telemetry.rule_connectors TO telemetry_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 6. MAINTENANCE WINDOWS
-- ══════════════════════════════════════════════════════════════════════════════
-- A window an alert, or every alert of a project (`rule_id` none), is muted
-- for: evaluated as ever, its incidents opened and resolved in the feed, and
-- told to no connector from `starts_at` until `ends_at`. Each is kept after it
-- ends as the record of who muted what, and when; one ended early keeps who
-- ended it.
CREATE TABLE telemetry.mutes (
    id              UUID        PRIMARY KEY,
    project_id      TEXT        NOT NULL,
    rule_id         UUID        REFERENCES telemetry.rules (id) ON DELETE CASCADE,
    starts_at       TIMESTAMPTZ NOT NULL,
    ends_at         TIMESTAMPTZ NOT NULL,
    created_by      TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_by        TEXT,
    CONSTRAINT mutes_window_check CHECK (ends_at > starts_at),
    CONSTRAINT mutes_length_check CHECK (ends_at - starts_at <= interval '30 days')
);

-- The project's mutes, newest first, and those in force at an instant.
CREATE INDEX mutes_project_idx ON telemetry.mutes (project_id, ends_at DESC);
CREATE INDEX mutes_rule_idx ON telemetry.mutes (rule_id) WHERE rule_id IS NOT NULL;

ALTER TABLE telemetry.mutes ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.mutes FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON telemetry.mutes
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true));

CREATE POLICY maintenance_access ON telemetry.mutes
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The loop reads which are in force; the purges delete a project's.
GRANT SELECT, DELETE ON telemetry.mutes TO telemetry_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 7. HEARTBEATS
-- ══════════════════════════════════════════════════════════════════════════════
-- The purge sweep, the plan refusals' notices and the alerts' evaluation,
-- each as the last replica
-- to finish a tick of it left it: what the external watchdog reads through
-- `GET /heartbeat`, a loop stale once no replica has beaten it for three of
-- its periods. Names no tenant.
CREATE TABLE telemetry.heartbeats (
    name           TEXT        PRIMARY KEY,
    period_seconds INTEGER     NOT NULL,
    beat_at        TIMESTAMPTZ NOT NULL,
    CONSTRAINT heartbeats_name_check CHECK (name IN ('purges', 'refusals', 'alerts')),
    CONSTRAINT heartbeats_period_check CHECK (period_seconds > 0)
);

ALTER TABLE telemetry.heartbeats ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.heartbeats FORCE ROW LEVEL SECURITY;

CREATE POLICY maintenance_access ON telemetry.heartbeats
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

GRANT SELECT, INSERT, UPDATE ON telemetry.heartbeats TO telemetry_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 8. MODEL PRICES
-- ══════════════════════════════════════════════════════════════════════════════
-- What a call costs that reports no cost of its own: the price of a token, in
-- US dollars, of a provider's model under each name it answers with — input
-- no cache served, output, a cache read and a cache write — from the day it
-- takes effect, none for always. The price file's (`../model_prices.json`),
-- which `migrate` writes over this table on every run, as its owner; the
-- module only reads it (`object_grants.sql`). Names no tenant, so it holds
-- nothing row security would keep from anyone.
CREATE TABLE telemetry.model_prices (
    provider       TEXT             NOT NULL,
    model          TEXT             NOT NULL,
    effective_from DATE,
    input          DOUBLE PRECISION NOT NULL,
    output         DOUBLE PRECISION NOT NULL,
    cache_read     DOUBLE PRECISION NOT NULL,
    cache_write    DOUBLE PRECISION NOT NULL,
    -- One price a name from a day, and one for always.
    CONSTRAINT model_prices_name_key UNIQUE NULLS NOT DISTINCT (provider, model, effective_from),
    -- NaN sorts above every number, so the bound below infinity refuses it too.
    CONSTRAINT model_prices_price_check CHECK (
        input >= 0 AND input < 'Infinity' AND output >= 0 AND output < 'Infinity'
        AND cache_read >= 0 AND cache_read < 'Infinity'
        AND cache_write >= 0 AND cache_write < 'Infinity')
);
