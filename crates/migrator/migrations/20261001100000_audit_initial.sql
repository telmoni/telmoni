CREATE SCHEMA IF NOT EXISTS audit;

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
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'notifications_maintenance') THEN
        CREATE ROLE notifications_maintenance NOLOGIN;
    END IF;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN unique_violation THEN NULL;
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'notifications_maintenance not created here (no CREATEROLE) — role_hardening.sql owns it in prod';
END $$;

GRANT USAGE ON SCHEMA audit TO auth_maintenance, notifications_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 1. AUDIT EVENTS (PARTITIONED PARENT)
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE audit.events (
    id              UUID        NOT NULL DEFAULT gen_random_uuid(),
    organization_id TEXT        NOT NULL,
    in_project      TEXT,
    actor_id        TEXT        NOT NULL,
    action          TEXT        NOT NULL,
    resource_kind   TEXT        NOT NULL,
    resource_id     TEXT,
    request_id      TEXT,
    ip_address      INET,
    user_agent      TEXT,
    metadata        JSONB,
    seq             BIGINT      NOT NULL,
    prev_hash       TEXT,
    row_hash        TEXT        NOT NULL,
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, created_at),
    CONSTRAINT events_genesis_check CHECK (seq >= 1 AND (seq = 1) = (prev_hash IS NULL))
) PARTITION BY RANGE (created_at);

CREATE INDEX events_organization_idx     ON audit.events (organization_id, id DESC);
CREATE INDEX events_organization_seq_idx ON audit.events (organization_id, seq);
CREATE INDEX events_actor_idx            ON audit.events (organization_id, actor_id, id DESC);
CREATE INDEX events_shard_key_idx        ON audit.events (shard_key);
CREATE INDEX events_in_project_idx       ON audit.events (organization_id, in_project, id DESC)
    WHERE in_project IS NOT NULL;

ALTER TABLE audit.events ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit.events FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON audit.events
    USING      (organization_id = current_setting('app.organization_id', true))
    WITH CHECK (organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON audit.events
    TO auth_maintenance, notifications_maintenance
    USING      (current_user IN ('auth_maintenance', 'notifications_maintenance'))
    WITH CHECK (current_user IN ('auth_maintenance', 'notifications_maintenance'));

GRANT SELECT, INSERT ON audit.events TO auth_maintenance, notifications_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 2. PARTITIONS
-- ══════════════════════════════════════════════════════════════════════════════
-- The runway `telmoni rotate` keeps — this month and the three ahead, the
-- months `months_to_create` plans in crates/shared/src/db/retention.rs
-- (BUFFER_MONTHS), named as it names them — created from the clock rather
-- than listed, so a database is writable whenever it is created and the
-- nightly rotation takes over from there. A listed run of months runs out.
DO $$
DECLARE
    first date := date_trunc('month', now())::date;
    m     date;
BEGIN
    FOR i IN 0..3 LOOP
        m := (first + make_interval(months => i))::date;
        EXECUTE format(
            'CREATE TABLE IF NOT EXISTS audit.events_%s PARTITION OF audit.events '
            || 'FOR VALUES FROM (%L) TO (%L)',
            to_char(m, 'YYYY_MM'),
            to_char(m, 'YYYY-MM-DD'),
            to_char((m + make_interval(months => 1))::date, 'YYYY-MM-DD'));
    END LOOP;
END $$;


-- ══════════════════════════════════════════════════════════════════════════════
-- 3. CHILD PARTITION RLS ENFORCEMENT
-- ══════════════════════════════════════════════════════════════════════════════
-- Postgres applies a partitioned parent's row-security policies only to queries
-- routed through the parent. Children queried directly require identical RLS.
DO $$
DECLARE
    child record;
BEGIN
    FOR child IN
        SELECT c.relname
          FROM pg_inherits i
          JOIN pg_class c     ON c.oid = i.inhrelid
          JOIN pg_class p     ON p.oid = i.inhparent
          JOIN pg_namespace n ON n.oid = p.relnamespace
         WHERE n.nspname = 'audit' AND p.relname = 'events'
    LOOP
        EXECUTE format('ALTER TABLE audit.%I ENABLE ROW LEVEL SECURITY', child.relname);
        EXECUTE format('ALTER TABLE audit.%I FORCE ROW LEVEL SECURITY', child.relname);

        IF NOT EXISTS (
            SELECT 1 FROM pg_policy p
              JOIN pg_class c     ON c.oid = p.polrelid
              JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'audit' AND c.relname = child.relname
               AND p.polname = 'tenant_isolation'
        ) THEN
            EXECUTE format(
                'CREATE POLICY tenant_isolation ON audit.%I '
                 || 'USING (organization_id = current_setting(''app.organization_id'', true)) '
                 || 'WITH CHECK (organization_id = current_setting(''app.organization_id'', true))',
                child.relname);
        END IF;

        IF NOT EXISTS (
            SELECT 1 FROM pg_policy p
              JOIN pg_class c     ON c.oid = p.polrelid
              JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'audit' AND c.relname = child.relname
               AND p.polname = 'maintenance_access'
        ) THEN
            EXECUTE format(
                'CREATE POLICY maintenance_access ON audit.%I '
                 || 'TO auth_maintenance, notifications_maintenance '
                 || 'USING (current_user IN (''auth_maintenance'', ''notifications_maintenance'')) '
                 || 'WITH CHECK (current_user IN (''auth_maintenance'', ''notifications_maintenance''))',
                child.relname);
        END IF;
    END LOOP;
END $$;
