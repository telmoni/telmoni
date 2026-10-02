//! The organization itself: its name, its deletion by its owner and the
//! owner's restore. An operator's termination and restore, and the steps
//! the deletion sweep drives it through, are [`crate::sweep`]. It CAN be
//! renamed: once others can be let onto it, a shared workspace named after
//! its owner's inbox is one nobody else can name.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;

use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::{AuthError, OrganizationRole, OrganizationStatus, TelmoniError};

use crate::{
    AppState,
    db::{
        AuthLane,
        confirmation_codes::{self, Act, Purpose},
        identities, locks, organization_members, organizations,
    },
    handler::{
        ActingOrganization, account::DeletionRequest, account::wrong_code, acting_organization,
        deletion::RESTORE_WINDOW_SECONDS, organization_of, organization_role_or_forbidden,
    },
    model::DeletionKind,
};
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{maintenance_scope, organization_scope, person_scope};
use telmoni_shared::{AuditAction, TelmoniResourceKind};
use telmoni_shared::{AuthzError, slug};

/// The longest name worth storing: a real company with a qualifier, still
/// whole in a rail row rather than an ellipsis.
use crate::db::organizations::MAX_ORGANIZATION_NAME;

/// `POST /internal/organization/deletion-code` — mail the owner the code that
/// authorizes deleting the organization in `x-organization-id`, and only that
/// one: the code is bound to it.
pub async fn request_organization_deletion_code(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let user_id = principal.user_id;

    // `acting_organization` has already refused an organization being deleted.
    let ActingOrganization { mut tx, role } =
        acting_organization(&state, &organization, &user_id).await?;
    let row = organizations::get(&mut tx, &organization).await?;
    tx.commit().await?;
    if role != OrganizationRole::Owner {
        return Err(
            AuthzError::Forbidden("only the organization's owner can delete it".into()).into(),
        );
    }
    let Some(row) = row else {
        return Err(AuthError::NotFound("no such organization".into()).into());
    };

    let code = confirmation_codes::generate_code();
    let expires_at = Utc::now() + Duration::minutes(confirmation_codes::CODE_TTL_MINUTES);
    let mut tx = person_scope(&state.db, &user_id).await?;
    let Some(person) = identities::get(&mut tx, &user_id).await? else {
        return Err(AuthError::Unauthenticated.into());
    };
    confirmation_codes::create(
        &mut tx,
        &user_id,
        Act::OrganizationDeletion(&organization),
        &confirmation_codes::hash_code(&code),
        expires_at,
    )
    .await?;
    tx.commit().await?;

    let label = crate::identity::organization_label(row.name.as_deref(), Some(&person.email));
    if let Err(e) = state
        .mailer
        .send_organization_deletion_code(&person.email, &code, &label)
        .await
    {
        tracing::error!(organization_id = %organization, error = %e,
            "organization-deletion code mail failed — check MAIL_FROM, the mail transport and the sending domain's DNS");
        return Err(TelmoniError::MailDelivery {
            context: "organization-deletion confirmation code".into(),
        });
    }

    tracing::info!(organization_id = %organization, "organization-deletion confirmation code issued");
    Ok((StatusCode::ACCEPTED, Json(json!({ "sent": true }))))
}

/// `DELETE /internal/organization` — the owner deletes the organization in
/// `x-organization-id`, gated by the emailed code minted for it. It is marked
/// `pending_deletion` in one transaction — from which nothing acts in it and
/// none of its keys validates — then the purge hook runs inline, and the
/// sweep deletes the row once the restore window has passed. 202 either way,
/// naming when the row goes. Nobody's account is touched: its members simply
/// stop being in it.
///
/// ⚠ **Its keys are NOT revoked**, unlike an account deletion's. The status
/// test in `tokens::validate` refuses every key of a pending organization
/// from the mark, and a restore inside the window brings the organization
/// back whole, keys included; revoked rows would have been swept by the
/// retention job before the window closed.
///
/// ⚠ **The owner is re-read under the organization's lock.** Without it, an
/// owner who had just handed the organization over could still delete it
/// with a code minted before the transfer.
pub async fn delete_organization(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<DeletionRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let user_id = principal.user_id;

    // Both keys bound: the person's for their code, the organization's for its
    // roster, its projects and its audit chain.
    let tx = person_scope(&state.db, &user_id).await?;
    let mut tx = tx.bind_organization(&organization).await?;
    // Membership before the lock, so a stranger naming any organization's id
    // cannot queue on its lock holding a pooled connection; the owner is then
    // read again under it.
    if organization_members::role_on_organization(&mut tx, &organization, &user_id)
        .await?
        .is_none()
    {
        return Err(
            AuthzError::Forbidden("you are not a member of this organization".into()).into(),
        );
    }
    locks::lock_organization(&mut tx, &organization).await?;
    if organization_role_or_forbidden(&mut tx, &organization, &user_id).await?
        != OrganizationRole::Owner
    {
        return Err(
            AuthzError::Forbidden("only the organization's owner can delete it".into()).into(),
        );
    }

    let Some(code_id) = confirmation_codes::find_live(
        &mut tx,
        &user_id,
        Act::OrganizationDeletion(&organization),
        &confirmation_codes::hash_code(req.code.trim()),
    )
    .await?
    else {
        let burned =
            confirmation_codes::register_failure(&mut tx, &user_id, Purpose::OrganizationDeletion)
                .await?;
        tx.commit().await?;
        return Err(wrong_code(burned));
    };
    confirmation_codes::consume(&mut tx, code_id).await?;

    let Some(erase_after) = organizations::mark_pending_deletion(
        &mut tx,
        &organization,
        DeletionKind::Owner,
        RESTORE_WINDOW_SECONDS,
    )
    .await?
    else {
        // The role check above refused anything but an active organization,
        // under the same lock this still holds.
        return Err(TelmoniError::Internal(format!(
            "{organization} changed status under its lock"
        )));
    };
    organization_members::clear_offers(&mut tx, &organization).await?;

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "organization_deletion",
                "deletion": "requested",
                "by": DeletionKind::Owner,
                "erase_after": erase_after,
            })),
        },
    )
    .await?;
    tx.commit().await?;

    tracing::info!(organization_id = %organization, erase_after = %erase_after,
        "organization deletion requested");
    Ok(crate::handler::deletion::deletion_response(&state, &organization, erase_after).await)
}

/// `POST /internal/organization/restore` — the owner brings back the
/// organization in `x-organization-id` that they deleted, while its restore
/// window is open. Everything it held is as it was, its keys included; what
/// the purge hook dropped at the deletion is not brought back. 204.
///
/// No code: undoing a deletion is the safe direction. The owner is read
/// under the organization's lock, as the deletion reads them, and the window
/// is judged by the database's clock, the same one the sweep's listing reads.
/// One Telmoni terminated is refused: only an operator restores that.
pub async fn restore_organization(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let user_id = principal.user_id;

    let mut tx = organization_scope(&state.db, &organization).await?;
    // Not `organization_role_or_forbidden`: that refuses a pending
    // organization, which is the only kind this lane acts on. The membership
    // is read before the lock, so a stranger neither learns its state nor
    // queues on its lock, and the owner is read again under it.
    if organization_members::role_on_organization(&mut tx, &organization, &user_id)
        .await?
        .is_none()
    {
        return Err(
            AuthzError::Forbidden("you are not a member of this organization".into()).into(),
        );
    }
    locks::lock_organization(&mut tx, &organization).await?;
    let role = organization_members::role_on_organization(&mut tx, &organization, &user_id)
        .await?
        .ok_or_else(|| -> TelmoniError {
            AuthzError::Forbidden("you are not a member of this organization".into()).into()
        })?;
    if role != OrganizationRole::Owner {
        return Err(
            AuthzError::Forbidden("only the organization's owner can restore it".into()).into(),
        );
    }
    let Some(row) = organizations::get(&mut tx, &organization).await? else {
        return Err(AuthError::NotFound("no such organization".into()).into());
    };
    if row.status != OrganizationStatus::PendingDeletion {
        return Err(AuthError::Conflict("this organization is not being deleted".into()).into());
    }
    match row.deletion_kind {
        Some(DeletionKind::Owner) => {}
        Some(DeletionKind::Operator) => {
            return Err(AuthError::Conflict(
                "this organization was closed by Telmoni; contact support to have it restored"
                    .into(),
            )
            .into());
        }
        Some(DeletionKind::Account) | None => {
            return Err(AuthError::Conflict(
                "this organization is being deleted with its owner's account".into(),
            )
            .into());
        }
    }
    // The window on the database's clock, the one the sweep's listing reads:
    // a row the sweep has listed for finalize is never restored a second
    // later by a skewed one. `statement_timestamp()`, not `now()`: the
    // transaction began before the wait on the organization's lock.
    let open: bool = sqlx::query_scalar("SELECT $1::timestamptz > statement_timestamp()")
        .bind(row.erase_after)
        .fetch_one(&mut *tx)
        .await?;
    if !open {
        return Err(AuthError::Conflict(
            "this organization's restore window has closed; it is being erased".into(),
        )
        .into());
    }
    if !organizations::restore(&mut tx, &organization).await? {
        return Err(TelmoniError::Internal(format!(
            "{organization} changed status under its lock"
        )));
    }

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "organization_deletion",
                "deletion": "restored",
                "by": DeletionKind::Owner,
            })),
        },
    )
    .await?;
    tx.commit().await?;

    tracing::info!(organization_id = %organization, "organization restored by its owner");
    Ok(StatusCode::NO_CONTENT)
}

/// Body of `PUT /internal/organization/name`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameOrganizationRequest {
    pub name: String,
}

/// `PUT /internal/organization/name` — name the organization. Its slug moves
/// with the name, so the answer carries both and the console follows it.
pub async fn rename_organization(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<RenameOrganizationRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;

    let Some(name) = crate::identity::sanitize_organization_name(&req.name) else {
        return Err(AuthError::BadRequest("give the organization a name".into()).into());
    };
    if name.chars().count() > MAX_ORGANIZATION_NAME {
        return Err(AuthError::BadRequest(format!(
            "an organization name is at most {MAX_ORGANIZATION_NAME} characters"
        ))
        .into());
    }

    let ActingOrganization {
        mut tx,
        role: caller_role,
    } = acting_organization(&state, &organization, &actor).await?;
    if !caller_role.can_manage_org_settings() {
        tx.commit().await?;
        return Err(AuthzError::Forbidden(
            "only the organization owner can rename the organization".into(),
        )
        .into());
    }

    // Whether a slug is free is the lane's to read: the organization's own
    // binding sees no other organization's row. A name that gives none keeps
    // the slug it had.
    let candidates = slug::candidates(slug::Scope::Organization, &name);
    let free = if candidates.is_empty() {
        None
    } else {
        let mut lane = maintenance_scope(&state.db, AuthLane).await?;
        let free = organizations::first_free_slug(&mut lane, &organization, &candidates).await?;
        lane.commit().await?;
        free
    };
    let Some(slug) = organizations::rename(&mut tx, &organization, &name, free.as_deref())
        .await
        .map_err(slug_taken)?
    else {
        tx.commit().await?;
        return Err(AuthError::NotFound("organization not found".into()).into());
    };

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "name": name, "slug": slug })),
        },
    )
    .await?;

    tx.commit().await?;
    tracing::info!(organization_id = %organization, "organization renamed");
    Ok(Json(json!({ "name": name, "slug": slug })))
}

/// The slug another organization took between [`organizations::first_free_slug`]
/// and the rename's write, as a 409 to try again rather than a 500.
fn slug_taken(e: sqlx::Error) -> TelmoniError {
    match e {
        sqlx::Error::Database(ref db) if db.constraint() == Some("organizations_slug_key") => {
            AuthError::Conflict(
                "another organization took this name's URL a moment ago — try again".into(),
            )
            .into()
        }
        other => other.into(),
    }
}
