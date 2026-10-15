//! The telemetry module's Postgres: each project's settings, its saved views
//! of the trace list, its monitors, its ping keys, its runs still going, and
//! the outbox of what it hands on once a transaction has committed.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{
    HasProject, Lane, Maintenance, MaintenanceLane, ProjectAndOrganization, Scoped,
};
use telmoni_shared::{ContentMode, MonitorState, OrganizationId, ProjectId};

/// This module's cross-tenant lane: the one declaration, so a sibling's lane
/// is unnameable here. See `tenant_session::Lane`.
#[derive(Debug, Clone, Copy)]
pub struct TelemetryLane;
impl Lane for TelemetryLane {
    const SET_ROLE: &'static str = MaintenanceLane::Telemetry.set_role();
}

/// A project's content mode: its own row's, or `off` where it has none.
pub async fn content_mode<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<ContentMode> {
    let mode: Option<ContentMode> = sqlx::query_scalar(
        "SELECT content_mode FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    Ok(mode.unwrap_or_default())
}

/// A project's row, locked for the change about to be written: the
/// organization it is filed under and its mode, or `None` where it has none.
pub async fn settings_for_update(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<(OrganizationId, ContentMode)>> {
    sqlx::query_as(
        "SELECT organization_id, content_mode FROM telemetry.project_settings
          WHERE project_id = $1
            FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await
}

/// A project's content mode, set: its row made where it has none, and filed
/// under `organization_id`, the organization the scope binds, which heals a
/// row a transfer's failed settling left under another.
pub async fn set_content_mode(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    mode: ContentMode,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id, content_mode)
         VALUES ($1, $2, $3)
         ON CONFLICT (project_id) DO UPDATE
            SET organization_id = EXCLUDED.organization_id,
                content_mode    = EXCLUDED.content_mode",
    )
    .bind(project_id)
    .bind(organization_id)
    .bind(mode)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The projects an organization's settings rows name: every project that
/// sent a span while it held it, which its purge must find in ClickHouse,
/// since a span names no organization.
pub async fn projects_of(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    organization_id: &OrganizationId,
) -> sqlx::Result<Vec<ProjectId>> {
    sqlx::query_scalar(
        "SELECT project_id FROM telemetry.project_settings
          WHERE organization_id = $1
          ORDER BY project_id",
    )
    .bind(organization_id)
    .fetch_all(tx.conn())
    .await
}

/// Every settings row of an organization's projects, their saved views,
/// found through those rows before they go, since a view names no
/// organization, and their monitors, ping keys, open runs and outbox rows,
/// which name it.
pub async fn purge_organization(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    organization_id: &OrganizationId,
) -> sqlx::Result<u64> {
    let mut purged = 0;
    for table in ORGANIZATION_TABLES {
        purged += sqlx::query(&format!("DELETE FROM {table} WHERE organization_id = $1"))
            .bind(organization_id)
            .execute(tx.conn())
            .await?
            .rows_affected();
    }
    let views = sqlx::query(
        "DELETE FROM telemetry.trace_views
          WHERE project_id IN (SELECT project_id FROM telemetry.project_settings
                                WHERE organization_id = $1)",
    )
    .bind(organization_id)
    .execute(tx.conn())
    .await?
    .rows_affected();
    let settings = sqlx::query("DELETE FROM telemetry.project_settings WHERE organization_id = $1")
        .bind(organization_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    Ok(purged + views + settings)
}

/// The tables filed under a project's organization as well as the project,
/// in the order a purge takes them: what hangs off a monitor before the
/// monitor, since a run goes with its monitor by cascade and is counted
/// here instead. The settings row, which the purges find a project by, and
/// the views, which name no organization, are taken apart from these.
const ORGANIZATION_TABLES: [&str; 4] = [
    "telemetry.outbox",
    "telemetry.open_runs",
    "telemetry.ping_keys",
    "telemetry.monitors",
];

/// One project's outbox rows, open runs, ping keys, monitors, saved views
/// and settings row.
pub async fn purge_project(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
) -> sqlx::Result<u64> {
    let mut purged = 0;
    for table in ORGANIZATION_TABLES {
        purged += sqlx::query(&format!("DELETE FROM {table} WHERE project_id = $1"))
            .bind(project_id)
            .execute(tx.conn())
            .await?
            .rows_affected();
    }
    let views = sqlx::query("DELETE FROM telemetry.trace_views WHERE project_id = $1")
        .bind(project_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    let settings = sqlx::query("DELETE FROM telemetry.project_settings WHERE project_id = $1")
        .bind(project_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    Ok(purged + views + settings)
}

/// A saved view as its row keeps it. Its query is the JSON the lane checked
/// when it was written; its columns, `TraceColumn`'s words.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ViewRow {
    pub id: Uuid,
    pub name: String,
    pub query: serde_json::Value,
    pub columns: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What saving a view writes.
#[derive(Debug, Clone, Copy)]
pub struct ViewWrite<'a> {
    pub name: &'a str,
    pub query: &'a serde_json::Value,
    pub columns: &'a [String],
}

/// The project's saved views, by name.
pub async fn views<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<ViewRow>> {
    sqlx::query_as(
        "SELECT id, name, query, columns, created_at, updated_at
           FROM telemetry.trace_views
          WHERE project_id = $1
          ORDER BY lower(name), id",
    )
    .bind(project_id)
    .fetch_all(tx.conn())
    .await
}

/// How many views the project keeps.
pub async fn view_count<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM telemetry.trace_views WHERE project_id = $1")
        .bind(project_id)
        .fetch_one(tx.conn())
        .await
}

/// A new view of the project. Its name taken already, in any case, is the
/// unique index's refusal, `trace_views_project_name_idx`.
pub async fn insert_view(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    id: Uuid,
    view: ViewWrite<'_>,
) -> sqlx::Result<ViewRow> {
    sqlx::query_as(
        "INSERT INTO telemetry.trace_views (id, project_id, name, query, columns)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, name, query, columns, created_at, updated_at",
    )
    .bind(id)
    .bind(project_id)
    .bind(view.name)
    .bind(view.query)
    .bind(view.columns)
    .fetch_one(tx.conn())
    .await
}

/// One of the project's views, written over, or `None` where it has no view
/// by that id.
pub async fn update_view<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    view: ViewWrite<'_>,
) -> sqlx::Result<Option<ViewRow>> {
    sqlx::query_as(
        "UPDATE telemetry.trace_views
            SET name = $3, query = $4, columns = $5, updated_at = now()
          WHERE project_id = $1 AND id = $2
         RETURNING id, name, query, columns, created_at, updated_at",
    )
    .bind(project_id)
    .bind(id)
    .bind(view.name)
    .bind(view.query)
    .bind(view.columns)
    .fetch_optional(tx.conn())
    .await
}

/// One of the project's views, removed. Answers whether there was one.
pub async fn delete_view<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<bool> {
    Ok(
        sqlx::query("DELETE FROM telemetry.trace_views WHERE project_id = $1 AND id = $2")
            .bind(project_id)
            .bind(id)
            .execute(tx.conn())
            .await?
            .rows_affected()
            > 0,
    )
}

/// A monitor as its row keeps it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MonitorRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub agent: String,
    pub schedule: Option<String>,
    pub interval_s: Option<i32>,
    pub timezone: String,
    pub grace_s: Option<i32>,
    pub expected_duration_s: Option<i32>,
    pub stall_after_s: Option<i32>,
    pub max_steps: Option<i32>,
    pub max_cost_usd: Option<f64>,
    pub state: MonitorState,
    pub state_changed_at: DateTime<Utc>,
    pub next_due_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The columns a [`MonitorRow`] is read from, in its order.
const MONITOR_COLUMNS: &str = "id, slug, name, agent, schedule, interval_s, timezone, grace_s, \
     expected_duration_s, stall_after_s, max_steps, max_cost_usd, state, state_changed_at, \
     next_due_at, created_at, updated_at";

/// What making or changing a monitor writes: its definition, checked by the
/// lane, and when its next run is due under it.
#[derive(Debug, Clone, Copy)]
pub struct MonitorWrite<'a> {
    pub slug: &'a str,
    pub name: &'a str,
    pub agent: &'a str,
    pub schedule: Option<&'a str>,
    pub interval_s: Option<i32>,
    pub timezone: &'a str,
    pub grace_s: Option<i32>,
    pub expected_duration_s: Option<i32>,
    pub stall_after_s: Option<i32>,
    pub max_steps: Option<i32>,
    pub max_cost_usd: Option<f64>,
    pub next_due_at: Option<DateTime<Utc>>,
}

/// The project's monitors: the worst state first, then by name.
pub async fn monitors<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<MonitorRow>> {
    let order: Vec<&str> = MonitorState::all().iter().map(|s| s.as_str()).collect();
    sqlx::query_as(&format!(
        "SELECT {MONITOR_COLUMNS}
           FROM telemetry.monitors
          WHERE project_id = $1
          ORDER BY array_position($2::text[], state), lower(name), slug"
    ))
    .bind(project_id)
    .bind(order)
    .fetch_all(tx.conn())
    .await
}

/// One of the project's monitors, by its slug.
pub async fn monitor<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    slug: &str,
) -> sqlx::Result<Option<MonitorRow>> {
    sqlx::query_as(&format!(
        "SELECT {MONITOR_COLUMNS} FROM telemetry.monitors WHERE project_id = $1 AND slug = $2"
    ))
    .bind(project_id)
    .bind(slug)
    .fetch_optional(tx.conn())
    .await
}

/// How many monitors the organization holds across its projects: what its
/// plan's ceiling is held to. Read in a scope that names the organization,
/// which the `organization_read` policy admits.
pub async fn monitor_count(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    organization_id: &OrganizationId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM telemetry.monitors WHERE organization_id = $1")
        .bind(organization_id)
        .fetch_one(tx.conn())
        .await
}

/// How many of the project's monitors its first pings made and nobody has
/// set up yet — in `new` — which a ping that would make one more is held
/// to.
pub async fn new_monitor_count<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.monitors WHERE project_id = $1 AND state = $2",
    )
    .bind(project_id)
    .bind(MonitorState::New)
    .fetch_one(tx.conn())
    .await
}

/// A new monitor of the project, in `state`. Its slug or its agent taken
/// already is the unique index's refusal, `monitors_project_slug_idx` or
/// `monitors_project_agent_idx`.
pub async fn insert_monitor(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    id: Uuid,
    state: MonitorState,
    monitor: MonitorWrite<'_>,
) -> sqlx::Result<MonitorRow> {
    sqlx::query_as(&format!(
        "INSERT INTO telemetry.monitors (id, project_id, organization_id, slug, name, agent, \
             schedule, interval_s, timezone, grace_s, expected_duration_s, stall_after_s, \
             max_steps, max_cost_usd, state, next_due_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
         RETURNING {MONITOR_COLUMNS}"
    ))
    .bind(id)
    .bind(project_id)
    .bind(organization_id)
    .bind(monitor.slug)
    .bind(monitor.name)
    .bind(monitor.agent)
    .bind(monitor.schedule)
    .bind(monitor.interval_s)
    .bind(monitor.timezone)
    .bind(monitor.grace_s)
    .bind(monitor.expected_duration_s)
    .bind(monitor.stall_after_s)
    .bind(monitor.max_steps)
    .bind(monitor.max_cost_usd)
    .bind(state)
    .bind(monitor.next_due_at)
    .fetch_one(tx.conn())
    .await
}

/// One of the project's monitors, written over — its definition and, where
/// `state` names one, its state — or `None` where it has no monitor by that
/// slug. The slug itself never changes: every crontab line carries it.
pub async fn update_monitor(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    slug: &str,
    monitor: MonitorWrite<'_>,
    state: Option<MonitorState>,
) -> sqlx::Result<Option<MonitorRow>> {
    sqlx::query_as(&format!(
        "UPDATE telemetry.monitors
            SET name = $3, agent = $4, schedule = $5, interval_s = $6, timezone = $7,
                grace_s = $8, expected_duration_s = $9, stall_after_s = $10, max_steps = $11,
                max_cost_usd = $12, next_due_at = $13,
                state = COALESCE($14::text, state),
                state_changed_at = CASE WHEN $14::text IS NULL OR $14::text = state
                                        THEN state_changed_at ELSE now() END,
                updated_at = now()
          WHERE project_id = $1 AND slug = $2
         RETURNING {MONITOR_COLUMNS}"
    ))
    .bind(project_id)
    .bind(slug)
    .bind(monitor.name)
    .bind(monitor.agent)
    .bind(monitor.schedule)
    .bind(monitor.interval_s)
    .bind(monitor.timezone)
    .bind(monitor.grace_s)
    .bind(monitor.expected_duration_s)
    .bind(monitor.stall_after_s)
    .bind(monitor.max_steps)
    .bind(monitor.max_cost_usd)
    .bind(monitor.next_due_at)
    .bind(state)
    .fetch_optional(tx.conn())
    .await
}

/// One of the project's monitors put in `state`, with when its next run is
/// due under it — none while it is paused — or `None` where it has no
/// monitor by that slug.
pub async fn set_monitor_state(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    slug: &str,
    state: MonitorState,
    next_due_at: Option<DateTime<Utc>>,
) -> sqlx::Result<Option<MonitorRow>> {
    sqlx::query_as(&format!(
        "UPDATE telemetry.monitors
            SET state = $3, state_changed_at = now(), next_due_at = $4, updated_at = now()
          WHERE project_id = $1 AND slug = $2
         RETURNING {MONITOR_COLUMNS}"
    ))
    .bind(project_id)
    .bind(slug)
    .bind(state)
    .bind(next_due_at)
    .fetch_optional(tx.conn())
    .await
}

/// One of the project's monitors, by its id, after a run of its was heard:
/// put in `state` where one is named, its `state_changed_at` moved only
/// where that differs from the state it was in, and its next due time set
/// to `next_due_at`. `None` where it has no monitor by that id.
pub async fn settle_monitor_run(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    id: Uuid,
    state: Option<MonitorState>,
    next_due_at: Option<DateTime<Utc>>,
) -> sqlx::Result<Option<MonitorRow>> {
    sqlx::query_as(&format!(
        "UPDATE telemetry.monitors
            SET state = COALESCE($3::text, state),
                state_changed_at = CASE WHEN $3::text IS NULL OR $3::text = state
                                        THEN state_changed_at ELSE now() END,
                next_due_at = $4,
                updated_at = now()
          WHERE project_id = $1 AND id = $2
         RETURNING {MONITOR_COLUMNS}"
    ))
    .bind(project_id)
    .bind(id)
    .bind(state)
    .bind(next_due_at)
    .fetch_optional(tx.conn())
    .await
}

/// One of the project's monitors, removed. Answers whether there was one.
pub async fn delete_monitor<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    slug: &str,
) -> sqlx::Result<bool> {
    Ok(
        sqlx::query("DELETE FROM telemetry.monitors WHERE project_id = $1 AND slug = $2")
            .bind(project_id)
            .bind(slug)
            .execute(tx.conn())
            .await?
            .rows_affected()
            > 0,
    )
}

/// A project's settings row and its monitors, ping keys, open runs and
/// outbox rows, handed to the organization that now holds the project.
/// Answers whether there was a settings row.
pub async fn move_project(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
) -> sqlx::Result<bool> {
    for table in ORGANIZATION_TABLES {
        sqlx::query(&format!(
            "UPDATE {table} SET organization_id = $2 WHERE project_id = $1"
        ))
        .bind(project_id)
        .bind(organization_id)
        .execute(tx.conn())
        .await?;
    }
    Ok(sqlx::query(
        "UPDATE telemetry.project_settings SET organization_id = $2 WHERE project_id = $1",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(tx.conn())
    .await?
    .rows_affected()
        > 0)
}

/// A project's hashing key as its row keeps it: sealed under a data key
/// wrapped by `TELEMETRY_KEK`, with the key version it was wrapped under.
#[derive(Debug, Clone)]
pub struct SealedHashingKey {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub wrapped_dek: Vec<u8>,
    pub key_version: i16,
}

/// What a project's settings say of its user ids.
#[derive(Debug, Clone)]
pub struct HashingSettings {
    /// Hashed before they are a row, or kept as sent.
    pub user_id_hashed: bool,
    /// The key that hashes them, once one has been made.
    pub key: Option<SealedHashingKey>,
}

type HashingRow = (
    bool,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<i16>,
);

/// A project's hashing settings, or `None` where it has no row yet.
pub async fn hashing_settings<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<HashingSettings>> {
    let row: Option<HashingRow> = sqlx::query_as(
        "SELECT user_id_hashed, hashing_key_ciphertext, hashing_key_nonce,
                hashing_key_wrapped_dek, hashing_key_version
           FROM telemetry.project_settings
          WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    Ok(row.map(
        |(user_id_hashed, ciphertext, nonce, wrapped_dek, key_version)| HashingSettings {
            user_id_hashed,
            // The CHECK keeps the four together; a row with some of them is
            // one no write of ours left, read as a row with none.
            key: match (ciphertext, nonce, wrapped_dek, key_version) {
                (Some(ciphertext), Some(nonce), Some(wrapped_dek), Some(key_version)) => {
                    Some(SealedHashingKey {
                        ciphertext,
                        nonce,
                        wrapped_dek,
                        key_version,
                    })
                }
                _ => None,
            },
        },
    ))
}

/// A project's row, made where it has none and filed under
/// `organization_id`, the organization auth says holds the project now — so
/// the purges find every project with spans, and a row a transfer's failed
/// settling left under another organization is re-filed. Its mode and its
/// key are left as they are.
pub async fn ensure_settings(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id)
         VALUES ($1, $2)
         ON CONFLICT (project_id) DO UPDATE
            SET organization_id = EXCLUDED.organization_id",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The project's hashing key, set where its row has none. The first writer's
/// stays and a second's is dropped, so two exports racing to make the
/// project's key leave one. Answers whether this one was kept.
pub async fn set_hashing_key_if_none(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    key: &SealedHashingKey,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE telemetry.project_settings
            SET hashing_key_ciphertext = $2, hashing_key_nonce = $3,
                hashing_key_wrapped_dek = $4, hashing_key_version = $5
          WHERE project_id = $1 AND hashing_key_ciphertext IS NULL",
    )
    .bind(project_id)
    .bind(&key.ciphertext)
    .bind(&key.nonce)
    .bind(&key.wrapped_dek)
    .bind(key.key_version)
    .execute(tx.conn())
    .await?
    .rows_affected()
        > 0)
}

// ── Ping keys ────────────────────────────────────────────────────────────────

/// A ping key as its row keeps it; the key itself is kept nowhere.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PingKeyRow {
    pub id: Uuid,
    /// What the list shows of it: the prefix and the first characters after.
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    /// When a ping last came under it, to the minute.
    pub last_ping_at: Option<DateTime<Utc>>,
    /// When it stopped opening the lane, by a person or by its rotation.
    pub retired_at: Option<DateTime<Utc>>,
}

/// The columns a [`PingKeyRow`] is read from, in its order.
const PING_KEY_COLUMNS: &str = "id, prefix, created_at, last_ping_at, retired_at";

/// The project's ping keys: the live ones first, each set newest first.
pub async fn ping_keys<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<PingKeyRow>> {
    sqlx::query_as(&format!(
        "SELECT {PING_KEY_COLUMNS}
           FROM telemetry.ping_keys
          WHERE project_id = $1
          ORDER BY (retired_at IS NOT NULL), created_at DESC"
    ))
    .bind(project_id)
    .fetch_all(tx.conn())
    .await
}

/// One of the project's ping keys, by its id, live or retired.
pub async fn ping_key<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<PingKeyRow>> {
    sqlx::query_as(&format!(
        "SELECT {PING_KEY_COLUMNS} FROM telemetry.ping_keys WHERE project_id = $1 AND id = $2"
    ))
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await
}

/// How many of the project's ping keys are live: what a minting is held to.
pub async fn live_ping_key_count<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.ping_keys WHERE project_id = $1 AND retired_at IS NULL",
    )
    .bind(project_id)
    .fetch_one(tx.conn())
    .await
}

/// A new ping key of the project: its hash and what the list shows of it.
pub async fn insert_ping_key(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    id: Uuid,
    key_hash: &str,
    prefix: &str,
) -> sqlx::Result<PingKeyRow> {
    sqlx::query_as(&format!(
        "INSERT INTO telemetry.ping_keys (id, project_id, organization_id, key_hash, prefix)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING {PING_KEY_COLUMNS}"
    ))
    .bind(id)
    .bind(project_id)
    .bind(organization_id)
    .bind(key_hash)
    .bind(prefix)
    .fetch_one(tx.conn())
    .await
}

/// One of the project's live ping keys, retired now; `None` where the
/// project has no live key by that id, so a second retirement changes
/// nothing and records nothing.
pub async fn retire_ping_key(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<PingKeyRow>> {
    sqlx::query_as(&format!(
        "UPDATE telemetry.ping_keys
            SET retired_at = now()
          WHERE project_id = $1 AND id = $2 AND retired_at IS NULL
         RETURNING {PING_KEY_COLUMNS}"
    ))
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await
}

/// A ping came under one of the project's keys: its `last_ping_at` moved,
/// at most once a minute, so a busy key is not a write per ping.
pub async fn touch_ping_key<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE telemetry.ping_keys
            SET last_ping_at = now()
          WHERE project_id = $1 AND id = $2
            AND (last_ping_at IS NULL OR last_ping_at < now() - interval '1 minute')",
    )
    .bind(project_id)
    .bind(id)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// What the lane learns of a key a ping presents, resolved outside any
/// tenant as auth resolves an API key: whose it is, and what the list and
/// the log call it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ResolvedPingKey {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub organization_id: OrganizationId,
    pub prefix: String,
}

/// The live key with `key_hash`, or `None`: a key the table does not hold,
/// or holds retired, opens nothing.
pub async fn resolve_ping_key(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    key_hash: &str,
) -> sqlx::Result<Option<ResolvedPingKey>> {
    sqlx::query_as(
        "SELECT id, project_id, organization_id, prefix
           FROM telemetry.ping_keys
          WHERE key_hash = $1 AND retired_at IS NULL",
    )
    .bind(key_hash)
    .fetch_optional(tx.conn())
    .await
}

/// The live keys a rotation retires: each `replaced_after_s` past its last
/// ping — or its minting, where no ping ever came — with a newer live key
/// of its project standing, so the crontab lines have had their week to
/// move over. Up to `limit` of them, across every tenant.
pub async fn stale_ping_keys(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    replaced_after_s: i64,
    limit: i64,
) -> sqlx::Result<Vec<ResolvedPingKey>> {
    sqlx::query_as(
        "SELECT k.id, k.project_id, k.organization_id, k.prefix
           FROM telemetry.ping_keys k
          WHERE k.retired_at IS NULL
            AND coalesce(k.last_ping_at, k.created_at) < now() - make_interval(secs => $1)
            AND EXISTS (SELECT 1 FROM telemetry.ping_keys n
                         WHERE n.project_id = k.project_id
                           AND n.retired_at IS NULL
                           AND n.created_at > k.created_at)
          LIMIT $2",
    )
    .bind(replaced_after_s)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

// ── Open runs ────────────────────────────────────────────────────────────────

/// A run still going, as its row keeps it: what the ticker reads to see one
/// gone quiet or past a bound, and what its finished span is built from.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OpenRunRow {
    pub id: Uuid,
    pub monitor_id: Uuid,
    pub agent: String,
    /// `ping` for a run reported by pings, `ingest` for one an SDK sends.
    pub source: String,
    pub ping_key_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub last_heard_at: DateTime<Utc>,
    /// When the run is over whatever it says: its ceiling, past which the
    /// ticker ends it as `error`.
    pub ends_by: DateTime<Utc>,
    pub steps: i32,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub stall_after_s: Option<i32>,
    pub max_steps: Option<i32>,
    pub max_cost_usd: Option<f64>,
}

/// The columns an [`OpenRunRow`] is read from, in its order.
const OPEN_RUN_COLUMNS: &str = "id, monitor_id, agent, source, ping_key_id, started_at, \
     last_heard_at, ends_by, steps, input_tokens, output_tokens, cost_usd, stall_after_s, \
     max_steps, max_cost_usd";

/// What opening a run writes.
#[derive(Debug, Clone, Copy)]
pub struct OpenRunWrite<'a> {
    pub monitor_id: Uuid,
    pub agent: &'a str,
    pub source: &'a str,
    pub ping_key_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub ends_by: DateTime<Utc>,
    pub steps: i32,
    pub cost_usd: f64,
    pub stall_after_s: Option<i32>,
    pub max_steps: Option<i32>,
    pub max_cost_usd: Option<f64>,
}

/// One of the project's open runs, by its id.
pub async fn open_run<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<OpenRunRow>> {
    sqlx::query_as(&format!(
        "SELECT {OPEN_RUN_COLUMNS} FROM telemetry.open_runs WHERE project_id = $1 AND id = $2"
    ))
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await
}

/// The monitor's newest open run: what a ping naming no run means.
pub async fn newest_open_run<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    monitor_id: Uuid,
) -> sqlx::Result<Option<OpenRunRow>> {
    sqlx::query_as(&format!(
        "SELECT {OPEN_RUN_COLUMNS}
           FROM telemetry.open_runs
          WHERE project_id = $1 AND monitor_id = $2
          ORDER BY started_at DESC, id
          LIMIT 1"
    ))
    .bind(project_id)
    .bind(monitor_id)
    .fetch_optional(tx.conn())
    .await
}

/// A run opened, under `id`, which is its own within the project: one open
/// already under that id in the project is the primary key's refusal,
/// `open_runs_pkey`, and one in another project is nothing to this one,
/// since the key is the project's and the id's.
pub async fn insert_open_run(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    id: Uuid,
    run: OpenRunWrite<'_>,
) -> sqlx::Result<OpenRunRow> {
    sqlx::query_as(&format!(
        "INSERT INTO telemetry.open_runs (id, project_id, organization_id, monitor_id, agent, \
             source, ping_key_id, started_at, last_heard_at, ends_by, steps, cost_usd, \
             stall_after_s, max_steps, max_cost_usd)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9, $10, $11, $12, $13, $14)
         RETURNING {OPEN_RUN_COLUMNS}"
    ))
    .bind(id)
    .bind(project_id)
    .bind(organization_id)
    .bind(run.monitor_id)
    .bind(run.agent)
    .bind(run.source)
    .bind(run.ping_key_id)
    .bind(run.started_at)
    .bind(run.ends_by)
    .bind(run.steps)
    .bind(run.cost_usd)
    .bind(run.stall_after_s)
    .bind(run.max_steps)
    .bind(run.max_cost_usd)
    .fetch_one(tx.conn())
    .await
}

/// One of the project's open runs heard at `heard_at`, its stall timeout
/// starting again, with its steps and spend so far where a checkpoint
/// reported them; `None` where the project has no open run by that id
/// under `monitor_id`, the monitor it was read under, so a run another
/// monitor opened under the id once this one ended is never taken for it.
pub async fn hear_open_run<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    monitor_id: Uuid,
    heard_at: DateTime<Utc>,
    steps: Option<i32>,
    cost_usd: Option<f64>,
) -> sqlx::Result<Option<OpenRunRow>> {
    sqlx::query_as(&format!(
        "UPDATE telemetry.open_runs
            SET last_heard_at = $4,
                steps = COALESCE($5, steps),
                cost_usd = COALESCE($6, cost_usd)
          WHERE project_id = $1 AND id = $2 AND monitor_id = $3
         RETURNING {OPEN_RUN_COLUMNS}"
    ))
    .bind(project_id)
    .bind(id)
    .bind(monitor_id)
    .bind(heard_at)
    .bind(steps)
    .bind(cost_usd)
    .fetch_optional(tx.conn())
    .await
}

/// One of the project's open runs, ended: its row gone, and answered as it
/// was; `None` where the project has no open run by that id.
pub async fn close_open_run<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<OpenRunRow>> {
    sqlx::query_as(&format!(
        "DELETE FROM telemetry.open_runs
          WHERE project_id = $1 AND id = $2
         RETURNING {OPEN_RUN_COLUMNS}"
    ))
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await
}

/// Every open run of the monitor's, ended: their rows gone, and answered as
/// they were, in no order. What a `/start` naming no run finds open, and
/// what an end ping naming none finds beside the run it means: the one the
/// ping means and runs that never said they ended.
pub async fn close_open_runs<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    monitor_id: Uuid,
) -> sqlx::Result<Vec<OpenRunRow>> {
    sqlx::query_as(&format!(
        "DELETE FROM telemetry.open_runs
          WHERE project_id = $1 AND monitor_id = $2
         RETURNING {OPEN_RUN_COLUMNS}"
    ))
    .bind(project_id)
    .bind(monitor_id)
    .fetch_all(tx.conn())
    .await
}

// ── The outbox ───────────────────────────────────────────────────────────────

/// A row handed on: written in the project's own scope, leased and landed
/// by the loop across every tenant.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OutboxRow {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub organization_id: OrganizationId,
    /// `span` or `notice`.
    pub kind: String,
    pub payload: serde_json::Value,
    /// How many times it has been leased, this time counted.
    pub attempts: i32,
}

/// The columns an [`OutboxRow`] is read from, in its order, as the lease
/// names them: on its own row, since the candidates it joins carry an `id`
/// too.
const OUTBOX_LEASED_COLUMNS: &str =
    "o.id, o.project_id, o.organization_id, o.kind, o.payload, o.attempts";

/// A row put in the outbox, in the transaction of what it hands on.
pub async fn enqueue_outbox(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    id: Uuid,
    kind: &str,
    payload: &serde_json::Value,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO telemetry.outbox (id, project_id, organization_id, kind, payload)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(project_id)
    .bind(organization_id)
    .bind(kind)
    .bind(payload)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Lease up to `limit` rows not leased already and not past `max_attempts`,
/// `SKIP LOCKED`, each held for `lease_secs`. No `ORDER BY`, as the delivery
/// loop takes its rows: the outbox is empty but for seconds at a time, and
/// a sort of a backlog would cost more than the order is worth.
pub async fn lease_outbox(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    limit: i64,
    lease_secs: i64,
    max_attempts: i32,
) -> sqlx::Result<Vec<OutboxRow>> {
    sqlx::query_as(&format!(
        "WITH candidate AS (
            SELECT o.id FROM telemetry.outbox o
             WHERE (o.lease_until IS NULL OR o.lease_until < now())
               AND o.attempts < $3
             LIMIT $1
             FOR UPDATE SKIP LOCKED
         )
         UPDATE telemetry.outbox o
            SET attempts = o.attempts + 1,
                lease_until = now() + make_interval(secs => $2::bigint)
           FROM candidate c
          WHERE o.id = c.id
      RETURNING {OUTBOX_LEASED_COLUMNS}"
    ))
    .bind(limit)
    .bind(lease_secs)
    .bind(max_attempts)
    .fetch_all(tx.conn())
    .await
}

/// Rows that landed, gone. Answers how many went.
pub async fn delete_outbox(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    ids: &[Uuid],
) -> sqlx::Result<u64> {
    Ok(
        sqlx::query("DELETE FROM telemetry.outbox WHERE id = ANY($1)")
            .bind(ids)
            .execute(tx.conn())
            .await?
            .rows_affected(),
    )
}

/// A row that did not land, held for `secs` before the loop takes it again.
pub async fn hold_outbox(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    id: Uuid,
    secs: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE telemetry.outbox
            SET lease_until = now() + make_interval(secs => $2::bigint)
          WHERE id = $1",
    )
    .bind(id)
    .bind(secs)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Rows the store was at fault for, handed back untouched: the try returned
/// and the lease dropped, so the next tick takes them again. The delivery
/// loop's posture for a KEK it cannot reach, and the reason an outage of
/// ours cannot spend a row's tries.
pub async fn unlease_outbox(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    ids: &[Uuid],
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE telemetry.outbox
            SET attempts = GREATEST(attempts - 1, 0),
                lease_until = NULL
          WHERE id = ANY($1)",
    )
    .bind(ids)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Every row past `max_attempts` given its tries back, for the loop to take
/// again: an operator's, once what refused the rows is mended. Answers how
/// many were released.
pub async fn release_outbox(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    max_attempts: i32,
) -> sqlx::Result<u64> {
    Ok(sqlx::query(
        "UPDATE telemetry.outbox
            SET attempts = 0,
                lease_until = NULL
          WHERE attempts >= $1",
    )
    .bind(max_attempts)
    .execute(tx.conn())
    .await?
    .rows_affected())
}

/// What the outbox holds: the rows still to land, those past `max_attempts`,
/// which stay until an operator releases them, and how old the oldest to
/// land is, in seconds.
pub async fn outbox_depth(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    max_attempts: i32,
) -> sqlx::Result<(i64, i64, Option<i64>)> {
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE attempts < $1),
                count(*) FILTER (WHERE attempts >= $1),
                CAST(extract(epoch FROM now() - min(created_at) FILTER (WHERE attempts < $1)) AS BIGINT)
           FROM telemetry.outbox",
    )
    .bind(max_attempts)
    .fetch_one(tx.conn())
    .await
}
