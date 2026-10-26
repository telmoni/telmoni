-- The telemetry module's ClickHouse store: the spans and their hourly rollup.
-- `telmoni migrate` applies this file after the Postgres sets, as the user
-- MIGRATOR_CLICKHOUSE_URL names, one statement per request — the HTTP
-- interface takes one at a time — and applies all of it on every run, so
-- every statement is IF NOT EXISTS. Names are fully qualified: the
-- migrator's connection names no database.
--
-- One node anywhere: MergeTree engines, never a Replicated one, in an Atomic
-- database, and no ON CLUSTER. Metadata only: no column holds content.

CREATE DATABASE IF NOT EXISTS telemetry ENGINE = Atomic;

-- The file's ledger: the digest of the statements this store is made from,
-- written once, by the first run, before any statement below. It comes
-- first because a run reads it before going on: one whose statements differ
-- stops here, since IF NOT EXISTS would skip the difference in silence
-- (`schema.rs`).
CREATE TABLE IF NOT EXISTS telemetry.migrations
(
    digest     String,
    applied_at DateTime64(3, 'UTC') DEFAULT now64(3)
)
ENGINE = MergeTree
ORDER BY applied_at;

-- One row per finished span — a run, a step, a model or tool call, or any
-- other span under a run — written once, when it arrives, and never updated,
-- so the table is read without FINAL; a row leaves only with its day's
-- retention or its project's deletion. A span carries its project and nothing
-- above it: a project moves between organizations without a row rewritten.
--
-- Partitioned by the day it ARRIVED, by the server's clock, so the nightly
-- purge counts its retention from then and no client's clock decides when it
-- goes; ordered by project and start, the order every read narrows by. A
-- trace is read by its run's id, which no key holds and no window bounds, so
-- a bloom filter a granule lets that read skip every granule but the run's
-- instead of its project's whole history.
CREATE TABLE IF NOT EXISTS telemetry.spans
(
    -- The server's own: the key's project, its clock at receipt, and what
    -- the span counts against its plan.
    project_id               String,
    received_at              DateTime64(6, 'UTC'),
    units                    UInt32,
    -- The contract's fields, each under its own name. A closed set is its
    -- enum's text, with no CHECK: the one query module is the only writer.
    run_id                   UUID,
    agent                    LowCardinality(String),
    trace_id                 String,
    span_id                  String,
    parent_span_id           String,
    kind                     LowCardinality(String),
    name                     String,
    started_at               DateTime64(6, 'UTC'),
    ended_at                 DateTime64(6, 'UTC'),
    status                   LowCardinality(String),
    error_type               LowCardinality(String),
    model                    LowCardinality(String),
    provider                 LowCardinality(String),
    input_tokens             UInt64,
    output_tokens            UInt64,
    cache_read_input_tokens  UInt64,
    cache_write_input_tokens UInt64,
    -- What a call cost in US dollars as reported, NULL where it reported
    -- none: a read prices those by their tokens, and never writes the price.
    cost                     Nullable(Float64),
    user_id                  String,
    session_id               String,
    -- The deployment the span's resource named, and its release; empty
    -- where it named none.
    environment              LowCardinality(String),
    release                  LowCardinality(String),
    tags                     Map(LowCardinality(String), String),
    -- A run's sample rate, NULL where it set none and on every other kind.
    sample_rate              Nullable(Float64),
    -- The attributes the field inventory names, and any other whose value is
    -- a number or a boolean; an array is kept as its JSON.
    attributes               Map(LowCardinality(String), String),
    INDEX run_id_idx run_id TYPE bloom_filter GRANULARITY 1
)
ENGINE = MergeTree
PARTITION BY toDate(received_at)
ORDER BY (project_id, started_at);

-- Each hour's totals, by project, agent, kind, model, provider, a tool call's
-- name, status, the deployment's environment and release, and whether the
-- spans reported their cost: no user and no tag, so it outlives the spans,
-- kept thirteen months. `tool` is empty on every kind but a tool call, so the
-- names an application gives its steps never multiply the rows. A row of
-- spans that reported none sums no cost, so a read prices its tokens alone.
-- Durations are a t-digest in milliseconds, which merges across hours and
-- keys and answers any percentile, and their sum, which an average needs.
CREATE TABLE IF NOT EXISTS telemetry.spans_hourly
(
    project_id               String,
    hour                     DateTime('UTC'),
    agent                    LowCardinality(String),
    kind                     LowCardinality(String),
    model                    LowCardinality(String),
    provider                 LowCardinality(String),
    tool                     LowCardinality(String),
    status                   LowCardinality(String),
    environment              LowCardinality(String),
    release                  LowCardinality(String),
    cost_reported            Bool,
    spans                    SimpleAggregateFunction(sum, UInt64),
    units                    SimpleAggregateFunction(sum, UInt64),
    input_tokens             SimpleAggregateFunction(sum, UInt64),
    output_tokens            SimpleAggregateFunction(sum, UInt64),
    cache_read_input_tokens  SimpleAggregateFunction(sum, UInt64),
    cache_write_input_tokens SimpleAggregateFunction(sum, UInt64),
    cost                     SimpleAggregateFunction(sum, Float64),
    duration_ms_sum          SimpleAggregateFunction(sum, Float64),
    duration_ms              AggregateFunction(quantilesTDigest(0.5, 0.9, 0.99), Float64)
)
ENGINE = AggregatingMergeTree
PARTITION BY toYYYYMM(hour)
ORDER BY (project_id, hour, agent, kind, model, provider, tool, status, environment, release,
          cost_reported);

-- ⚠ The one second table an insert into `spans` writes, in the same flush:
-- a view that failed would fail the insert after its spans were written, so
-- its query is one no row `spans` took can fail. It runs as its definer, the
-- user this file runs as, whom `tenant_isolation` does not hold; run as the
-- user inserting, the policy would read a setting no insert carries, and
-- fail. `spans.cost` is named whole: bare, `cost` reads as the sum's alias.
CREATE MATERIALIZED VIEW IF NOT EXISTS telemetry.spans_hourly_mv
TO telemetry.spans_hourly
DEFINER = CURRENT_USER SQL SECURITY DEFINER
AS SELECT
    project_id,
    toStartOfHour(received_at) AS hour,
    agent,
    kind,
    model,
    provider,
    if(kind = 'tool_call', name, '') AS tool,
    status,
    environment,
    release,
    isNotNull(spans.cost) AS cost_reported,
    count() AS spans,
    sum(units) AS units,
    sum(input_tokens) AS input_tokens,
    sum(output_tokens) AS output_tokens,
    sum(cache_read_input_tokens) AS cache_read_input_tokens,
    sum(cache_write_input_tokens) AS cache_write_input_tokens,
    sum(ifNull(spans.cost, 0)) AS cost,
    sum(greatest(dateDiff('microsecond', started_at, ended_at), 0) / 1000) AS duration_ms_sum,
    quantilesTDigestState(0.5, 0.9, 0.99)(
        greatest(dateDiff('microsecond', started_at, ended_at), 0) / 1000
    ) AS duration_ms
FROM telemetry.spans
GROUP BY project_id, hour, agent, kind, model, provider, tool, status, environment, release,
    cost_reported;

-- ⚠ Tenancy: a read sees only the projects its query's setting
-- `SQL_telmoni_projects` names, a JSON array the one query module sets on
-- every read; a read naming none fails on the unknown setting, never answers
-- every project's rows. Every user but the one this file runs as — on a tier
-- the module's `telemetry`, never `migrator` — is held by it. No insert
-- carries the setting: ClickHouse gathers into one asynchronous insert only
-- inserts whose settings match, and a policy filters reads alone.
CREATE ROW POLICY IF NOT EXISTS tenant_isolation ON telemetry.spans
    USING has(JSONExtract(getSetting('SQL_telmoni_projects'), 'Array(String)'), project_id)
    TO ALL EXCEPT CURRENT_USER;

CREATE ROW POLICY IF NOT EXISTS tenant_isolation ON telemetry.spans_hourly
    USING has(JSONExtract(getSetting('SQL_telmoni_projects'), 'Array(String)'), project_id)
    TO ALL EXCEPT CURRENT_USER;

-- The one this file runs as reads every row by a policy of its own: the
-- nightly purge lists the partitions it drops from the rows, and the view
-- runs as the same user. A user no policy names reads rows only while the
-- server keeps `users_without_row_policies_can_read_rows` on; with it off
-- the purge would list and drop nothing, night after night, and the view
-- would total no span, both in silence.
CREATE ROW POLICY IF NOT EXISTS migrator_reads_all ON telemetry.spans
    USING 1
    TO CURRENT_USER;

CREATE ROW POLICY IF NOT EXISTS migrator_reads_all ON telemetry.spans_hourly
    USING 1
    TO CURRENT_USER;
