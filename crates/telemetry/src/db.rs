//! The telemetry module's Postgres: each project's settings, and its saved
//! views of the trace list.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{
    HasProject, Lane, Maintenance, MaintenanceLane, ProjectAndOrganization, Scoped,
};
use telmoni_shared::{ContentMode, OrganizationId, ProjectId};

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

/// Every settings row of an organization's projects, and their saved views,
/// found through those rows before they go: a view names no organization.
pub async fn purge_organization(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    organization_id: &OrganizationId,
) -> sqlx::Result<u64> {
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
    Ok(views + settings)
}

/// One project's saved views and settings row.
pub async fn purge_project(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
) -> sqlx::Result<u64> {
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
    Ok(views + settings)
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

/// A project's settings row, handed to the organization that now holds the
/// project. Answers whether there was one.
pub async fn move_project(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
) -> sqlx::Result<bool> {
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
