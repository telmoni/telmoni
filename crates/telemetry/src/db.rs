//! The telemetry module's Postgres: each project's settings.

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

/// Every settings row of an organization's projects.
pub async fn purge_organization(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    organization_id: &OrganizationId,
) -> sqlx::Result<u64> {
    Ok(
        sqlx::query("DELETE FROM telemetry.project_settings WHERE organization_id = $1")
            .bind(organization_id)
            .execute(tx.conn())
            .await?
            .rows_affected(),
    )
}

/// One project's settings row.
pub async fn purge_project(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
) -> sqlx::Result<u64> {
    Ok(
        sqlx::query("DELETE FROM telemetry.project_settings WHERE project_id = $1")
            .bind(project_id)
            .execute(tx.conn())
            .await?
            .rows_affected(),
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
