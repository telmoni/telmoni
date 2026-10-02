-- Superuser half of the role setup. Owner-issued grants are object_grants.sql
-- beside this file, which `telmoni migrate` applies after every run, and each
-- module's migration.
--
-- Applied as a superuser before the first migration, and again after any edit;
-- every statement is idempotent:
--
--   psql "$SUPERUSER_DATABASE_URL" -v ON_ERROR_STOP=1 -f crates/migrator/sql/role_hardening.sql
--
-- The four login roles must exist first, each with its own password: `auth`,
-- `notifications` and `agent`, which the server's pools connect as, and
-- `migrator`. This file creates only the maintenance lanes, which nobody logs
-- in as.
--
-- The agent's tables need the `vector` extension, which only a superuser
-- installs; it is not this file's (see crates/agent/migrations).

-- ══════════════════════════════════════════════════════════════════════════════
-- 1. LOGIN ROLES
-- ══════════════════════════════════════════════════════════════════════════════
-- A superuser, or a role with BYPASSRLS, skips row-level security even on a
-- FORCEd table, so a module's role holding either voids tenant isolation.
DO $$
DECLARE
    svc text;
BEGIN
    IF current_user IN ('auth', 'notifications', 'agent', 'migrator') THEN
        RAISE EXCEPTION 'run this as a superuser of your own, not as %', current_user;
    END IF;
    FOREACH svc IN ARRAY ARRAY['auth', 'notifications', 'agent', 'migrator'] LOOP
        IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = svc) THEN
            RAISE EXCEPTION 'role % does not exist; create it, with a password, before this file runs', svc;
        END IF;
        EXECUTE format('ALTER ROLE %I NOSUPERUSER NOCREATEROLE NOCREATEDB', svc);
    END LOOP;
    FOREACH svc IN ARRAY ARRAY['auth', 'notifications', 'agent'] LOOP
        EXECUTE format('ALTER ROLE %I NOBYPASSRLS', svc);
    END LOOP;
END $$;


-- ══════════════════════════════════════════════════════════════════════════════
-- 2. RUNTIME ROLE SESSION DEFAULTS
-- ══════════════════════════════════════════════════════════════════════════════
ALTER ROLE auth          SET statement_timeout = '5s';
ALTER ROLE auth          SET lock_timeout      = '2s';
ALTER ROLE notifications SET statement_timeout = '5s';
ALTER ROLE notifications SET lock_timeout      = '2s';
ALTER ROLE agent         SET statement_timeout = '5s';
ALTER ROLE agent         SET lock_timeout      = '2s';

-- A leaked transaction would hold its row and advisory locks (an organization's
-- audit chain among them) until the pool recycled it. Auth's sweeps hold their
-- leader lock on a transaction of its own, pinged every 30 seconds while the
-- tick runs: a leader that crashes frees it at once, one cut off from the
-- database two minutes after its last ping, and a tick stuck on an outside
-- call when its time budget runs out (crates/auth/src/sweep.rs).
ALTER ROLE auth          SET idle_in_transaction_session_timeout = '2min';
ALTER ROLE notifications SET idle_in_transaction_session_timeout = '2min';
ALTER ROLE agent         SET idle_in_transaction_session_timeout = '2min';

ALTER ROLE auth          SET search_path = auth;
ALTER ROLE notifications SET search_path = notifications;
-- `public` too: the `vector` type and its operators live there.
ALTER ROLE agent         SET search_path = agent, public;


-- ══════════════════════════════════════════════════════════════════════════════
-- 3. PUBLIC SCHEMA
-- ══════════════════════════════════════════════════════════════════════════════
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
DO $$
BEGIN
    EXECUTE format('REVOKE TEMPORARY ON DATABASE %I FROM PUBLIC', current_database());
END $$;


-- ══════════════════════════════════════════════════════════════════════════════
-- 4. MIGRATOR
-- ══════════════════════════════════════════════════════════════════════════════
ALTER ROLE migrator BYPASSRLS;
ALTER ROLE migrator SET statement_timeout = '5min';
ALTER ROLE migrator SET lock_timeout      = '30s';

DO $$
BEGIN
    EXECUTE format('GRANT CREATE ON DATABASE %I TO migrator', current_database());
END $$;
-- The migrator creates each schema before its ledger, so nothing it runs lands in `public`.
REVOKE CREATE ON SCHEMA public FROM migrator;


-- ══════════════════════════════════════════════════════════════════════════════
-- 5. MAINTENANCE LANES
-- ══════════════════════════════════════════════════════════════════════════════
DO $$
DECLARE
    lane text;
BEGIN
    FOREACH lane IN ARRAY ARRAY['auth_maintenance', 'notifications_maintenance', 'agent_maintenance'] LOOP
        BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = lane) THEN
                EXECUTE format('CREATE ROLE %I NOLOGIN', lane);
            END IF;
        EXCEPTION
            WHEN duplicate_object THEN NULL;
            WHEN unique_violation THEN NULL;
        END;
    END LOOP;
END $$;

GRANT auth_maintenance TO auth WITH INHERIT FALSE, SET TRUE;
GRANT notifications_maintenance TO notifications WITH INHERIT FALSE, SET TRUE;
GRANT agent_maintenance TO agent WITH INHERIT FALSE, SET TRUE;
