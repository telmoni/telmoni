//! Invitations: how somebody becomes a member.
//!
//! The invitee is matched by the address on their IDENTITY — what the
//! identity provider asserted — against the address the invitation was sent
//! to, so a forwarded link cannot seat somebody the inviter never named.
//!
//! ⚠ **An accept never rewrites a role.** It inserts, and a person already in
//! the organization is told so. Once the owner became a row, an upsert here
//! would have let an owner accepting an invitation to their own organization
//! demote themselves out of it.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{Maintenance, Scoped, maintenance_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, OrganizationId, OrganizationRole, OrganizationStatus,
    ProjectId, Role, TelmoniError, TelmoniResourceKind, UserId,
};

use crate::{
    AppState,
    db::{AuthLane, identities, invites, locks, members, organization_members, organizations},
    handler::{acting_project, authorize, member::grantable, parse_project_id},
    model::InviteScope,
};

/// How long an invitation lives: through a long weekend, and dead before a
/// link found in an old inbox is worth anything.
pub const INVITE_TTL_DAYS: i64 = 7;

/// How many invitations one project may send in a day.
pub const INVITE_DAILY_MAX: i64 = 20;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInviteRequest {
    /// The address to invite. Shape-checked and lowercased; never resolved.
    pub email: String,
    /// `admin` or `member`. There is no `owner`: a project's owner is its
    /// organization's owner.
    pub role: Role,
}

/// The secret from the link, as the invited person's browser presents it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviteTokenRequest {
    pub token: String,
}

impl std::fmt::Debug for InviteTokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InviteTokenRequest")
            .field("token", &"***")
            .finish()
    }
}

/// Hash the secret the way [`create_invite`] stored it.
fn hash_invite_token(raw: &str) -> String {
    telmoni_shared::digest::sha256_hex(raw.as_bytes())
}

/// `POST /internal/projects/{project_id}/invites` — offer somebody a role.
pub async fn create_invite(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    principal: Principal,
    Json(req): Json<CreateInviteRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let actor = principal.user_id;
    let role = grantable(req.role)?;
    let email = crate::identity::validate_email(&req.email)?;

    let acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Create, Resource::Member)?;
    let organization = acting.organization.clone();

    // ⚠ **Under the owning organization's scope.** Its flag overrides, its
    // roster and every address on it are the organization's rows; with only
    // `app.project_id` bound the flag read saw no override at all and the
    // "already a member" check saw no address.
    let mut acting = acting.enter_owner_scope().await?;
    let flags = crate::db::flags::resolve_for_organization(&mut acting.tx, &organization).await?;
    if !flags.is_on(telmoni_shared::Flag::Members) {
        return Err(telmoni_shared::TenantError::FeatureOff {
            flag: telmoni_shared::Flag::Members,
        }
        .into());
    }

    match organizations::status_for_share(&mut acting.tx, &organization).await? {
        Some(OrganizationStatus::Active) => {}
        Some(_) | None => {
            return Err(AuthError::Conflict(
                "this organization is being deleted — it can take no new members".into(),
            )
            .into());
        }
    }

    let inviter = identities::contact(&mut acting.tx, &actor).await?;
    if inviter.as_ref().is_some_and(|c| c.email == email) {
        return Err(AuthError::BadRequest(
            "that is your own address — you are already in this project".into(),
        )
        .into());
    }
    if invites::already_a_member_by_email(&mut acting.tx, &project_id, &email).await? {
        return Err(AuthError::Conflict("that address is already a member".into()).into());
    }
    // ⚠ Only a named organization invites: the invitation shows the person the
    // organization's name, and the console asks the owner for one before it
    // opens; this holds the same line for any client.
    let Some(name) = organizations::get(&mut acting.tx, &organization)
        .await?
        .and_then(|o| o.name)
    else {
        return Err(AuthError::Conflict(
            "name the organization before inviting anyone to its projects".into(),
        )
        .into());
    };
    let mut acting = acting.leave_owner_scope().await?;

    let sent_today =
        invites::count_since(&mut acting.tx, &project_id, Utc::now() - Duration::days(1)).await?;
    if sent_today >= INVITE_DAILY_MAX {
        return Err(AuthError::Conflict(format!(
            "you have sent {INVITE_DAILY_MAX} invitations today — the rest can go out tomorrow"
        ))
        .into());
    }

    let secret = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + Duration::days(INVITE_TTL_DAYS);

    invites::revoke_live_for_email(&mut acting.tx, &project_id, &email).await?;
    let invite_id = invites::create(
        &mut acting.tx,
        &project_id,
        &email,
        role,
        &hash_invite_token(&secret),
        &actor,
        expires_at,
    )
    .await?;

    emit_audit(
        &mut acting.tx,
        AuditEvent {
            organization_id: &organization,
            in_project: Some(&project_id),
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(invite_id.to_string().as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "invited",
                "email": email,
                "role": role.to_string(),
            })),
        },
    )
    .await?;
    acting.tx.commit().await?;

    let link = invite_link(&state, &secret);
    let label = crate::identity::organization_label(Some(&name));
    let inviter = inviter.map_or_else(|| label.clone(), |c| c.display());
    if let Err(e) = state
        .mailer
        .send_member_invite(
            &email,
            &inviter,
            &label,
            crate::mailer::InvitedTo::Project,
            &role.to_string(),
            &link,
        )
        .await
    {
        tracing::warn!(invite = %invite_id, error = %e, "invitation mail failed");
    }

    tracing::info!(project_id = %project_id, invite = %invite_id, %role, "member invited");
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": invite_id,
            "link": link,
            "expiresAt": expires_at,
            "ownerOrganizationId": organization,
        })),
    ))
}

/// `GET /internal/projects/{project_id}/invites` — what is still outstanding.
pub async fn list_invites(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let actor = principal.user_id;

    let mut acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Read, Resource::Member)?;
    let rows = invites::list_pending(&mut acting.tx, &project_id).await?;
    acting.tx.commit().await?;

    Ok(Json(json!({ "invites": rows })))
}

/// `DELETE /internal/projects/{project_id}/invites/{invite_id}` — take the
/// offer back. The link dies immediately, because accept reads liveness.
pub async fn revoke_invite(
    State(state): State<Arc<AppState>>,
    Path((project_id, invite_id)): Path<(String, Uuid)>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let actor = principal.user_id;

    let mut acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Delete, Resource::Member)?;

    let Some(email) = invites::revoke(&mut acting.tx, &project_id, invite_id).await? else {
        return Err(AuthError::NotFound("no such invitation".into()).into());
    };
    let owner = acting.organization.clone();
    emit_audit(
        &mut acting.tx,
        AuditEvent {
            organization_id: &owner,
            in_project: Some(&project_id),
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(invite_id.to_string().as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "kind": "invite_revoked" })),
        },
    )
    .await?;
    acting.tx.commit().await?;

    tracing::info!(project_id = %project_id, invite = %invite_id, "invitation revoked");
    Ok(Json(json!({
        "inviteId": invite_id,
        "email": email,
        "ownerOrganizationId": owner,
    })))
}

/// What the accept page prints for who is inviting: the person who sent it,
/// or the organization when they have since been erased.
fn inviter_label(
    inviter_email: Option<&str>,
    inviter_display_name: Option<&str>,
    organization: &str,
) -> String {
    inviter_email.map_or_else(
        || organization.to_owned(),
        |email| crate::identity::display_for(inviter_display_name, email),
    )
}

/// `POST /internal/invites/look` — what does this link open?
pub async fn look_up_invite(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InviteTokenRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let token_hash = hash_invite_token(&req.token);
    let found = invites::find_live(&mut tx, &token_hash).await?;

    if let Some(invite) = found {
        tx.commit().await?;
        let organization = crate::identity::organization_label(invite.organization_name.as_deref());
        // ⚠ `scope` says which roster the link seats somebody on. The two
        // ladders spell their roles the same, so the role alone cannot.
        return Ok(Json(json!({
            "scope": "project",
            "inviter": inviter_label(
                invite.inviter_email.as_deref(),
                invite.inviter_display_name.as_deref(),
                &organization,
            ),
            "organization": organization,
            "email": invite.email,
            "role": invite.role.to_string(),
        })));
    }

    let found_organization =
        organization_members::find_live_organization_invite(&mut tx, &token_hash).await?;
    tx.commit().await?;

    if let Some(invite) = found_organization {
        let organization = crate::identity::organization_label(invite.organization_name.as_deref());
        return Ok(Json(json!({
            "scope": "organization",
            "inviter": inviter_label(
                invite.inviter_email.as_deref(),
                invite.inviter_display_name.as_deref(),
                &organization,
            ),
            "organization": organization,
            "email": invite.email,
            "role": invite.role.to_string(),
        })));
    }

    Err(AuthError::NotFound("this invitation has expired or been withdrawn".into()).into())
}

/// A PROJECT invitation both lanes have already decided to honour.
struct AcceptedProjectInvite<'a> {
    id: Uuid,
    project_id: &'a ProjectId,
    organization_id: &'a OrganizationId,
    role: Role,
}

/// An ORGANIZATION invitation both lanes have already decided to honour.
struct AcceptedOrganizationInvite<'a> {
    id: Uuid,
    organization_id: &'a OrganizationId,
    role: OrganizationRole,
}

/// The organization's lock, and a refusal if it is on its way out. Both seat
/// paths start here — after [`accepter`] has taken the person's lock, so the
/// order is the person's, then the organization's, as every lane that takes
/// both — and a person must never land in an organization whose deletion has
/// already been confirmed.
async fn lock_active(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    organization: &OrganizationId,
) -> Result<(), TelmoniError> {
    locks::lock_organization(&mut *tx, organization).await?;
    if organizations::status(tx, organization).await? != Some(OrganizationStatus::Active) {
        return Err(AuthError::Conflict(
            "this organization is being deleted — it can take no new members".into(),
        )
        .into());
    }
    Ok(())
}

/// Seat an accepted project invitation, **commit**, and announce it.
async fn seat_project_member(
    state: &AppState,
    mut tx: Scoped<'_, Maintenance<AuthLane>>,
    invite: AcceptedProjectInvite<'_>,
    actor: &UserId,
    accepter: &str,
) -> Result<(), TelmoniError> {
    let organization = invite.organization_id;
    lock_active(&mut tx, organization).await?;
    if organization_members::role_on_organization(&mut tx, organization, actor).await?
        == Some(OrganizationRole::Owner)
    {
        return Err(AuthError::BadRequest(
            "you own this project — there is nothing to accept".into(),
        )
        .into());
    }

    if !invites::mark_accepted(&mut tx, invite.id, actor).await? {
        return Err(AuthError::Conflict("this invitation was already used".into()).into());
    }

    match members::insert(&mut tx, invite.project_id, actor, invite.role, actor).await {
        Ok(_) => {}
        Err(e) if members::already_a_member(&e) => {}
        Err(e) => return Err(e.into()),
    }

    // Every seat holder is on the owning organization's roster: that is what
    // lets the organization read their address for its own project rosters.
    if organization_members::add_member(
        &mut tx,
        organization,
        actor,
        OrganizationRole::Member,
        actor,
    )
    .await?
    {
        emit_audit(
            &mut tx,
            AuditEvent {
                organization_id: organization,
                in_project: None,
                actor: Actor::User(actor.as_str()),
                action: AuditAction::Created,
                resource_kind: TelmoniResourceKind::Member,
                resource_id: Some(actor.as_str()),
                request_id: None,
                ip_address: None,
                user_agent: None,
                metadata: Some(json!({
                    "kind": "enrolled_with_project_invite",
                    "invite_id": invite.id,
                    "role": OrganizationRole::Member.to_string(),
                    "scope": InviteScope::Organization,
                })),
            },
        )
        .await?;
    }

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: organization,
            in_project: Some(invite.project_id),
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(actor.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "invite_accepted",
                "invite_id": invite.id,
                "role": invite.role.to_string(),
            })),
        },
    )
    .await?;
    tx.commit().await?;

    crate::notify::emit_member_joined(
        state,
        invite.project_id,
        organization,
        actor,
        accepter,
        invite.role,
    )
    .await;
    Ok(())
}

/// Seat an accepted organization invitation, and **commit**. A person already
/// in the organization — at any role, the owner's included — is refused, and
/// the invitation stays unaccepted.
async fn seat_organization_member(
    mut tx: Scoped<'_, Maintenance<AuthLane>>,
    invite: AcceptedOrganizationInvite<'_>,
    actor: &UserId,
) -> Result<(), TelmoniError> {
    lock_active(&mut tx, invite.organization_id).await?;
    if !organization_members::add_member(&mut tx, invite.organization_id, actor, invite.role, actor)
        .await?
    {
        return Err(AuthError::Conflict("you are already in this organization".into()).into());
    }
    if !organization_members::mark_accepted(&mut tx, invite.id, actor).await? {
        return Err(AuthError::Conflict("this invitation was already used".into()).into());
    }

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: invite.organization_id,
            in_project: None,
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(actor.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "organization_invite_accepted",
                "invite_id": invite.id,
                "role": invite.role.to_string(),
                "scope": InviteScope::Organization,
            })),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The accepting person's identity, read under their own lock in the
/// maintenance lane the accept runs in. A bearer auth never recorded is
/// somebody who has not signed in through it.
///
/// ⚠ **The person's lock is what the self-service lanes read under.**
/// `acting_person_in` lets somebody in no organization act with no chain to
/// record on, after checking under this lock that they are in none; an accept
/// that did not take it could seat them between that check and the act, and
/// the act would go unaudited. It is taken before the address is read, so an
/// email change — which holds the same lock — cannot move the address between
/// the match below and the seat. Under it too, an account whose deletion has
/// been confirmed takes no new seat for its erasure to trip over.
///
/// The link's token is the proof that the caller holds the inbox the
/// invitation went to, so the address need only match; the in-console lanes,
/// which have no token, go through [`verified_accepter`].
async fn accepter(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    actor: &UserId,
) -> Result<identities::Person, TelmoniError> {
    locks::lock_person(&mut *tx, actor).await?;
    let person = identities::get(tx, actor)
        .await?
        .ok_or_else(|| -> TelmoniError {
            AuthError::NotFound("sign in again to accept this invitation".into()).into()
        })?;
    if person.deletion_requested_at.is_some() {
        return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
    }
    Ok(person)
}

/// [`accepter`], for a lane that names the invitation by id rather than by
/// its token. ⚠ **Verified, or refused.** The recorded address is whatever
/// was asserted at sign-up or by the provider, and with `VERIFY_EMAIL` off
/// nothing proved it; a password sign-up for an invited address is open to
/// anyone who knows it. Only a verified address is proof the caller holds the
/// inbox, so without one the link in the mail is the only way in.
async fn verified_accepter(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    actor: &UserId,
) -> Result<identities::Person, TelmoniError> {
    let person = accepter(tx, actor).await?;
    if !person.email_verified {
        return Err(AuthzError::Forbidden(
            "this account's address has not been verified — open the invitation from the link \
             in its mail"
                .into(),
        )
        .into());
    }
    Ok(person)
}

/// The refusal for an invitation addressed to somebody else. The address it
/// went to is not repeated: the caller is not its holder.
fn not_yours() -> TelmoniError {
    AuthzError::Forbidden(
        "this invitation was sent to a different address — sign in with the one it was mailed to"
            .into(),
    )
    .into()
}

/// `POST /internal/invites/accept` — spend the link, and become a member.
pub async fn accept_invite(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Json(req): Json<InviteTokenRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let token_hash = hash_invite_token(&req.token);

    if let Some(invite) = invites::find_live(&mut tx, &token_hash).await? {
        let me = accepter(&mut tx, &actor).await?;
        if me.email != invite.email {
            return Err(not_yours());
        }
        seat_project_member(
            &state,
            tx,
            AcceptedProjectInvite {
                id: invite.id,
                project_id: &invite.project_id,
                organization_id: &invite.organization_id,
                role: invite.role,
            },
            &actor,
            &crate::identity::display_for(me.display_name.as_deref(), &me.email),
        )
        .await?;

        tracing::info!(
            project_id = %invite.project_id,
            member = %actor,
            role = %invite.role,
            "invitation accepted"
        );
        return Ok((
            StatusCode::CREATED,
            Json(json!({
                "projectId": invite.project_id,
                "inviteId": invite.id,
                "inviterEmail": invite.inviter_email,
                "ownerOrganizationId": invite.organization_id,
            })),
        ));
    }

    if let Some(invite) =
        organization_members::find_live_organization_invite(&mut tx, &token_hash).await?
    {
        let me = accepter(&mut tx, &actor).await?;
        if me.email != invite.email {
            return Err(not_yours());
        }
        seat_organization_member(
            tx,
            AcceptedOrganizationInvite {
                id: invite.id,
                organization_id: &invite.organization_id,
                role: invite.role,
            },
            &actor,
        )
        .await?;

        tracing::info!(
            organization_id = %invite.organization_id,
            member = %actor,
            role = %invite.role,
            "organization invitation accepted"
        );
        return Ok((
            StatusCode::CREATED,
            Json(json!({
                "organizationId": invite.organization_id,
                "inviteId": invite.id,
                "inviterEmail": invite.inviter_email,
                "ownerOrganizationId": invite.organization_id,
            })),
        ));
    }

    Err(AuthError::NotFound("this invitation has expired or been withdrawn".into()).into())
}

/// The URL that opens an invitation, on the console this service belongs to.
fn invite_link(state: &AppState, secret: &str) -> String {
    format!(
        "{}/invite/{secret}",
        state.config.app_url.trim_end_matches('/')
    )
}

/// `GET /internal/me/invites` — list pending invitations addressed to the caller.
pub async fn list_my_incoming_invites(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let me = verified_accepter(&mut tx, &actor).await?;
    let rows = invites::list_incoming(&mut tx, &me.email, &actor).await?;
    tx.commit().await?;

    Ok(Json(json!({ "invites": rows })))
}

/// `POST /internal/me/invites/{invite_id}/accept` — accept an incoming invitation directly.
pub async fn accept_my_incoming_invite(
    State(state): State<Arc<AppState>>,
    Path(invite_id): Path<Uuid>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let me = verified_accepter(&mut tx, &actor).await?;

    if let Some(invite) =
        invites::find_incoming_member_invite(&mut tx, invite_id, &me.email).await?
    {
        seat_project_member(
            &state,
            tx,
            AcceptedProjectInvite {
                id: invite.id,
                project_id: &invite.project_id,
                organization_id: &invite.owner_organization_id,
                role: invite.role,
            },
            &actor,
            &crate::identity::display_for(me.display_name.as_deref(), &me.email),
        )
        .await?;

        tracing::info!(
            project_id = %invite.project_id,
            member = %actor,
            role = %invite.role,
            "incoming project invitation accepted"
        );
        return Ok((
            StatusCode::OK,
            Json(json!({
                "status": "accepted",
                "scope": InviteScope::Project,
                "targetId": invite.project_id,
                "inviteId": invite.id,
                "inviterEmail": invite.inviter_email,
                "ownerOrganizationId": invite.owner_organization_id,
            })),
        ));
    }

    if let Some(invite) =
        invites::find_incoming_organization_invite(&mut tx, invite_id, &me.email).await?
    {
        seat_organization_member(
            tx,
            AcceptedOrganizationInvite {
                id: invite.id,
                organization_id: &invite.organization_id,
                role: invite.role,
            },
            &actor,
        )
        .await?;

        tracing::info!(
            organization_id = %invite.organization_id,
            member = %actor,
            role = %invite.role,
            "incoming organization invitation accepted"
        );
        return Ok((
            StatusCode::OK,
            Json(json!({
                "status": "accepted",
                "scope": InviteScope::Organization,
                "targetId": invite.organization_id,
                "inviteId": invite.id,
                "inviterEmail": invite.inviter_email,
                "ownerOrganizationId": invite.owner_organization_id,
            })),
        ));
    }

    Err(AuthError::NotFound("invitation not found or no longer active".into()).into())
}

/// `POST /internal/me/invites/{invite_id}/decline` — decline an incoming invitation.
pub async fn decline_my_incoming_invite(
    State(state): State<Arc<AppState>>,
    Path(invite_id): Path<Uuid>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let me = verified_accepter(&mut tx, &actor).await?;

    if let Some(invite) =
        invites::find_incoming_member_invite(&mut tx, invite_id, &me.email).await?
    {
        if !invites::decline_incoming_member_invite(&mut tx, invite_id, &me.email).await? {
            return Err(AuthError::Conflict("this invitation was already resolved".into()).into());
        }
        let owner = invite.owner_organization_id;
        emit_audit(
            &mut tx,
            AuditEvent {
                organization_id: &owner,
                in_project: Some(&invite.project_id),
                actor: Actor::User(actor.as_str()),
                action: AuditAction::Deleted,
                resource_kind: TelmoniResourceKind::Member,
                resource_id: Some(invite_id.to_string().as_str()),
                request_id: None,
                ip_address: None,
                user_agent: None,
                metadata: Some(json!({ "kind": "invite_declined" })),
            },
        )
        .await?;
        tx.commit().await?;
        tracing::info!(
            project_id = %invite.project_id,
            invite = %invite_id,
            member = %actor,
            "project invitation declined"
        );
        return Ok(Json(json!({
            "status": "declined",
            "scope": InviteScope::Project,
            "targetId": invite.project_id,
            "inviteId": invite.id,
            "inviterEmail": invite.inviter_email,
            "ownerOrganizationId": owner,
        })));
    }

    if let Some(invite) =
        invites::find_incoming_organization_invite(&mut tx, invite_id, &me.email).await?
    {
        if !invites::decline_incoming_organization_invite(&mut tx, invite_id, &me.email).await? {
            return Err(AuthError::Conflict("this invitation was already resolved".into()).into());
        }
        emit_audit(
            &mut tx,
            AuditEvent {
                organization_id: &invite.organization_id,
                in_project: None,
                actor: Actor::User(actor.as_str()),
                action: AuditAction::Deleted,
                resource_kind: TelmoniResourceKind::Member,
                resource_id: Some(invite_id.to_string().as_str()),
                request_id: None,
                ip_address: None,
                user_agent: None,
                metadata: Some(
                    json!({ "kind": "organization_invite_declined", "scope": InviteScope::Organization }),
                ),
            },
        )
        .await?;
        tx.commit().await?;
        tracing::info!(
            organization_id = %invite.organization_id,
            invite = %invite_id,
            member = %actor,
            "organization invitation declined"
        );
        return Ok(Json(json!({
            "status": "declined",
            "scope": InviteScope::Organization,
            "targetId": invite.organization_id,
            "inviteId": invite.id,
            "inviterEmail": invite.inviter_email,
            "ownerOrganizationId": invite.owner_organization_id,
        })));
    }

    Err(AuthError::NotFound("invitation not found or no longer active".into()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sibling of the token request's masking test: this body once derived
    /// `Debug` and rendered the live invite secret in full.
    #[test]
    fn invite_token_request_debug_masks_the_raw_token() {
        let req = InviteTokenRequest {
            token: "inv_secret_from_the_link_9876".into(),
        };
        let rendered = format!("{req:?}");
        assert!(
            !rendered.contains("secret_from_the_link"),
            "leaked: {rendered}"
        );
        assert!(rendered.contains("***"));
    }
}
