-- Owner-issued grants for the runtime roles, applied by the migrator binary as
-- the schema owner after every migrate run. Each role reaches its own schema
-- and the audit append surface, nothing else. Maintenance lanes are granted
-- table by table in each module's migration. A module outside this
-- repository brings its own file, which the migrator applies after this one:
-- the ones MIGRATION_GRANTS lists, else every .sql under /app/grants.

-- ══════════════════════════════════════════════════════════════════════════════
-- 1. AUDIT APPEND SURFACE
-- ══════════════════════════════════════════════════════════════════════════════
-- On the parent only: a query naming a child partition bypasses the parent's policies.
DO $$
BEGIN
    EXECUTE 'GRANT USAGE ON SCHEMA audit TO auth, notifications, telemetry';
    EXECUTE 'GRANT SELECT, INSERT ON audit.events TO auth, notifications, telemetry';
EXCEPTION
    WHEN invalid_schema_name THEN
        RAISE NOTICE 'audit schema not yet created — grants land on next re-apply';
    WHEN undefined_table THEN
        RAISE NOTICE 'audit.events not yet created — grants land on next re-apply';
END $$;


-- ══════════════════════════════════════════════════════════════════════════════
-- 2. AUTH
-- ══════════════════════════════════════════════════════════════════════════════
DO $$
BEGIN
    EXECUTE 'GRANT USAGE ON SCHEMA auth TO auth';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA auth TO auth';
    EXECUTE 'ALTER DEFAULT PRIVILEGES FOR ROLE migrator IN SCHEMA auth '
         || 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO auth';
    IF to_regclass('auth._sqlx_migrations') IS NOT NULL THEN
        EXECUTE 'REVOKE ALL ON auth._sqlx_migrations FROM auth';
    END IF;
    -- The latest row per key is the answer, so an INSERT is a flip; `make flag` writes as the owner.
    EXECUTE 'REVOKE INSERT, UPDATE, DELETE ON auth.feature_flags, auth.organization_flags FROM auth';
EXCEPTION
    WHEN invalid_schema_name THEN
        RAISE NOTICE 'auth schema not yet created — auth own-schema grants land on next re-apply';
END $$;


-- ══════════════════════════════════════════════════════════════════════════════
-- 3. NOTIFICATIONS
-- ══════════════════════════════════════════════════════════════════════════════
DO $$
BEGIN
    EXECUTE 'GRANT USAGE ON SCHEMA notifications TO notifications';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA notifications TO notifications';
    EXECUTE 'ALTER DEFAULT PRIVILEGES FOR ROLE migrator IN SCHEMA notifications '
         || 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO notifications';
    IF to_regclass('notifications._sqlx_migrations') IS NOT NULL THEN
        EXECUTE 'REVOKE ALL ON notifications._sqlx_migrations FROM notifications';
    END IF;
    -- The delivery log is a record: a project reads its own, only the lane
    -- writes it, and its rows go with their delivery by cascade, which needs
    -- no grant. Revoked here, after the schema-wide grant, because that grant
    -- is reissued on every run and a revoke in the migration would not stick.
    IF to_regclass('notifications.delivery_attempts') IS NOT NULL THEN
        EXECUTE 'REVOKE INSERT, UPDATE, DELETE ON notifications.delivery_attempts FROM notifications';
    END IF;
EXCEPTION
    WHEN invalid_schema_name THEN
        RAISE NOTICE 'notifications schema not yet created — notifications own-schema grants land on next re-apply';
END $$;


-- ══════════════════════════════════════════════════════════════════════════════
-- 4. AGENT
-- ══════════════════════════════════════════════════════════════════════════════
-- No audit grant: the agent writes nothing a chain records. Its sources are
-- read through the other modules' seams, each in that module's own lane.
DO $$
BEGIN
    EXECUTE 'GRANT USAGE ON SCHEMA agent TO agent';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA agent TO agent';
    EXECUTE 'ALTER DEFAULT PRIVILEGES FOR ROLE migrator IN SCHEMA agent '
         || 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO agent';
    IF to_regclass('agent._sqlx_migrations') IS NOT NULL THEN
        EXECUTE 'REVOKE ALL ON agent._sqlx_migrations FROM agent';
    END IF;
    -- An erasure's fence is the lane's: a request reads whether one stands,
    -- and nothing a request does may lift or forge one. Revoked here, after
    -- the schema-wide grant, which is reissued on every run.
    IF to_regclass('agent.erasures') IS NOT NULL THEN
        EXECUTE 'REVOKE INSERT, UPDATE, DELETE ON agent.erasures FROM agent';
    END IF;
EXCEPTION
    WHEN invalid_schema_name THEN
        RAISE NOTICE 'agent schema not yet created — agent own-schema grants land on next re-apply';
END $$;


-- ══════════════════════════════════════════════════════════════════════════════
-- 5. TELEMETRY
-- ══════════════════════════════════════════════════════════════════════════════
-- Its audit grant is section 1's: a project's content mode is changed by the
-- module's own lane, and recorded in the same transaction. Its spans are
-- ClickHouse's, whose user and grants are the ClickHouse server's own.
DO $$
BEGIN
    EXECUTE 'GRANT USAGE ON SCHEMA telemetry TO telemetry';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA telemetry TO telemetry';
    EXECUTE 'ALTER DEFAULT PRIVILEGES FOR ROLE migrator IN SCHEMA telemetry '
         || 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO telemetry';
    IF to_regclass('telemetry._sqlx_migrations') IS NOT NULL THEN
        EXECUTE 'REVOKE ALL ON telemetry._sqlx_migrations FROM telemetry';
    END IF;
    -- A project's settings row goes with its project or its organization, by
    -- the lane's purge; a request that removed one would set the project's
    -- content mode back with no audit row. Revoked here, after the
    -- schema-wide grant, which is reissued on every run.
    IF to_regclass('telemetry.project_settings') IS NOT NULL THEN
        EXECUTE 'REVOKE DELETE ON telemetry.project_settings FROM telemetry';
    END IF;
EXCEPTION
    WHEN invalid_schema_name THEN
        RAISE NOTICE 'telemetry schema not yet created — telemetry own-schema grants land on next re-apply';
END $$;
