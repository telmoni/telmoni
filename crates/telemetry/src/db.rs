//! The telemetry module's Postgres: each project's settings, its saved views
//! of the trace list and layouts of a session, and the content it keeps.

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
    Ok(rules + mutes + views + content + held + settings)
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
    let content = purge_project_content(tx, std::slice::from_ref(project_id)).await?;
    let settings = sqlx::query("DELETE FROM telemetry.project_settings WHERE project_id = $1")
        .bind(project_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    Ok(rules + mutes + views + content + settings)
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
/// `TELEMETRY_KEK`, with the key version it was wrapped under.
#[derive(Debug, Clone)]
pub struct WrappedContentKey {
    pub wrapped_dek: Vec<u8>,
    pub key_version: i16,
}

/// The key a project's content is sealed under, or `None` where it has no
/// row or no key yet.
pub async fn content_key<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<WrappedContentKey>> {
    let row: Option<(Option<Vec<u8>>, Option<i16>)> = sqlx::query_as(
        "SELECT content_key_wrapped_dek, content_key_version
           FROM telemetry.project_settings
          WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    Ok(row.and_then(|(wrapped_dek, key_version)| {
        Some(WrappedContentKey {
            wrapped_dek: wrapped_dek?,
            key_version: key_version?,
        })
    }))
}

/// The project's content key, set where its row has none: the first writer's
/// stays, as [`set_hashing_key_if_none`]'s does.
pub async fn set_content_key_if_none(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    key: &WrappedContentKey,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE telemetry.project_settings
            SET content_key_wrapped_dek = $2, content_key_version = $3
          WHERE project_id = $1 AND content_key_wrapped_dek IS NULL",
    )
    .bind(project_id)
    .bind(&key.wrapped_dek)
    .bind(key.key_version)
    .execute(tx.conn())
    .await?
    .rows_affected()
        > 0)
}

/// One content field, sealed, as `plain_content` keeps it.
#[derive(Debug, Default)]
pub struct SealedFields {
    pub trace_ids: Vec<String>,
    pub span_ids: Vec<String>,
    pub fields: Vec<String>,
    pub ciphertexts: Vec<Vec<u8>>,
    pub nonces: Vec<Vec<u8>>,
    pub bytes: Vec<i32>,
}

/// Content kept, its bytes added to the organization's count. A field
/// already kept, an export's retry, is kept once and counted once.
pub async fn keep_content(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    received_at: DateTime<Utc>,
    sealed: &SealedFields,
) -> sqlx::Result<()> {
    sqlx::query(
        "WITH kept AS (
             INSERT INTO telemetry.plain_content
                 (project_id, trace_id, span_id, field, received_at, ciphertext, nonce, bytes)
             SELECT $1, t, s, f, $3, c, n, b
               FROM UNNEST($4::text[], $5::text[], $6::text[], $7::bytea[], $8::bytea[], $9::int[])
                 AS u(t, s, f, c, n, b)
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
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// One content field as kept: its trace, span and name, sealed.
pub(crate) type KeptField = (String, String, String, Vec<u8>, Vec<u8>);

/// Every field the project keeps for the traces, sealed, in trace, span and
/// name order: the crate's own, read only by [`crate::content::open`], which
/// files each read.
pub(crate) async fn read_content(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> sqlx::Result<Vec<KeptField>> {
    sqlx::query_as(
        "SELECT trace_id, span_id, field, ciphertext, nonce
           FROM telemetry.plain_content
          WHERE project_id = $1 AND trace_id = ANY($2)
          ORDER BY trace_id, span_id, field",
    )
    .bind(project_id)
    .bind(trace_ids)
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
    .fetch_one(conn)
    .await?;
    Ok(gone.unsigned_abs())
}

/// A project's content deleted, and its bytes taken off its organization's
/// count; run before its settings row goes.
async fn purge_project_content(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_ids: &[ProjectId],
) -> sqlx::Result<u64> {
    delete_content(tx.conn(), project_ids, None, i64::MAX).await
}
