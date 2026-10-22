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

CREATE INDEX project_settings_organization_idx ON telemetry.project_settings (organization_id);
-- The key a deleted layout's SET NULL follows (the rule at `monitors_organization_idx`).
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
-- 3. MONITORS
-- ══════════════════════════════════════════════════════════════════════════════
-- An agent's contract, one monitor an agent: when its runs are expected — a
-- five-field cron line in the monitor's own zone, or an interval, or neither
-- for one that only holds its runs to their bounds — how long past the due
-- time a run may start, how long one is expected to take, and the bounds every
-- run is held to, the tighter of these and the run's own. Filed under the
-- project and its organization both: the organization's plan counts its
-- monitors across its projects, and its purge finds them by it. The slug is
-- what every crontab line and ping carries, fixed once saved; the agent is
-- the `telmoni.agent` its runs carry, the slug unless set otherwise.
CREATE TABLE telemetry.monitors (
    id                  UUID        PRIMARY KEY,
    project_id          TEXT        NOT NULL,
    organization_id     TEXT        NOT NULL,
    slug                TEXT        NOT NULL,
    name                TEXT        NOT NULL,
    agent               TEXT        NOT NULL,
    schedule            TEXT,
    interval_s          INTEGER,
    timezone            TEXT        NOT NULL DEFAULT 'UTC',
    -- Meaningless without a schedule or an interval, so held to one.
    grace_s             INTEGER,
    expected_duration_s INTEGER,
    stall_after_s       INTEGER,
    max_steps           INTEGER,
    max_cost_usd        DOUBLE PRECISION,
    -- `MonitorState`'s word; `new` is a monitor a first ping or run made.
    state               TEXT        NOT NULL DEFAULT 'healthy',
    state_changed_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- When the next run is expected, in UTC; none for a monitor with no
    -- schedule, and none while it is paused.
    next_due_at         TIMESTAMPTZ,
    -- Its last run that ended, written as the run ends — by the ping that
    -- ended it, the ping after one that never said it ended, or the ticker
    -- — so the project's board reads a row and sums nothing: which run,
    -- when, how it ended (`SpanStatus`'s word), and its steps and spend
    -- where it reported any. A run that ended before the one kept never
    -- replaces it.
    last_run_id         UUID,
    last_run_started_at TIMESTAMPTZ,
    last_run_ended_at   TIMESTAMPTZ,
    last_run_status     TEXT,
    last_run_steps      INTEGER,
    last_run_cost_usd   DOUBLE PRECISION,
    -- When its last run that ended well ended, written beside the last run
    -- and never moved back, so the monitor's page names its last success
    -- however many runs have failed since.
    last_success_at     TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT monitors_slug_check CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$' AND char_length(slug) <= 48),
    CONSTRAINT monitors_name_check CHECK (char_length(name) BETWEEN 1 AND 60),
    CONSTRAINT monitors_agent_check CHECK (char_length(agent) BETWEEN 1 AND 128),
    CONSTRAINT monitors_timezone_check CHECK (char_length(timezone) BETWEEN 1 AND 64),
    CONSTRAINT monitors_one_timing_check CHECK (schedule IS NULL OR interval_s IS NULL),
    CONSTRAINT monitors_grace_with_timing_check CHECK ((grace_s IS NULL) = (schedule IS NULL AND interval_s IS NULL)),
    CONSTRAINT monitors_interval_check CHECK (interval_s IS NULL OR interval_s >= 60),
    CONSTRAINT monitors_grace_check CHECK (grace_s IS NULL OR grace_s >= 0),
    CONSTRAINT monitors_expected_duration_check CHECK (expected_duration_s IS NULL OR expected_duration_s > 0),
    CONSTRAINT monitors_stall_check CHECK (stall_after_s IS NULL OR stall_after_s > 0),
    CONSTRAINT monitors_steps_check CHECK (max_steps IS NULL OR max_steps > 0),
    CONSTRAINT monitors_cost_check CHECK (max_cost_usd IS NULL OR max_cost_usd > 0),
    CONSTRAINT monitors_state_check CHECK (state IN ('failed', 'stalled', 'over_bound', 'late', 'running', 'new', 'paused', 'healthy')),
    CONSTRAINT monitors_last_run_check CHECK (num_nulls(last_run_id, last_run_started_at, last_run_ended_at, last_run_status) IN (0, 4)),
    CONSTRAINT monitors_last_run_status_check CHECK (last_run_status IN ('ok', 'error', 'cancelled')),
    CONSTRAINT monitors_last_run_steps_check CHECK (last_run_steps IS NULL OR (last_run_id IS NOT NULL AND last_run_steps >= 0)),
    CONSTRAINT monitors_last_run_cost_check CHECK (last_run_cost_usd IS NULL OR (last_run_id IS NOT NULL AND last_run_cost_usd >= 0))
);

-- One slug and one agent per monitor in a project: the slug is what a ping
-- names, the agent what a run names, and each must find one monitor.
CREATE UNIQUE INDEX monitors_project_slug_idx ON telemetry.monitors (project_id, slug);
CREATE UNIQUE INDEX monitors_project_agent_idx ON telemetry.monitors (project_id, agent);
-- The rule for every table here: one the organization's purge deletes by
-- `organization_id` carries an index on it (this, `ping_keys`, `outbox`);
-- one a cascade takes carries an index on the key the cascade follows
-- (`open_runs`, by monitor).
CREATE INDEX monitors_organization_idx ON telemetry.monitors (organization_id);
-- The ticker's scan: the scheduled monitors by when their next run is due.
CREATE INDEX monitors_due_idx ON telemetry.monitors (next_due_at) WHERE next_due_at IS NOT NULL;

ALTER TABLE telemetry.monitors ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.monitors FORCE ROW LEVEL SECURITY;

-- A write names the organization too, as a settings row's does, so a
-- project's scope alone cannot file a monitor under another organization.
CREATE POLICY tenant_isolation ON telemetry.monitors
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true)
                AND organization_id = current_setting('app.organization_id', true));

-- An organization reads its projects' monitors — the count its plan holds
-- it to — and writes none: each is changed on its own project.
CREATE POLICY organization_read ON telemetry.monitors
    FOR SELECT
    USING (organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON telemetry.monitors
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The purges delete an organization's or a deleted project's monitors.
GRANT SELECT, DELETE ON telemetry.monitors TO telemetry_maintenance;
-- A transfer moves them to the organization that now holds their project.
GRANT UPDATE (organization_id) ON telemetry.monitors TO telemetry_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 4. PING KEYS
-- ══════════════════════════════════════════════════════════════════════════════
-- A project's ping keys: what every crontab line carries, in the URL of each
-- ping, minted for pings alone — `telmoniping_` and 32 base62 characters,
-- shown once and kept as its SHA-256, as an API key is — so a line's leak can
-- fake a ping and make a paused monitor, and nothing more. Two may be live at
-- once, so a rotation moves the crontab lines over one at a time: the older is
-- retired by a person, or a week after its last ping once a newer one stands
-- (`rotation.rs`). A key the table does not hold, or holds retired, is a 404.
-- Filed under the project and its organization both, as a monitor is.
CREATE TABLE telemetry.ping_keys (
    id              UUID        PRIMARY KEY,
    project_id      TEXT        NOT NULL,
    organization_id TEXT        NOT NULL,
    -- The SHA-256 of the key, in hex; the key itself is kept nowhere.
    key_hash        TEXT        NOT NULL,
    -- What the list shows of it: the prefix and the first characters after
    -- it, enough to tell two keys apart and nothing a guesser gains by.
    prefix          TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Touched by its pings, at most once a minute.
    last_ping_at    TIMESTAMPTZ,
    retired_at      TIMESTAMPTZ,
    CONSTRAINT ping_keys_key_hash_check CHECK (key_hash ~ '^[0-9a-f]{64}$'),
    CONSTRAINT ping_keys_prefix_check CHECK (char_length(prefix) BETWEEN 1 AND 32)
);

-- The ping lane finds a key by its hash, and by nothing else.
CREATE UNIQUE INDEX ping_keys_hash_idx ON telemetry.ping_keys (key_hash);
-- A project's keys, the retired ones with the live: the list, which shows
-- both, the count a minting is held to, the rotation's search for a newer
-- key standing, and the project's purge and move. Retired keys are kept
-- while their project stands, so an index of the live ones alone left the
-- list and the purge a walk of every tenant's keys.
CREATE INDEX ping_keys_project_idx ON telemetry.ping_keys (project_id, created_at);
CREATE INDEX ping_keys_organization_idx ON telemetry.ping_keys (organization_id);

ALTER TABLE telemetry.ping_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.ping_keys FORCE ROW LEVEL SECURITY;

-- A write names the organization too, as a monitor's does.
CREATE POLICY tenant_isolation ON telemetry.ping_keys
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true)
                AND organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON telemetry.ping_keys
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The lane resolves a key outside any tenant, as auth resolves an API key;
-- the rotation finds the keys a week past their last ping; and the purges
-- delete an organization's or a deleted project's keys. A key is minted and
-- retired in the project's own scope.
GRANT SELECT, DELETE ON telemetry.ping_keys TO telemetry_maintenance;
-- A transfer moves them to the organization that now holds their project.
GRANT UPDATE (organization_id) ON telemetry.ping_keys TO telemetry_maintenance;
-- A ping touches its key in the statement that resolves it, as auth touches
-- an API key, so the ping's own transaction takes no lock on the key's row,
-- which the rotation, a retirement, a transfer's move and a purge each take
-- beside the rows a ping writes, in orders of their own.
GRANT UPDATE (last_ping_at) ON telemetry.ping_keys TO telemetry_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 5. OPEN RUNS
-- ══════════════════════════════════════════════════════════════════════════════
-- A row per run still going: opened by a `/start` or a `/checkpoint` ping —
-- and, once ingest opens runs, by a run's `telmoni.run.start` — heard by every
-- ping after it, and gone when the run ends, its finished span handed to the
-- outbox below. A run that never said it ended — a job that crashed after its
-- `/start`, a `/checkpoint` that came after its run's end — is ended `error`
-- by the next ping that finds it: a `/start` naming no run ends what its
-- monitor still has open, and an end ping naming none ends the newest with
-- its verdict and the rest the same way. What it holds is what the ticker
-- reads to see a run gone quiet, past a bound or past its ceiling: when it
-- started, when it was last heard, when it is over whatever it says, its
-- steps and spend so far, and the bounds it is held to — its monitor's, for a
-- run reported by pings, which declare none. A run goes with its monitor.
-- Filed under the project and its organization both, as a monitor is.
CREATE TABLE telemetry.open_runs (
    -- The run's id: the `rid` a ping names, or one minted for it, and the
    -- `run_id` of its span once it ends.
    id              UUID        NOT NULL,
    project_id      TEXT        NOT NULL,
    organization_id TEXT        NOT NULL,
    monitor_id      UUID        NOT NULL REFERENCES telemetry.monitors (id) ON DELETE CASCADE,
    -- The monitor's agent, which the span names.
    agent           TEXT        NOT NULL,
    -- `ping` for a run reported by pings, `ingest` for one an SDK sends.
    source          TEXT        NOT NULL,
    -- The key its pings came under, for a run reported by pings.
    ping_key_id     UUID,
    started_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_heard_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- When the run is over whatever it says: its monitor's expected duration
    -- four times over, or a day where that is shorter or nothing is expected
    -- (`handler/pings.rs`, `ends_by`). The ticker ends a run still open past
    -- it as `error`, since a run that never ends is one that crashed before
    -- its end ping.
    ends_by         TIMESTAMPTZ NOT NULL,
    steps           INTEGER     NOT NULL DEFAULT 0,
    input_tokens    BIGINT      NOT NULL DEFAULT 0,
    output_tokens   BIGINT      NOT NULL DEFAULT 0,
    cost_usd        DOUBLE PRECISION NOT NULL DEFAULT 0,
    stall_after_s   INTEGER,
    max_steps       INTEGER,
    max_cost_usd    DOUBLE PRECISION,
    -- A run's id is its own within its project, as `spans` keys the span
    -- by project: a `rid` a script derives from its own name — `uuidgen
    -- --sha1` on the job's name and the day, the stable UUID the manual
    -- gives — is open in two projects at once, neither told of the other.
    -- The lane opens a run with `ON CONFLICT (project_id, id) DO NOTHING`,
    -- which names this key by its columns, so a change of the key is a
    -- change of that insert. A ping that meets the key reads the run it
    -- found: its own monitor's, opened beside it by another of the run's
    -- pings, it hears; another monitor's, it is refused.
    PRIMARY KEY (project_id, id),
    CONSTRAINT open_runs_agent_check CHECK (char_length(agent) BETWEEN 1 AND 128),
    CONSTRAINT open_runs_source_check CHECK (source IN ('ping', 'ingest')),
    CONSTRAINT open_runs_steps_check CHECK (steps >= 0),
    CONSTRAINT open_runs_tokens_check CHECK (input_tokens >= 0 AND output_tokens >= 0),
    CONSTRAINT open_runs_cost_check CHECK (cost_usd >= 0),
    CONSTRAINT open_runs_stall_check CHECK (stall_after_s IS NULL OR stall_after_s > 0),
    CONSTRAINT open_runs_max_steps_check CHECK (max_steps IS NULL OR max_steps > 0),
    CONSTRAINT open_runs_max_cost_check CHECK (max_cost_usd IS NULL OR max_cost_usd > 0)
);

-- A ping that names no run means its monitor's newest open one: the index's
-- first row, the id breaking a tie, as `newest_open_run` orders them; the
-- monitor's cascade follows it too.
CREATE INDEX open_runs_monitor_idx ON telemetry.open_runs (monitor_id, started_at DESC, id);
-- The ticker's scan for silence: the runs held to a stall timeout, by when
-- they were last heard — every run under a monitor its first ping made has
-- one, the lane's default, and one under a saved monitor its owner's or
-- none.
CREATE INDEX open_runs_heard_idx ON telemetry.open_runs (last_heard_at) WHERE stall_after_s IS NOT NULL;
-- The ticker's scan for runs past their ceiling, which every run has, so a
-- row no stall timeout covers is visited all the same.
CREATE INDEX open_runs_ends_by_idx ON telemetry.open_runs (ends_by);

ALTER TABLE telemetry.open_runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.open_runs FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON telemetry.open_runs
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true)
                AND organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON telemetry.open_runs
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The ticker reads every open run across tenants; the purges delete an
-- organization's or a deleted project's. A run is opened, heard and ended
-- in the project's own scope.
GRANT SELECT, DELETE ON telemetry.open_runs TO telemetry_maintenance;
-- A transfer moves them to the organization that now holds their project.
GRANT UPDATE (organization_id) ON telemetry.open_runs TO telemetry_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 6. OUTBOX
-- ══════════════════════════════════════════════════════════════════════════════
-- What telemetry hands on once a transaction has committed, and must not
-- lose: a ping's finished run, which becomes a row of ClickHouse's `spans`
-- (`kind` = 'span'), and a notice for the project's feed, handed to
-- notifications through the seam (`kind` = 'notice'). A row is written in the
-- transaction that ends the run, makes the monitor or retires the key, so a
-- monitor is never late because ClickHouse was down; a notice is never told
-- twice, since the seam answers a replayed dedup key with its first row, and
-- a run is written once, since the loop reads which of a batch's runs
-- `spans` holds already before it inserts and drops those rows: the loop
-- every replica runs (`outbox.rs`) leases rows across tenants, `FOR UPDATE
-- SKIP LOCKED`, lands each and deletes it; a row ClickHouse or the seam
-- refused, or that does not parse, waits and is tried again up to a cap,
-- past which it stays until `telmoni release outbox`; a row a store outage
-- left is handed back with its try, so an outage strands nothing. Filed
-- under the project and its organization both, as a monitor is.
CREATE TABLE telemetry.outbox (
    id              UUID        PRIMARY KEY,
    project_id      TEXT        NOT NULL,
    organization_id TEXT        NOT NULL,
    kind            TEXT        NOT NULL,
    -- The finished run as `outbox::FinishedRun` spells it, or the notice as
    -- `outbox::QueuedNotice` does.
    payload         JSONB       NOT NULL,
    attempts        INTEGER     NOT NULL DEFAULT 0,
    lease_until     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT outbox_kind_check CHECK (kind IN ('span', 'notice')),
    CONSTRAINT outbox_payload_check CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT outbox_attempts_check CHECK (attempts >= 0)
);

-- The purges find a project's rows and an organization's (the rule at
-- `monitors_organization_idx`); the loop takes rows in physical order.
CREATE INDEX outbox_project_idx ON telemetry.outbox (project_id);
CREATE INDEX outbox_organization_idx ON telemetry.outbox (organization_id);

ALTER TABLE telemetry.outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.outbox FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON telemetry.outbox
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true)
                AND organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON telemetry.outbox
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The loop leases rows across tenants and deletes each once it has landed,
-- as the purges delete a project's; a lease is two columns, granted as
-- columns. A row is written in the project's own scope.
GRANT SELECT, DELETE ON telemetry.outbox TO telemetry_maintenance;
GRANT UPDATE (attempts, lease_until) ON telemetry.outbox TO telemetry_maintenance;
-- A transfer moves them to the organization that now holds their project.
GRANT UPDATE (organization_id) ON telemetry.outbox TO telemetry_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 7. INCIDENTS
-- ══════════════════════════════════════════════════════════════════════════════
-- One row per spell a monitor needed someone: opened as its state moves into
-- failed, stalled, over a bound or late, resolved as it is healthy again or
-- its owner pauses it, and the one table the alert rules will write too. A
-- monitor holds one open incident at most, however many runs fail while it
-- stands; one that flips in and out of trouble more than six times in an
-- hour is flapping, its incident kept open and marked so until it has held
-- one state for an hour, so a flapping job costs one incident, not hundreds
-- (`incidents.rs`). Each opening and each resolution is a notice, through
-- the outbox. Filed under the project and its organization both, as a
-- monitor is; gone with its monitor.
CREATE TABLE telemetry.incidents (
    id              UUID        PRIMARY KEY,
    project_id      TEXT        NOT NULL,
    organization_id TEXT        NOT NULL,
    monitor_id      UUID        NOT NULL REFERENCES telemetry.monitors (id) ON DELETE CASCADE,
    -- The state that opened it, `MonitorState`'s word, and why, finer:
    -- `incidents::Trigger`'s word.
    cause           TEXT        NOT NULL,
    detail          TEXT        NOT NULL,
    opened_at       TIMESTAMPTZ NOT NULL,
    resolved_at     TIMESTAMPTZ,
    -- How it closed: the monitor healthy again, or paused by its owner.
    resolution      TEXT,
    flapping        BOOLEAN     NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT incidents_cause_check CHECK (cause IN ('failed', 'stalled', 'over_bound', 'late')),
    CONSTRAINT incidents_detail_check CHECK (detail IN (
        'run_failed', 'run_never_came', 'run_never_ended', 'run_late', 'run_stalled', 'run_over_bound'
    )),
    CONSTRAINT incidents_resolution_check CHECK (
        (resolution IS NULL) = (resolved_at IS NULL)
        AND (resolution IS NULL OR resolution IN ('recovered', 'paused'))
    )
);

-- One open incident a monitor.
CREATE UNIQUE INDEX incidents_open_idx ON telemetry.incidents (monitor_id) WHERE resolved_at IS NULL;
-- The flip count's read, a monitor's incidents by when they opened, and the
-- cascade's key (the rule at `monitors_organization_idx`).
CREATE INDEX incidents_monitor_idx ON telemetry.incidents (monitor_id, opened_at DESC);
-- A project's incidents, newest first; and the purges' read of an
-- organization's.
CREATE INDEX incidents_project_idx ON telemetry.incidents (project_id, opened_at DESC);
CREATE INDEX incidents_organization_idx ON telemetry.incidents (organization_id);

ALTER TABLE telemetry.incidents ENABLE ROW LEVEL SECURITY;
ALTER TABLE telemetry.incidents FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON telemetry.incidents
    USING      (project_id = current_setting('app.project_id', true))
    WITH CHECK (project_id = current_setting('app.project_id', true)
                AND organization_id = current_setting('app.organization_id', true));

-- An organization reads its projects' incidents — its health across them —
-- and writes none: each is opened and resolved on its own project.
CREATE POLICY organization_read ON telemetry.incidents
    FOR SELECT
    USING (organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON telemetry.incidents
    TO telemetry_maintenance
    USING      (current_user = 'telemetry_maintenance')
    WITH CHECK (current_user = 'telemetry_maintenance');

-- The ticker reads across tenants for a flapping incident whose monitor has
-- held its state an hour; the purges take an incident with its monitor.
GRANT SELECT, DELETE ON telemetry.incidents TO telemetry_maintenance;
-- A transfer moves them to the organization that now holds their project.
GRANT UPDATE (organization_id) ON telemetry.incidents TO telemetry_maintenance;
