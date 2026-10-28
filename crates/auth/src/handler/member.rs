//! Members: who else may see an organization's runs, one project at a time.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::Email;
use telmoni_shared::OrganizationAction;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::extract::{Json, Path};
use telmoni_shared::person_token::Principal;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, OrganizationId, OrganizationRole, ProjectId, Role,
    TelmoniError, TelmoniResourceKind, UserId,
};
use uuid::Uuid;

use super::authorize_organization;
use crate::mailer::AuthMail;
use crate::{
    AppState,
    db::{
        AuthLane, audit, flags, identities, invites, members, organization_members, organizations,
        projects,
    },
    handler::{
        ActingProject, acting_project, answer::Roster, authorize, hold_project,
        invite::INVITE_DAILY_MAX,
    },
    model::AuditKind,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRoleRequest {
    pub role: Role,
}

/// Body of `POST /internal/v1/projects/{project_id}/members`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GiveSeatRequest {
    /// A member of the organization — their user id, as its roster lists them.
    pub member_id: UserId,
    pub role: Role,
}

/// `owner` is a role on the wire, but not one anyone can be GRANTED on a
/// project: a project's owner is its organization's owner.
pub(crate) fn grantable(role: Role) -> Result<Role, TelmoniError> {
    match role {
        Role::Admin | Role::Member => Ok(role),
        Role::Owner => Err(TelmoniError::BadRequest(
            "a seat is admin or member — the project's owner is its organization's owner".into(),
        )),
    }
}

/// Whether `member` owns the organization that holds this project. Read under
/// the owning organization's scope, the only one its roster is visible in,
/// and handed back project-scoped.
async fn is_organization_owner<'a>(
    acting: ActingProject<'a>,
    member: &UserId,
) -> Result<(ActingProject<'a>, bool, Option<String>), TelmoniError> {
    let mut acting = acting.enter_owner_scope().await?;
    let role =
        organization_members::role_on_organization(&mut acting.tx, &acting.organization, member)
            .await?;
    let name =
        organization_members::member_display_name(&mut acting.tx, &acting.organization, member)
            .await?;
    let acting = acting.leave_owner_scope().await?;
    Ok((acting, role == Some(OrganizationRole::Owner), name))
}

/// `GET /internal/v1/projects/{project_id}/members` — the roster.
pub async fn list_members(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Read, Resource::Member)?;
    // ⚠ Under the owning organization as well as the project: the owner's row
    // and every person's address are the organization's to see, and a
    // project-only scope would join them against nothing.
    let mut acting = acting.enter_owner_scope().await?;
    let rows = members::list_for_project(&mut acting.tx, &project_id).await?;
    let acting = acting.leave_owner_scope().await?;
    let organization = acting.organization.clone();
    // The owning organization's ceiling, which a seat here counts against,
    // to whoever could invite: what stands against it counts every project
    // of the organization, which a member of this one has no roster of.
    let may_invite = telmoni_shared::rbac::can(acting.role, Verb::Create, Resource::Member);
    acting.tx.commit().await?;
    // Read once the roster's connection is back, never beside it.
    let ceiling = if may_invite {
        crate::handler::invite::roster_ceiling(&state, &organization).await?
    } else {
        None
    };

    Ok(Json(Roster {
        members: rows,
        ceiling,
    }))
}

/// `POST /internal/v1/projects/{project_id}/members` — a seat for a member of
/// the organization, given by its owner or an admin with nothing to accept:
/// the person agreed to the organization when they joined it. The project's
/// feed is told, and the person by mail and in their bell (`/me`).
pub async fn give_seat(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
    Json(req): Json<GiveSeatRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let member = req.member_id;
    let actor = principal.user_id;
    let role = grantable(req.role)?;
    // Before the lane's transaction opens: a request never holds two of the
    // pool's connections at once.
    let going = leaving(&state, &member).await?;

    let acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Create, Resource::Member)?;
    from_the_roster(acting.organization_role)?;
    // A paused organization takes nothing new. Its member ceiling counts the
    // roster, which a seat adds nobody to, so it is not asked.
    if let Some(entitlements) = state.siblings.entitlements.as_deref() {
        telmoni_shared::seam::admit_new(entitlements, &acting.organization).await?;
    }
    let acting = acting
        .hold(&project_id, &actor, Verb::Create, Resource::Member)
        .await?;
    from_the_roster(acting.organization_role)?;

    let mut acting = acting.enter_owner_scope().await?;
    let organization = acting.organization.clone();
    let flags = flags::resolve_for_organization(&mut acting.tx, &organization).await?;
    if !flags.is_on(telmoni_shared::Flag::Members) {
        return Err(telmoni_shared::TenantError::FeatureOff {
            flag: telmoni_shared::Flag::Members,
        }
        .into());
    }
    match organization_members::role_on_organization(&mut acting.tx, &organization, &member).await?
    {
        Some(OrganizationRole::Member) => {}
        Some(_) => {
            return Err(TelmoniError::BadRequest(
                "the organization's owner and admins are on every project already".into(),
            ));
        }
        None => return Err(not_on_the_roster()),
    }
    let Some(person) = identities::contact(&mut acting.tx, &member).await? else {
        return Err(not_on_the_roster());
    };
    // Someone whose account is going is seated nowhere, as no invitation of
    // theirs is accepted, and the refusal says no more than a stranger's.
    if going {
        return Err(not_on_the_roster());
    }
    // The mail each seat sends is capped as invitations are: the chain counts
    // the seats given, which a removal takes away from the roster.
    let since = Utc::now() - Duration::days(1);
    if audit::seats_given_since(&mut acting.tx, &organization, &project_id, since).await?
        >= INVITE_DAILY_MAX
    {
        return Err(AuthError::Conflict(format!(
            "you have added {INVITE_DAILY_MAX} members to this project today — the rest can be added tomorrow"
        ))
        .into());
    }
    let Some(home) = organizations::name_of(&mut acting.tx, &organization).await? else {
        return Err(AuthError::NotFound("organization not found".into()).into());
    };
    let Some(project) = projects::standing(&mut acting.tx, &project_id).await? else {
        return Err(AuthError::NotFound("project not found".into()).into());
    };
    let giver = identities::contact(&mut acting.tx, &actor)
        .await?
        .map_or_else(|| home.clone(), |c| c.display());
    let mut acting = acting.leave_owner_scope().await?;

    // An invitation of theirs to this project is spent by the seat: accepted
    // later, it would find the seat already held.
    let withdrawn =
        invites::revoke_live_for_email(&mut acting.tx, &project_id, &person.email).await?;
    let seat = match members::give(&mut acting.tx, &project_id, &member, role, &actor).await {
        Ok(id) => id,
        Err(e) if members::already_a_member(&e) => {
            return Err(AuthError::Conflict("that person is already a member".into()).into());
        }
        Err(e) => return Err(e.into()),
    };
    emit_audit(
        &mut acting.tx,
        AuditEvent::new(
            &organization,
            Actor::User(actor.as_str()),
            AuditAction::Created,
            TelmoniResourceKind::Member,
        )
        .in_project(&project_id)
        .on(member.as_str())
        .details(json!({
            "kind": AuditKind::SeatGiven,
            "role": role,
            "seat_id": seat,
            "invitations_withdrawn": withdrawn,
        })),
    )
    .await?;
    acting.tx.commit().await?;

    // A deletion confirmed while the seat waited leaves it to the erasure,
    // whose last pass a notice naming the person could outrun: nobody is told.
    let gone = leaving(&state, &member).await.unwrap_or_else(|e| {
        tracing::warn!(project_id = %project_id, member = %member, error = %e, "seat's person not read again");
        true
    });
    if gone {
        tracing::info!(project_id = %project_id, member = %member, "member seated as their account goes; nobody told");
    } else {
        let name = person.display();
        crate::notify::emit_member_seated(&state, &project_id, &organization, &member, &name, role)
            .await;
        // By id, which outlasts a slug: the console takes either segment as one.
        let link = state
            .issuer
            .page(&format!("/{organization}/{project_id}"), &[]);
        if let Err(e) = state
            .mailer
            .send_mail(
                person.email.as_str(),
                AuthMail::ProjectSeat {
                    giver: &giver,
                    project: &project.name,
                    organization: &home,
                    role,
                    link: &link,
                },
            )
            .await
        {
            tracing::warn!(project_id = %project_id, member = %member, error = %e, "seat mail failed");
        }
    }

    tracing::info!(project_id = %project_id, member = %member, %role, "member seated from the roster");
    Ok((
        StatusCode::CREATED,
        Json(SeatGiven {
            id: seat,
            member_id: member,
            email: person.email,
            role,
            owner_organization_id: organization,
        }),
    ))
}

/// A seat given from the roster: whose, at which role, and whose console to
/// tell.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatGiven {
    pub id: Uuid,
    pub member_id: UserId,
    pub email: Email,
    pub role: Role,
    pub owner_organization_id: OrganizationId,
}

/// A seat seen by its holder: whose console to tell, none where the seat
/// is gone.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatSeen {
    pub organization_id: Option<OrganizationId>,
}

/// The roster is the organization's: its owner and admins give seats from
/// it, and someone not on it is refused as a member is.
fn from_the_roster(role: Option<OrganizationRole>) -> Result<(), TelmoniError> {
    authorize_organization(
        role.unwrap_or(OrganizationRole::Member),
        OrganizationAction::GiveSeats,
    )
}

fn not_on_the_roster() -> TelmoniError {
    AuthError::NotFound("that person is not in this organization".into()).into()
}

/// Whether the person's account deletion is confirmed, or no account is
/// recorded. Their account's own row is the lane's alone to read.
async fn leaving(state: &AppState, member: &UserId) -> Result<bool, TelmoniError> {
    let person =
        crate::db::lane(&state.db, async |lane| identities::get(lane, member).await).await?;
    Ok(person.is_none_or(|p| p.deletion_requested_at.is_some()))
}

/// `POST /internal/v1/me/seats/{project_id}/seen` — the person has seen the
/// seat given to them, and the bell stops showing it. Idempotent: answers the
/// organization holding the seat when one was waiting, for the person's other
/// consoles to be told, and `null` when none was.
pub async fn mark_seat_seen(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    // A move marks its project's seats under its own locks: a wait past the
    // role's `lock_timeout` answers 409, as every lock wait does.
    let organization = members::mark_seen(&mut tx, &project_id, &principal.user_id).await?;
    tx.commit().await?;
    Ok(Json(SeatSeen {
        organization_id: organization,
    }))
}

/// `PUT /internal/v1/projects/{project_id}/members/{member_id}/role`.
pub async fn update_role(
    State(state): State<Arc<AppState>>,
    Path((project_id, member_id)): Path<(ProjectId, UserId)>,
    principal: Principal,
    Json(req): Json<UpdateRoleRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;
    let role = grantable(req.role)?;

    let acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Update, Resource::Member)?;
    let acting = acting
        .hold(&project_id, &actor, Verb::Update, Resource::Member)
        .await?;

    let (mut acting, owner, _) = is_organization_owner(acting, &member_id).await?;
    if owner {
        acting.tx.rollback().await?;
        return Err(TelmoniError::BadRequest(
            "cannot modify the project owner's role".into(),
        ));
    }

    if !members::update_role(&mut acting.tx, &project_id, &member_id, role).await? {
        return Err(AuthError::NotFound("no such member".into()).into());
    }
    emit_audit(
        &mut acting.tx,
        AuditEvent::new(
            &acting.organization,
            Actor::User(actor.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Member,
        )
        .in_project(&project_id)
        .on(member_id.as_str())
        .details(json!({ "role": role })),
    )
    .await?;
    acting.tx.commit().await?;

    tracing::info!(project_id = %project_id, member = %member_id, %role, "member role changed");
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /internal/v1/projects/{project_id}/members/{member_id}` — the owner
/// or an admin removes somebody.
pub async fn remove_member(
    State(state): State<Arc<AppState>>,
    Path((project_id, member_id)): Path<(ProjectId, UserId)>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Delete, Resource::Member)?;
    let acting = acting
        .hold(&project_id, &actor, Verb::Delete, Resource::Member)
        .await?;

    let (mut acting, owner, leaver_name) = is_organization_owner(acting, &member_id).await?;
    if owner {
        acting.tx.rollback().await?;
        return Err(TelmoniError::BadRequest(
            "cannot remove the project owner".into(),
        ));
    }

    if !members::remove(&mut acting.tx, &project_id, &member_id).await? {
        return Err(AuthError::NotFound("no such member".into()).into());
    }
    emit_audit(
        &mut acting.tx,
        AuditEvent::new(
            &acting.organization,
            Actor::User(actor.as_str()),
            AuditAction::Deleted,
            TelmoniResourceKind::Member,
        )
        .in_project(&project_id)
        .on(member_id.as_str())
        .details(json!({ "kind": AuditKind::RemovedByOwner })),
    )
    .await?;
    let organization = acting.organization.clone();
    acting.tx.commit().await?;

    let name = leaver_name.as_deref().unwrap_or("A member");
    crate::notify::emit_member_left(&state, &project_id, &organization, &member_id, name).await;

    tracing::info!(project_id = %project_id, member = %member_id, "member removed");
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /internal/v1/memberships/{project_id}` — the member gives up their
/// seat. Maintenance lane: the seat is a row they may only read.
pub async fn leave(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let not_a_member = || -> TelmoniError {
        AuthError::NotFound("you are not a member of this project".into()).into()
    };

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    // A missing project answers as a seat the caller does not hold: a stranger learns nothing.
    let Some(owner) = projects::organization_of(&mut tx, &project_id).await? else {
        return Err(not_a_member());
    };
    if organization_members::role_on_organization(&mut tx, &owner, &actor).await?
        == Some(OrganizationRole::Owner)
    {
        return Err(AuthzError::Forbidden(
            "you own this project — delete it instead of leaving it".into(),
        )
        .into());
    }
    // The seat before the lock, so a stranger never stands in the organization's queue.
    if members::role_on(&mut tx, &project_id, &actor)
        .await?
        .is_none()
    {
        return Err(not_a_member());
    }
    if hold_project(&mut tx, &owner, &project_id, &actor)
        .await?
        .is_none()
    {
        return Err(not_a_member());
    }
    let leaver_name = organization_members::member_display_name(&mut tx, &owner, &actor).await?;

    if !members::remove(&mut tx, &project_id, &actor).await? {
        return Err(not_a_member());
    }
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &owner,
            Actor::User(actor.as_str()),
            AuditAction::Deleted,
            TelmoniResourceKind::Member,
        )
        .in_project(&project_id)
        .on(actor.as_str())
        .details(json!({ "kind": AuditKind::Left })),
    )
    .await?;
    tx.commit().await?;

    let name = leaver_name.as_deref().unwrap_or("A member");
    crate::notify::emit_member_left(&state, &project_id, &owner, &actor, name).await;

    tracing::info!(project_id = %project_id, member = %actor, "member left");
    Ok(StatusCode::NO_CONTENT)
}
