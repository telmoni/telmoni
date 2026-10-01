//! `auth.feature_flags` + `auth.organization_flags` — the feature-flag store,
//! and the one resolver over it.
//!
//! Both tables are append-only; the answer is the latest row per key, applied
//! global first and then the organization's own onto an all-on catalog. A key
//! with no row is on.

use sqlx::prelude::FromRow;

use telmoni_shared::db::tenant_session::{
    Binding, Maintenance, Organization, ProjectAndOrganization, Scoped,
};
use telmoni_shared::{Flag, FlagSet, OrganizationId};

use crate::db::AuthLane;

/// Bindings an organization's overrides are read under: its own, a project
/// lane acting on its behalf, and the maintenance lane.
pub trait FlagRead: Binding {}
impl FlagRead for Organization {}
impl FlagRead for ProjectAndOrganization {}
impl FlagRead for Maintenance<AuthLane> {}

#[derive(Debug, FromRow)]
struct FlagRow {
    key: String,
    enabled: bool,
}

/// The latest global row per key.
async fn global_rows<B: Binding>(tx: &mut Scoped<'_, B>) -> sqlx::Result<Vec<FlagRow>> {
    sqlx::query_as::<_, FlagRow>(
        "SELECT DISTINCT ON (key) key, enabled
           FROM auth.feature_flags
          ORDER BY key, created_at DESC, id DESC",
    )
    .fetch_all(tx.conn())
    .await
}

/// The latest global row per key, then the latest of the organization's own,
/// in that order, so `apply` lays the organization's over the global. One
/// statement rather than two: this runs on every `/v1` request and every
/// `/me`.
async fn layered_rows<B: FlagRead>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
) -> sqlx::Result<Vec<FlagRow>> {
    sqlx::query_as::<_, FlagRow>(
        "SELECT key, enabled
           FROM (
                (SELECT DISTINCT ON (key) 0 AS layer, key, enabled
                   FROM auth.feature_flags
                  ORDER BY key, created_at DESC, id DESC)
                UNION ALL
                (SELECT DISTINCT ON (key) 1 AS layer, key, enabled
                   FROM auth.organization_flags
                  WHERE organization_id = $1
                  ORDER BY key, created_at DESC, id DESC)
           ) latest
          ORDER BY layer, key",
    )
    .bind(organization_id)
    .fetch_all(tx.conn())
    .await
}

fn apply(set: &mut FlagSet, rows: Vec<FlagRow>) {
    for row in rows {
        if let Ok(flag) = row.key.parse::<Flag>() {
            set.set(flag, row.enabled);
        }
    }
}

/// The global answer: every flag, with the global overrides applied. What
/// gates sign-up before an organization exists. The table carries no tenant
/// key and no policy, which is why any binding reads it.
pub async fn resolve_global<B: Binding>(tx: &mut Scoped<'_, B>) -> sqlx::Result<FlagSet> {
    let mut set = FlagSet::all_on();
    apply(&mut set, global_rows(tx).await?);
    Ok(set)
}

/// One organization's answer: global overrides, then the organization's own
/// on top.
pub async fn resolve_for_organization<B: FlagRead>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
) -> sqlx::Result<FlagSet> {
    let mut set = FlagSet::all_on();
    apply(&mut set, layered_rows(tx, organization_id).await?);
    Ok(set)
}

/// Whether `flag` resolves on for ANY of the organizations, by the same
/// precedence as [`resolve_for_organization`], in one statement: `/me` asked
/// it of every organization a person is in, two round trips each.
pub async fn on_for_any_organization(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    flag: Flag,
    organization_ids: &[OrganizationId],
) -> sqlx::Result<bool> {
    if organization_ids.is_empty() {
        return Ok(false);
    }
    sqlx::query_scalar(
        "SELECT COALESCE(bool_or(COALESCE(o.enabled, g.enabled, true)), false)
           FROM unnest($1::text[]) AS ids(organization_id)
           LEFT JOIN LATERAL (
               SELECT f.enabled FROM auth.organization_flags f
                WHERE f.organization_id = ids.organization_id AND f.key = $2
                ORDER BY f.created_at DESC, f.id DESC LIMIT 1) o ON true
           LEFT JOIN LATERAL (
               SELECT g.enabled FROM auth.feature_flags g
                WHERE g.key = $2
                ORDER BY g.created_at DESC, g.id DESC LIMIT 1) g ON true",
    )
    .bind(organization_ids)
    .bind(flag.as_str())
    .fetch_one(tx.conn())
    .await
}
