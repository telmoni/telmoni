//! The telemetry module's Postgres: each project's settings, and its saved
//! views of the trace list and layouts of a session.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{
    HasProject, Lane, Maintenance, MaintenanceLane, ProjectAndOrganization, Scoped,
};
use telmoni_shared::{ContentMode, OrganizationId, ProjectId, ViewKind};

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

/// A page of the projects with a settings row, in id order after `after`
/// (empty for the first page), for the purge sweep: the row stays until its
/// project's purge finishes.
pub async fn settings_after(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    after: &str,
    limit: i64,
) -> sqlx::Result<Vec<ProjectId>> {
    sqlx::query_scalar(
        "SELECT project_id FROM telemetry.project_settings
          WHERE project_id > $1
          ORDER BY project_id
          LIMIT $2",
    )
    .bind(after)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// Every settings row of an organization's projects, and their saved views,
/// found through those rows before they go, since a view names no
/// organization: each project held alone first, in id order so that two
/// purges agree. ⚠ Each hold is a lock-table entry until commit: an
/// organization of thousands of projects needs `max_locks_per_transaction`
/// raised.
pub async fn purge_organization(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    organization_id: &OrganizationId,
) -> sqlx::Result<u64> {
    let projects = projects_of(tx, organization_id).await?;
    hold_alone(tx, &projects).await?;
    let rules = sqlx::query("DELETE FROM telemetry.rules WHERE organization_id = $1")
        .bind(organization_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
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
    Ok(rules + views + settings)
}

/// How long a purge waits for any lock. Set on the transaction because the
/// Compose superuser has no role-level `lock_timeout`, and a purge left
/// queued for a project's lock turns its exports away meanwhile.
const PURGE_LOCK_WAIT: &str = "2s";

/// Holds each project alone, in the order given, as a move does, so no
/// export files a settings row of the project beside the purge.
async fn hold_alone(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_ids: &[ProjectId],
) -> sqlx::Result<()> {
    sqlx::query("SELECT set_config('lock_timeout', $1, true)")
        .bind(PURGE_LOCK_WAIT)
        .execute(tx.conn())
        .await?;
    for project_id in project_ids {
        project_move::lock_for_move(tx.conn(), project_id).await?;
    }
    Ok(())
}

/// One project's saved views and settings row, the project held alone
/// first, as a move holds it. ⚠ It waits at most `PURGE_LOCK_WAIT`; a purge
/// that times out is left to the sweep.
pub async fn purge_project(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
) -> sqlx::Result<u64> {
    hold_alone(tx, std::slice::from_ref(project_id)).await?;
    let rules = sqlx::query("DELETE FROM telemetry.rules WHERE project_id = $1")
        .bind(project_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
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
    Ok(rules + views + settings)
}

/// A saved view as its row keeps it: a view of the trace list, or a layout
/// of a session's page and a trace's (`kind`). Its query is the JSON the
/// lane checked when it was written; its columns, `TraceColumn`'s words.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ViewRow {
    pub id: Uuid,
    pub kind: ViewKind,
    pub name: String,
    pub query: serde_json::Value,
    pub columns: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The columns a [`ViewRow`] is read from, in its order.
const VIEW_COLUMNS: &str = "id, kind, name, query, columns, created_at, updated_at";

/// What saving a view writes.
#[derive(Debug, Clone, Copy)]
pub struct ViewWrite<'a> {
    pub name: &'a str,
    pub query: &'a serde_json::Value,
    pub columns: &'a [String],
}

/// The project's saved views of one kind, by name.
pub async fn views<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    kind: ViewKind,
) -> sqlx::Result<Vec<ViewRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VIEW_COLUMNS}
           FROM telemetry.trace_views
          WHERE project_id = $1 AND kind = $2
          ORDER BY lower(name), id"
    )))
    .bind(project_id)
    .bind(kind)
    .fetch_all(tx.conn())
    .await
}

/// How many views of one kind the project keeps.
pub async fn view_count<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    kind: ViewKind,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.trace_views WHERE project_id = $1 AND kind = $2",
    )
    .bind(project_id)
    .bind(kind)
    .fetch_one(tx.conn())
    .await
}

/// A new view of the project. Its name taken already by one of its kind, in
/// any case, is the unique index's refusal, `trace_views_project_name_idx`.
pub async fn insert_view(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    id: Uuid,
    kind: ViewKind,
    view: ViewWrite<'_>,
) -> sqlx::Result<ViewRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO telemetry.trace_views (id, project_id, kind, name, query, columns)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING {VIEW_COLUMNS}"
    )))
    .bind(id)
    .bind(project_id)
    .bind(kind)
    .bind(view.name)
    .bind(view.query)
    .bind(view.columns)
    .fetch_one(tx.conn())
    .await
}

/// One of the project's views of `kind`, written over, or `None` where it
/// has no view of that kind by that id: a view never changes its kind.
pub async fn update_view<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    kind: ViewKind,
    view: ViewWrite<'_>,
) -> sqlx::Result<Option<ViewRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE telemetry.trace_views
            SET name = $4, query = $5, columns = $6, updated_at = now()
          WHERE project_id = $1 AND id = $2 AND kind = $3
         RETURNING {VIEW_COLUMNS}"
    )))
    .bind(project_id)
    .bind(id)
    .bind(kind)
    .bind(view.name)
    .bind(view.query)
    .bind(view.columns)
    .fetch_optional(tx.conn())
    .await
}

/// Whether the project keeps a layout by that id, held (`FOR KEY SHARE`)
/// until the transaction ends: a delete of the layout then waits for the
/// settings row to name it, the order its own `SET NULL` takes the two in,
/// so neither waits on the other in a circle.
pub async fn layout_exists<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM telemetry.trace_views
          WHERE project_id = $1 AND id = $2 AND kind = $3
            FOR KEY SHARE",
    )
    .bind(project_id)
    .bind(id)
    .bind(ViewKind::Layout)
    .fetch_optional(tx.conn())
    .await
    .map(|found| found.is_some())
}

/// The saved layout a project's sessions and traces open in, while it names
/// one; `None` for the console's own default, and where the project has no
/// row.
pub async fn session_layout<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<Uuid>> {
    let row: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT session_view_id FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    Ok(row.flatten())
}

/// A project's layout, set: its row made where it has none, and filed under
/// `organization_id`, as [`set_content_mode`] files it.
pub async fn set_session_layout(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    view_id: Option<Uuid>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO telemetry.project_settings
             (project_id, organization_id, session_view_id)
         VALUES ($1, $2, $3)
         ON CONFLICT (project_id) DO UPDATE
            SET organization_id = EXCLUDED.organization_id,
                session_view_id = EXCLUDED.session_view_id",
    )
    .bind(project_id)
    .bind(organization_id)
    .bind(view_id)
    .execute(tx.conn())
    .await?;
    Ok(())
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

/// The tables a project's alerts are filed in under its organization.
const MOVED_ALERTS: [&str; 2] = ["telemetry.rules", "telemetry.incidents"];

/// A project's settings row and its alerts, handed to the organization that
/// now holds the project, its alerts counted against that one's plan from
/// then on. Answers whether there was a settings row.
pub async fn move_project(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
) -> sqlx::Result<bool> {
    for table in MOVED_ALERTS {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET organization_id = $2 WHERE project_id = $1"
        )))
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

/// The organization a project's settings row is filed under, or `None`
/// where it has no row yet; read with the project held in place, so that no
/// move passes meanwhile.
pub async fn settings_organization<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<OrganizationId>> {
    sqlx::query_scalar(
        "SELECT organization_id FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await
}

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
/// `organization_id`, so the purges find every project with spans; its mode
/// and key are left as they are. ⚠ The organization must be one no move has
/// passed since it was read — auth's answer under the transfer's chain
/// lock, or one read with the project held in place: one read before a
/// transfer committed files the row back under the organization that gave
/// the project up.
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
