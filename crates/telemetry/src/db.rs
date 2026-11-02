//! The telemetry module's Postgres: each project's settings, its saved views
//! of the trace list and layouts of a session, and the content it keeps.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{
    HasProject, Lane, Maintenance, MaintenanceLane, Organization, ProjectAndOrganization, Scoped,
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

/// A project's own retention, in days: none where it keeps the longest sold,
/// or has no settings row.
pub async fn retention_days<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<i16>> {
    Ok(sqlx::query_scalar(
        "SELECT retention_days FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?
    .flatten())
}

/// A project's retention, set, as [`set_content_mode`] sets its mode: its row
/// made where it has none and filed under the scope's organization; `None`
/// for the longest sold. Answers what it held before.
pub async fn set_retention_days(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    days: Option<i16>,
) -> sqlx::Result<Option<i16>> {
    let before = retention_days(tx, project_id).await?;
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id, retention_days)
         VALUES ($1, $2, $3)
         ON CONFLICT (project_id) DO UPDATE
            SET organization_id = EXCLUDED.organization_id,
                retention_days  = EXCLUDED.retention_days",
    )
    .bind(project_id)
    .bind(organization_id)
    .bind(days)
    .execute(tx.conn())
    .await?;
    Ok(before)
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
/// (empty for the first page), each with the organization it is filed under,
/// for the purge sweep: the row stays until its project's purge finishes.
pub async fn settings_after(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    after: &str,
    limit: i64,
) -> sqlx::Result<Vec<(ProjectId, OrganizationId)>> {
    sqlx::query_as(
        "SELECT project_id, organization_id FROM telemetry.project_settings
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
    let mutes = sqlx::query("DELETE FROM telemetry.mutes WHERE project_id = ANY($1)")
        .bind(&projects)
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
    let prices = sqlx::query("DELETE FROM telemetry.project_prices WHERE project_id = ANY($1)")
        .bind(&projects)
        .execute(tx.conn())
        .await?
        .rows_affected();
    let content = purge_project_content(tx, &projects).await?;
    let held = sqlx::query("DELETE FROM telemetry.content_held WHERE organization_id = $1")
        .bind(organization_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    let settings = sqlx::query("DELETE FROM telemetry.project_settings WHERE organization_id = $1")
        .bind(organization_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    Ok(rules + mutes + views + prices + content + held + settings)
}

/// How long a purge waits for any lock: one left queued for a project's lock
/// turns its exports away meanwhile.
const PURGE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Holds each project alone, in the order given, as a move does, so no
/// export files a settings row of the project beside the purge.
async fn hold_alone(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_ids: &[ProjectId],
) -> sqlx::Result<()> {
    telmoni_shared::db::set_lock_timeout(tx.conn(), PURGE_LOCK_WAIT).await?;
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
    let mutes = sqlx::query("DELETE FROM telemetry.mutes WHERE project_id = $1")
        .bind(project_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    let views = sqlx::query("DELETE FROM telemetry.trace_views WHERE project_id = $1")
        .bind(project_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    let prices = sqlx::query("DELETE FROM telemetry.project_prices WHERE project_id = $1")
        .bind(project_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    let content = purge_project_content(tx, std::slice::from_ref(project_id)).await?;
    let settings = sqlx::query("DELETE FROM telemetry.project_settings WHERE project_id = $1")
        .bind(project_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    Ok(rules + mutes + views + prices + content + settings)
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

/// A project's settings row and its alerts, handed to the organization that
/// now holds the project, its alerts counted against that one's plan from
/// then on. Answers whether there was a settings row.
pub async fn move_project(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
) -> sqlx::Result<bool> {
    // Each lease ended, so an evaluation in flight files no incident under
    // the organization the project left.
    sqlx::query(
        "UPDATE telemetry.rules SET organization_id = $2, lease_id = NULL, leased_until = NULL
          WHERE project_id = $1",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(tx.conn())
    .await?;
    sqlx::query("UPDATE telemetry.incidents SET organization_id = $2 WHERE project_id = $1")
        .bind(project_id)
        .bind(organization_id)
        .execute(tx.conn())
        .await?;
    // The content's bytes follow it to the organization that now holds it,
    // its settings row locked first, as `delete_content` locks it.
    sqlx::query("SELECT 1 FROM telemetry.project_settings WHERE project_id = $1 FOR UPDATE")
        .bind(project_id)
        .execute(tx.conn())
        .await?;
    sqlx::query(
        "WITH project AS (
             SELECT s.organization_id AS left_org,
                    (SELECT coalesce(sum(bytes), 0) FROM telemetry.plain_content
                      WHERE project_id = $1)
                    + (SELECT coalesce(sum(bytes), 0) FROM telemetry.media
                        WHERE project_id = $1) AS bytes
               FROM telemetry.project_settings s
              WHERE s.project_id = $1 AND s.organization_id <> $2),
         taken AS (
             UPDATE telemetry.content_held h SET bytes = greatest(h.bytes - p.bytes, 0)
               FROM project p WHERE h.organization_id = p.left_org AND p.bytes > 0
             RETURNING 1)
         INSERT INTO telemetry.content_held (organization_id, bytes)
         SELECT $2, bytes FROM project WHERE bytes > 0
         ON CONFLICT (organization_id) DO UPDATE
            SET bytes = telemetry.content_held.bytes + EXCLUDED.bytes",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(tx.conn())
    .await?;
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

type HashingRow = (
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

/// The key a project's user ids are hashed under, or `None` where it has no
/// row or no key yet.
pub async fn hashing_key<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<SealedHashingKey>> {
    let row: Option<HashingRow> = sqlx::query_as(
        "SELECT hashing_key_ciphertext, hashing_key_nonce,
                hashing_key_wrapped_dek, hashing_key_version
           FROM telemetry.project_settings
          WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    // The CHECK keeps the four together; a row with some of them is one no
    // write of ours left, read as a row with none.
    Ok(
        row.and_then(|(ciphertext, nonce, wrapped_dek, key_version)| {
            Some(SealedHashingKey {
                ciphertext: ciphertext?,
                nonce: nonce?,
                wrapped_dek: wrapped_dek?,
                key_version: key_version?,
            })
        }),
    )
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

/// A project's content key as its row keeps it: a data key wrapped by
/// `CONTENT_KEK`, with the key version it was wrapped under, and the id each
/// row sealed under it carries.
#[derive(Debug, Clone)]
pub struct WrappedContentKey {
    pub id: Uuid,
    pub wrapped_dek: Vec<u8>,
    pub key_version: i16,
}

/// A settings row's content key columns, each there or not with the others.
type ContentKeyRow = (Option<Uuid>, Option<Vec<u8>>, Option<i16>);

/// The key a project's content is sealed under, or `None` where it has no
/// row or no key yet.
pub async fn content_key<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<WrappedContentKey>> {
    let row: Option<ContentKeyRow> = sqlx::query_as(
        "SELECT content_key_id, content_key_wrapped_dek, content_key_version
           FROM telemetry.project_settings
          WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    Ok(row.and_then(|(id, wrapped_dek, key_version)| {
        Some(WrappedContentKey {
            id: id?,
            wrapped_dek: wrapped_dek?,
            key_version: key_version?,
        })
    }))
}

/// The project's content key, set where its row has none and its mode is
/// `on`: the first writer's stays, as [`set_hashing_key_if_none`]'s does, and
/// a project switched `off` meanwhile gets no key for the export in flight.
pub async fn set_content_key_if_none(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    key: &WrappedContentKey,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE telemetry.project_settings
            SET content_key_id = $4, content_key_wrapped_dek = $2, content_key_version = $3
          WHERE project_id = $1 AND content_key_wrapped_dek IS NULL AND content_mode = 'on'",
    )
    .bind(project_id)
    .bind(&key.wrapped_dek)
    .bind(key.key_version)
    .bind(key.id)
    .execute(tx.conn())
    .await?
    .rows_affected()
        > 0)
}

/// The project's content key deleted, so nothing reads what it sealed from
/// then, but a backup taken before: the id its rows are purged by, or none
/// where there was no key.
pub async fn delete_content_key(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar(
        "WITH held AS (
             SELECT content_key_id FROM telemetry.project_settings
              WHERE project_id = $1 AND content_key_wrapped_dek IS NOT NULL FOR UPDATE)
         UPDATE telemetry.project_settings s
            SET content_key_id = NULL, content_key_wrapped_dek = NULL, content_key_version = NULL
           FROM held WHERE s.project_id = $1
      RETURNING held.content_key_id",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await
}

/// One content field, sealed, as `plain_content` keeps it.
#[derive(Debug, Default)]
pub struct SealedFields {
    pub run_ids: Vec<Uuid>,
    pub trace_ids: Vec<String>,
    pub span_ids: Vec<String>,
    pub fields: Vec<String>,
    pub ciphertexts: Vec<Vec<u8>>,
    pub nonces: Vec<Vec<u8>>,
    pub bytes: Vec<i32>,
    /// Each field's hashed words ([`crate::content::words_of`]), as a JSON
    /// array, since a column of arrays of differing lengths binds no other way.
    pub words: Vec<String>,
    /// Each field's hashed pairs of neighbouring words, the same way.
    pub pairs: Vec<String>,
}

/// Every organization with a project filed here, and those projects, across
/// tenants: what each night's pass holds to its plan.
pub async fn organizations_filed(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
) -> sqlx::Result<Vec<(OrganizationId, Vec<ProjectId>)>> {
    sqlx::query_as(
        "SELECT organization_id, array_agg(project_id ORDER BY project_id)
           FROM telemetry.project_settings
          GROUP BY organization_id ORDER BY organization_id",
    )
    .fetch_all(tx.conn())
    .await
}

/// The projects filed under the organization, across tenants.
pub async fn projects_filed(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    organization_id: &OrganizationId,
) -> sqlx::Result<Vec<ProjectId>> {
    sqlx::query_scalar(
        "SELECT project_id FROM telemetry.project_settings
          WHERE organization_id = $1 ORDER BY project_id",
    )
    .bind(organization_id)
    .fetch_all(tx.conn())
    .await
}

/// When the oldest content of `projects` landed, across tenants; `None`
/// where they hold none.
pub async fn oldest_content(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    projects: &[ProjectId],
) -> sqlx::Result<Option<DateTime<Utc>>> {
    sqlx::query_scalar(
        "SELECT min(received_at) FROM telemetry.plain_content WHERE project_id = ANY($1)",
    )
    .bind(projects)
    .fetch_one(tx.conn())
    .await
}

/// The organization a project's settings row is filed under, if it has one.
pub async fn filed_under(
    tx: &mut Scoped<'_, impl HasProject>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<OrganizationId>> {
    sqlx::query_scalar(
        "SELECT organization_id FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await
}

/// The sealed bytes of content the organization's projects hold.
pub async fn content_held(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
) -> sqlx::Result<u64> {
    let held: Option<i64> =
        sqlx::query_scalar("SELECT bytes FROM telemetry.content_held WHERE organization_id = $1")
            .bind(organization_id)
            .fetch_optional(tx.conn())
            .await?;
    Ok(held.map_or(0, i64::unsigned_abs))
}

/// Content kept, its bytes added to the organization's count. A field
/// already kept, an export's retry, is kept once and counted once.
pub async fn keep_content(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    received_at: DateTime<Utc>,
    key_id: Uuid,
    sealed: &SealedFields,
) -> sqlx::Result<()> {
    sqlx::query(
        "WITH kept AS (
             INSERT INTO telemetry.plain_content
                 (project_id, run_id, trace_id, span_id, field, received_at, ciphertext, nonce,
                  bytes, words, pairs, key_id)
             SELECT $1, r, t, s, f, $3, c, n, b,
                    ARRAY(SELECT jsonb_array_elements_text(w::jsonb)::bigint),
                    ARRAY(SELECT jsonb_array_elements_text(p::jsonb)::bigint),
                    $13
               FROM UNNEST($10::uuid[], $4::text[], $5::text[], $6::text[], $7::bytea[],
                           $8::bytea[], $9::int[], $11::text[], $12::text[])
                 AS u(r, t, s, f, c, n, b, w, p)
             ON CONFLICT DO NOTHING
             RETURNING bytes)
         INSERT INTO telemetry.content_held (organization_id, bytes)
         SELECT $2, coalesce(sum(bytes), 0) FROM kept
         ON CONFLICT (organization_id) DO UPDATE
            SET bytes = telemetry.content_held.bytes + EXCLUDED.bytes",
    )
    .bind(project_id)
    .bind(organization_id)
    .bind(received_at)
    .bind(&sealed.trace_ids)
    .bind(&sealed.span_ids)
    .bind(&sealed.fields)
    .bind(&sealed.ciphertexts)
    .bind(&sealed.nonces)
    .bind(&sealed.bytes)
    .bind(&sealed.run_ids)
    .bind(&sealed.words)
    .bind(&sealed.pairs)
    .bind(key_id)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The run of each field of the project's content holding every one of
/// `words` and of `pairs`, hashed, newest first, up to `limit` fields; nothing
/// sealed. The words' index finds the fields, each one's pairs checked on it.
/// Only fields under the project's key now, as [`read_content`] reads: none
/// once it is deleted.
pub(crate) async fn search_content<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    (words, pairs): (&[i64], &[i64]),
    limit: i64,
) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar(
        "SELECT run_id
           FROM telemetry.plain_content
          WHERE project_id = $1 AND words @> $2 AND pairs @> $4
            AND key_id = (SELECT content_key_id FROM telemetry.project_settings
                           WHERE project_id = $1)
          ORDER BY received_at DESC
          LIMIT $3",
    )
    .bind(project_id)
    .bind(words)
    .bind(limit)
    .bind(pairs)
    .fetch_all(tx.conn())
    .await
}

/// One content field as kept: its run, trace, span and name, sealed.
pub(crate) type KeptField = (Uuid, String, String, String, Vec<u8>, Vec<u8>);

/// Every field the project keeps for the runs, or for those of their spans
/// `spans` names, sealed, in trace, span and name order: the crate's own,
/// read only by [`crate::content::open`], which files each read. Only fields
/// under the project's key now: none once it is deleted
/// ([`delete_content_key`]), whatever rows its purge has yet to take, and
/// none a deleted key sealed once a later key is made.
pub(crate) async fn read_content(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    runs: &[Uuid],
    spans: Option<&[String]>,
) -> sqlx::Result<Vec<KeptField>> {
    sqlx::query_as(
        "SELECT run_id, trace_id, span_id, field, ciphertext, nonce
           FROM telemetry.plain_content
          WHERE project_id = $1 AND run_id = ANY($2)
            AND ($3::text[] IS NULL OR span_id = ANY($3))
            AND key_id = (SELECT content_key_id FROM telemetry.project_settings
                           WHERE project_id = $1)
          ORDER BY trace_id, span_id, field",
    )
    .bind(project_id)
    .bind(runs)
    .bind(spans)
    .fetch_all(tx.conn())
    .await
}

/// Up to `limit` of the projects' content rows deleted, those received before
/// `before` where it is given, their bytes taken off the count of the
/// organization each project's settings row is filed under; answers how many
/// went. ⚠ Each settings row is locked first, in id order, as a transfer locks
/// it: a transfer and a purge each count what the other left, and never the
/// same bytes twice.
pub async fn delete_content(
    conn: &mut sqlx::PgConnection,
    project_ids: &[ProjectId],
    before: Option<DateTime<Utc>>,
    limit: i64,
) -> sqlx::Result<u64> {
    delete_content_of(conn, project_ids, before, limit, None, None).await
}

/// The project's content rows of these runs, whenever their spans landed,
/// deleted and counted off as [`delete_content`] does them: a trace's
/// deletion's, or an end user's erasure's.
pub async fn delete_runs_content(
    conn: &mut sqlx::PgConnection,
    project_id: &ProjectId,
    runs: &[Uuid],
) -> sqlx::Result<u64> {
    delete_content_of(
        conn,
        std::slice::from_ref(project_id),
        None,
        i64::MAX,
        Some(runs),
        None,
    )
    .await
}

/// The project's rows sealed under `key`, a content key deleted at `since`,
/// and the files and names kept before then, deleted and counted off as
/// [`delete_content`] does them: the content switch's purge, which leaves
/// what a key made since sealed.
pub async fn delete_key_content(
    conn: &mut sqlx::PgConnection,
    project_id: &ProjectId,
    key: Uuid,
    since: DateTime<Utc>,
    limit: i64,
) -> sqlx::Result<u64> {
    delete_content_of(
        conn,
        std::slice::from_ref(project_id),
        Some(since),
        limit,
        None,
        Some(key),
    )
    .await
}

async fn delete_content_of(
    conn: &mut sqlx::PgConnection,
    project_ids: &[ProjectId],
    before: Option<DateTime<Utc>>,
    limit: i64,
    runs: Option<&[Uuid]>,
    key: Option<Uuid>,
) -> sqlx::Result<u64> {
    sqlx::query(
        "SELECT 1 FROM telemetry.project_settings
          WHERE project_id = ANY($1) ORDER BY project_id FOR UPDATE",
    )
    .bind(project_ids)
    .execute(&mut *conn)
    .await?;
    let gone: i64 = sqlx::query_scalar(
        "WITH gone AS (
             DELETE FROM telemetry.plain_content
              WHERE ctid IN (SELECT ctid FROM telemetry.plain_content
                              WHERE project_id = ANY($1)
                                AND ($2::timestamptz IS NULL OR received_at < $2)
                                AND ($4::uuid[] IS NULL OR run_id = ANY($4))
                                AND ($5::uuid IS NULL OR key_id = $5)
                              LIMIT $3)
             RETURNING project_id, bytes),
         sums AS (
             SELECT s.organization_id, sum(g.bytes) AS bytes
               FROM gone g JOIN telemetry.project_settings s USING (project_id)
              GROUP BY s.organization_id),
         taken AS (
             UPDATE telemetry.content_held h SET bytes = greatest(h.bytes - sums.bytes, 0)
               FROM sums WHERE h.organization_id = sums.organization_id
             RETURNING 1)
         SELECT count(*) FROM gone",
    )
    .bind(project_ids)
    .bind(before)
    .bind(limit)
    .bind(runs)
    .bind(key)
    .fetch_one(&mut *conn)
    .await?;
    delete_media_of(conn, project_ids, before, runs, key).await?;
    Ok(gone.unsigned_abs())
}

/// How long a file nothing names is kept, as a Postgres interval: an upload
/// comes before the export whose spans name it.
const UNNAMED_MEDIA_KEPT: &str = "1 day";

/// The projects holding a file nothing has named past [`UNNAMED_MEDIA_KEPT`],
/// an upload whose spans never came, which [`delete_media_of`] then takes.
pub(crate) async fn projects_with_unnamed_media(
    pool: &sqlx::PgPool,
) -> sqlx::Result<Vec<ProjectId>> {
    sqlx::query_scalar(
        "SELECT DISTINCT m.project_id FROM telemetry.media m
          WHERE m.created_at < now() - $1::interval
            AND NOT EXISTS (SELECT 1 FROM telemetry.media_names n
                             WHERE n.project_id = m.project_id AND n.sha256 = m.sha256)",
    )
    .bind(UNNAMED_MEDIA_KEPT)
    .fetch_all(pool)
    .await
}

/// The names of the files the same content named, and then each file nothing
/// names any more, its bytes taken off as content's are: a file unnamed by
/// this deletion, one left unnamed past [`UNNAMED_MEDIA_KEPT`] (an upload
/// whose spans never came), and every file where the project's whole content
/// goes.
async fn delete_media_of(
    conn: &mut sqlx::PgConnection,
    project_ids: &[ProjectId],
    before: Option<DateTime<Utc>>,
    runs: Option<&[Uuid]>,
    key: Option<Uuid>,
) -> sqlx::Result<()> {
    let unnamed: Vec<String> = sqlx::query_scalar(
        "DELETE FROM telemetry.media_names
          WHERE project_id = ANY($1)
            AND ($2::timestamptz IS NULL OR received_at < $2)
            AND ($3::uuid[] IS NULL OR run_id = ANY($3))
      RETURNING project_id || '/' || sha256",
    )
    .bind(project_ids)
    .bind(before)
    .bind(runs)
    .fetch_all(&mut *conn)
    .await?;
    // A file sealed under a deleted key goes whatever names it: nothing opens it.
    sqlx::query(
        "WITH gone AS (
             DELETE FROM telemetry.media m
              WHERE m.project_id = ANY($1)
                AND (m.key_id = $5
                     OR (NOT EXISTS (SELECT 1 FROM telemetry.media_names n
                                      WHERE n.project_id = m.project_id AND n.sha256 = m.sha256)
                         AND ($2 OR m.project_id || '/' || m.sha256 = ANY($3)
                              OR m.created_at < now() - $4::interval)))
             RETURNING project_id, bytes),
         sums AS (
             SELECT s.organization_id, sum(g.bytes) AS bytes
               FROM gone g JOIN telemetry.project_settings s USING (project_id)
              GROUP BY s.organization_id)
         UPDATE telemetry.content_held h SET bytes = greatest(h.bytes - sums.bytes, 0)
           FROM sums WHERE h.organization_id = sums.organization_id",
    )
    .bind(project_ids)
    .bind(before.is_none() && runs.is_none())
    .bind(&unnamed)
    .bind(UNNAMED_MEDIA_KEPT)
    .bind(key)
    .execute(conn)
    .await?;
    Ok(())
}

/// A file a message carried, sealed, kept once a project: answers whether it
/// was new, its bytes then counted on the organization's row.
pub async fn keep_media(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    (project_id, organization_id): (&ProjectId, &OrganizationId),
    sha256: &str,
    mime_type: &str,
    sealed: (&[u8], &[u8]),
    bytes: i32,
    key_id: Uuid,
) -> sqlx::Result<bool> {
    let kept: i64 = sqlx::query_scalar(
        "WITH kept AS (
             INSERT INTO telemetry.media
                 (project_id, sha256, mime_type, bytes, ciphertext, nonce, key_id)
             VALUES ($1, $3, $4, $5, $6, $7, $8)
             ON CONFLICT DO NOTHING
             RETURNING bytes),
         counted AS (
             INSERT INTO telemetry.content_held (organization_id, bytes)
             SELECT $2, bytes FROM kept
             ON CONFLICT (organization_id) DO UPDATE
                SET bytes = telemetry.content_held.bytes + EXCLUDED.bytes
             RETURNING 1)
         SELECT count(*) FROM kept",
    )
    .bind(project_id)
    .bind(organization_id)
    .bind(sha256)
    .bind(mime_type)
    .bind(bytes)
    .bind(sealed.0)
    .bind(sealed.1)
    .bind(key_id)
    .fetch_one(tx.conn())
    .await?;
    Ok(kept > 0)
}

/// The files each run's kept content names, by hash: what keeps each one.
pub async fn name_media(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    received_at: DateTime<Utc>,
    (hashes, runs): (&[String], &[Uuid]),
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO telemetry.media_names (project_id, sha256, run_id, received_at)
         SELECT $1, h, r, $2 FROM UNNEST($3::text[], $4::uuid[]) AS u(h, r)
         ON CONFLICT DO NOTHING",
    )
    .bind(project_id)
    .bind(received_at)
    .bind(hashes)
    .bind(runs)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// A file the project keeps, sealed: its type and its bytes; only one under
/// the project's key now, as content is read, so none once that is deleted.
pub(crate) async fn read_media(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    sha256: &str,
) -> sqlx::Result<Option<(String, Vec<u8>, Vec<u8>)>> {
    sqlx::query_as(
        "SELECT mime_type, ciphertext, nonce FROM telemetry.media
          WHERE project_id = $1 AND sha256 = $2
            AND key_id = (SELECT content_key_id FROM telemetry.project_settings
                           WHERE project_id = $1)",
    )
    .bind(project_id)
    .bind(sha256)
    .fetch_optional(tx.conn())
    .await
}

/// A project's content deleted, and its bytes taken off its organization's
/// count; run before its settings row goes.
async fn purge_project_content(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_ids: &[ProjectId],
) -> sqlx::Result<u64> {
    // Another exporter's spans still held go with their project too.
    let held = sqlx::query("DELETE FROM telemetry.held_spans WHERE project_id = ANY($1)")
        .bind(project_ids)
        .execute(tx.conn())
        .await?
        .rows_affected();
    Ok(held + delete_content(tx.conn(), project_ids, None, i64::MAX).await?)
}
