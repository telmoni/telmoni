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

use axum::{extract::State, http::StatusCode, response::IntoResponse};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{Maintenance, Scoped, maintenance_scope};
use telmoni_shared::extract::{Json, Path};
use telmoni_shared::person_token::Principal;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, MemberCeiling, OrganizationId, OrganizationRole,
    OrganizationStatus, ProjectId, Role, TelmoniError, TelmoniResourceKind, UserId,
};

use crate::db::invites::InviteTarget;
use crate::{
    AppState,
    db::{AuthLane, identities, invites, locks, members, organization_members, organizations},
    handler::{
        acting_project,
        answer::{InviteCreated, InviteRevoked, Invites},
        authorize, lock_membership,
        member::grantable,
        refuse_unless_still_in,
    },
    model::{AuditKind, InviteScope},
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

/// What the deployment says before a seat is promised or taken: a paused
/// organization's refusal now, and the ceiling its roster is held to once
/// the count is read ([`admit_invitation`], [`admit_seat`]). Nothing in a
/// binary that sells no plans. ⚠ Asked before the organization's lock is
/// taken, never under it: the deployment answers on its own pool, and the
/// roster's lanes queue behind that lock.
pub(crate) async fn ceiling_of(
    state: &AppState,
    organization: &OrganizationId,
) -> Result<Option<MemberCeiling>, TelmoniError> {
    let Some(entitlements) = state.siblings.entitlements.as_deref() else {
        return Ok(None);
    };
    telmoni_shared::seam::admit_new(entitlements, organization).await?;
    Ok(telmoni_shared::seam::member_ceiling(entitlements, organization).await)
}

/// A count as SQL answers it; a negative one, which no table gives, fails
/// closed.
fn count(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Hold a promised seat to the ceiling: the roster and everyone invited,
/// read in the lane, the one binding that sees every project's
/// invitations. An invitation to somebody already among them — on the
/// roster, or invited and asked again — promises nobody a further seat and
/// is never refused by it.
pub(crate) async fn admit_invitation(
    state: &AppState,
    organization: &OrganizationId,
    ceiling: Option<MemberCeiling>,
    email: &str,
) -> Result<(), TelmoniError> {
    let Some(ceiling) = ceiling else {
        return Ok(());
    };
    let mut lane = maintenance_scope(&state.db, AuthLane).await?;
    let held = organization_members::held(&mut lane, organization, Some(email)).await?;
    lane.commit().await?;
    if held.names_email {
        return Ok(());
    }
    ceiling.admit(count(held.people))
}

/// Hold a seat being taken to the ceiling, under the organization's lock:
/// the roster alone counts here, since the seat is the promise kept, and an
/// invitation sent while the organization held more than it holds now seats
/// nobody past it. Only for somebody not on the roster yet: a member joining
/// a second project takes no seat.
async fn admit_seat(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    organization: &OrganizationId,
    ceiling: Option<MemberCeiling>,
) -> Result<(), TelmoniError> {
    let Some(ceiling) = ceiling else {
        return Ok(());
    };
    let seated = organization_members::seated(tx, organization).await?;
    ceiling.admit(count(seated))
}

/// The ceiling as a roster page reads it: `null` where none is held.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RosterCeiling {
    /// How many the organization may hold, those invited among them.
    pub limit: u32,
    /// How many it holds against that: its members and everyone invited.
    pub held: u64,
    /// Whether an invitation would be refused now: the seam's own rule, so
    /// no page restates it.
    pub reached: bool,
    /// The sentence an invitation past it is refused with.
    pub detail: String,
}

/// The organization's ceiling and what stands against it, for the roster
/// pages, which hide the invitation they know would be refused.
pub(crate) async fn roster_ceiling(
    state: &AppState,
    organization: &OrganizationId,
) -> Result<Option<RosterCeiling>, TelmoniError> {
    let Some(entitlements) = state.siblings.entitlements.as_deref() else {
        return Ok(None);
    };
    let Some(ceiling) = telmoni_shared::seam::member_ceiling(entitlements, organization).await
    else {
        return Ok(None);
    };
    let mut lane = maintenance_scope(&state.db, AuthLane).await?;
    let held = organization_members::held(&mut lane, organization, None).await?;
    lane.commit().await?;
    let held = count(held.people);
    Ok(Some(RosterCeiling {
        limit: ceiling.limit,
        held,
        reached: ceiling.reached(held),
        detail: ceiling.detail,
    }))
}

/// `POST /internal/v1/projects/{project_id}/invites` — offer somebody a role.
pub async fn create_invite(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
    Json(req): Json<CreateInviteRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;
    let role = grantable(req.role)?;
    let email = crate::identity::validate_email(&req.email)?;

    let acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Create, Resource::Member)?;
    let organization = acting.organization.clone();
    // A paused organization takes no new members, and one at its ceiling
    // promises no further seat.
    let ceiling = ceiling_of(&state, &organization).await?;
    admit_invitation(&state, &organization, ceiling, &email).await?;
    // Held in its organization, whose flags, roster, name and chain the rest reads and writes.
    let acting = acting
        .hold(&project_id, &actor, Verb::Create, Resource::Member)
        .await?;

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

    let inviter = identities::contact(&mut acting.tx, &actor).await?;
    if inviter.as_ref().is_some_and(|c| c.email == email) {
        return Err(TelmoniError::BadRequest(
            "that is your own address — you are already in this project".into(),
        ));
    }
    if invites::already_a_member_by_email(&mut acting.tx, &project_id, &email).await? {
        return Err(AuthError::Conflict("that address is already a member".into()).into());
    }
    // The invitation's mail calls the organization by its name.
    let Some(name) = organizations::get(&mut acting.tx, &organization)
        .await?
        .map(|o| o.name)
    else {
        return Err(AuthError::NotFound("organization not found".into()).into());
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
        AuditEvent::new(
            &organization,
            Actor::User(actor.as_str()),
            AuditAction::Created,
            TelmoniResourceKind::Member,
        )
        .in_project(&project_id)
        .on(invite_id.to_string().as_str())
        .details(json!({
            "kind": AuditKind::Invited,
            "email": email,
            "role": role,
        })),
    )
    .await?;
    acting.tx.commit().await?;

    let link = invite_link(&state, &secret);
    let label = name;
    let inviter = inviter.map_or_else(|| label.clone(), |c| c.display());
    if let Err(e) = state
        .mailer
        .send_member_invite(
            &email,
            &inviter,
            &label,
            crate::mailer::InvitedTo::Project(role),
            &link,
        )
        .await
    {
        tracing::warn!(invite = %invite_id, error = %e, "invitation mail failed");
    }

    tracing::info!(project_id = %project_id, invite = %invite_id, %role, "member invited");
    Ok((
        StatusCode::CREATED,
        Json(InviteCreated {
            id: invite_id,
            link,
            expires_at,
            owner_organization_id: organization,
        }),
    ))
}

/// `GET /internal/v1/projects/{project_id}/invites` — what is still outstanding.
pub async fn list_invites(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let mut acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Read, Resource::Member)?;
    let rows = invites::list_pending(&mut acting.tx, &project_id).await?;
    acting.tx.commit().await?;

    Ok(Json(Invites { invites: rows }))
}

/// `DELETE /internal/v1/projects/{project_id}/invites/{invite_id}` — take the
/// offer back. The link dies immediately, because accept reads liveness.
pub async fn revoke_invite(
    State(state): State<Arc<AppState>>,
    Path((project_id, invite_id)): Path<(ProjectId, Uuid)>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Delete, Resource::Member)?;
    let mut acting = acting
        .hold(&project_id, &actor, Verb::Delete, Resource::Member)
        .await?;

    let Some(email) = invites::revoke(&mut acting.tx, &project_id, invite_id).await? else {
        return Err(AuthError::NotFound("no such invitation".into()).into());
    };
    let owner = acting.organization.clone();
    emit_audit(
        &mut acting.tx,
        AuditEvent::new(
            &owner,
            Actor::User(actor.as_str()),
            AuditAction::Deleted,
            TelmoniResourceKind::Member,
        )
        .in_project(&project_id)
        .on(invite_id.to_string().as_str())
        .details(json!({ "kind": AuditKind::InviteRevoked })),
    )
    .await?;
    acting.tx.commit().await?;

    tracing::info!(project_id = %project_id, invite = %invite_id, "invitation revoked");
    Ok(Json(InviteRevoked {
        invite_id,
        email,
        owner_organization_id: owner,
    }))
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

/// What an invitation's link opens: the roster it seats somebody on, who sent
/// it from which organization, to whom, and at which role.
#[derive(Debug, Serialize)]
pub struct InviteLook {
    pub scope: InviteScope,
    pub inviter: String,
    pub organization: String,
    pub email: String,
    pub role: &'static str,
}

/// `POST /internal/v1/invites/look` — what does this link open?
pub async fn look_up_invite(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InviteTokenRequest>,
) -> Result<Json<InviteLook>, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let token_hash = hash_invite_token(&req.token);
    let found = invites::find_live_by_token(&mut tx, &token_hash).await?;
    tx.commit().await?;
    let Some(invite) = found else {
        return Err(
            AuthError::NotFound("this invitation has expired or been withdrawn".into()).into(),
        );
    };
    // ⚠ `scope` says which roster the link seats somebody on. The two
    // ladders spell their roles the same, so the role alone cannot.
    Ok(Json(InviteLook {
        scope: invite.target.scope(),
        inviter: inviter_label(
            invite.inviter_email.as_deref(),
            invite.inviter_display_name.as_deref(),
            &invite.organization_name,
        ),
        organization: invite.organization_name,
        email: invite.email,
        role: invite.target.role_word(),
    }))
}

/// An invitation both lanes have already decided to honour.
struct Accepted<'a> {
    id: Uuid,
    /// The organization it is recorded on.
    organization_id: &'a OrganizationId,
    target: &'a InviteTarget,
}

/// Seat an accepted invitation on the roster it names, **commit**, and
/// announce a project's.
async fn seat(
    state: &AppState,
    tx: Scoped<'_, Maintenance<AuthLane>>,
    invite: Accepted<'_>,
    ceiling: Option<MemberCeiling>,
    actor: &UserId,
    accepter: &str,
) -> Result<(), TelmoniError> {
    match invite.target {
        InviteTarget::Project { project_id, role } => {
            seat_project_member(
                state,
                tx,
                AcceptedProjectInvite {
                    id: invite.id,
                    project_id,
                    organization_id: invite.organization_id,
                    role: *role,
                },
                ceiling,
                actor,
                accepter,
            )
            .await
        }
        InviteTarget::Organization { role } => {
            seat_organization_member(
                tx,
                AcceptedOrganizationInvite {
                    id: invite.id,
                    organization_id: invite.organization_id,
                    role: *role,
                },
                ceiling,
                actor,
            )
            .await
        }
    }
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
/// already been confirmed. A wait the role's `lock_timeout` ends is a 409 to
/// try again ([`lock_membership`]).
async fn lock_active(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    organization: &OrganizationId,
) -> Result<(), TelmoniError> {
    lock_membership(tx.conn(), organization).await?;
    if organizations::status(tx, organization).await? != Some(OrganizationStatus::Active) {
        return Err(AuthError::Conflict(
            "this organization is being deleted — it can take no new members".into(),
        )
        .into());
    }
    Ok(())
}

/// Seat an accepted project invitation, **commit**, and announce it. The
/// `ceiling` is the deployment's answer for the organization, asked by the
/// caller before the organization's lock ([`ceiling_of`]).
async fn seat_project_member(
    state: &AppState,
    mut tx: Scoped<'_, Maintenance<AuthLane>>,
    invite: AcceptedProjectInvite<'_>,
    ceiling: Option<MemberCeiling>,
    actor: &UserId,
    accepter: &str,
) -> Result<(), TelmoniError> {
    let organization = invite.organization_id;
    lock_active(&mut tx, organization).await?;
    // Where the project is, read again under the lock: the invitation was read before it, and
    // a move committed meanwhile would seat the person on the project but enrol them here.
    refuse_unless_still_in(&mut tx, organization, invite.project_id).await?;
    match organization_members::role_on_organization(&mut tx, organization, actor).await? {
        Some(OrganizationRole::Owner) => {
            return Err(TelmoniError::BadRequest(
                "you own this project — there is nothing to accept".into(),
            ));
        }
        // Already on the roster: a second project is no further seat.
        Some(_) => {}
        None => admit_seat(&mut tx, organization, ceiling).await?,
    }

    // A seat already held — given from the roster since the link was read,
    // which withdrew this invitation too — is said so, not as a used link.
    if members::role_on(&mut tx, invite.project_id, actor)
        .await?
        .is_some()
    {
        return Err(AuthError::Conflict("you are already a member of this project".into()).into());
    }
    if !invites::mark_accepted(&mut tx, invite.id, actor).await? {
        return Err(AuthError::Conflict("this invitation was already used".into()).into());
    }
    // Never the plain insert: a duplicate would abort the rest of the transaction.
    if !members::insert_if_absent(&mut tx, invite.project_id, actor, invite.role, actor).await? {
        return Err(AuthError::Conflict("you are already a member of this project".into()).into());
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
            AuditEvent::new(
                organization,
                Actor::User(actor.as_str()),
                AuditAction::Created,
                TelmoniResourceKind::Member,
            )
            .on(actor.as_str())
            .details(json!({
                "kind": AuditKind::EnrolledWithProjectInvite,
                "invite_id": invite.id,
                "role": OrganizationRole::Member,
                "scope": InviteScope::Organization,
            })),
        )
        .await?;
    }

    emit_audit(
        &mut tx,
        AuditEvent::new(
            organization,
            Actor::User(actor.as_str()),
            AuditAction::Created,
            TelmoniResourceKind::Member,
        )
        .in_project(invite.project_id)
        .on(actor.as_str())
        .details(json!({
            "kind": AuditKind::InviteAccepted,
            "invite_id": invite.id,
            "role": invite.role,
        })),
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
/// the invitation stays unaccepted; so is a seat past the organization's
/// `ceiling`, the deployment's answer, asked by the caller before the
/// organization's lock ([`ceiling_of`]).
async fn seat_organization_member(
    mut tx: Scoped<'_, Maintenance<AuthLane>>,
    invite: AcceptedOrganizationInvite<'_>,
    ceiling: Option<MemberCeiling>,
    actor: &UserId,
) -> Result<(), TelmoniError> {
    lock_active(&mut tx, invite.organization_id).await?;
    if organization_members::role_on_organization(&mut tx, invite.organization_id, actor)
        .await?
        .is_some()
    {
        return Err(AuthError::Conflict("you are already in this organization".into()).into());
    }
    admit_seat(&mut tx, invite.organization_id, ceiling).await?;
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
        AuditEvent::new(
            invite.organization_id,
            Actor::User(actor.as_str()),
            AuditAction::Created,
            TelmoniResourceKind::Member,
        )
        .on(actor.as_str())
        .details(json!({
            "kind": AuditKind::OrganizationInviteAccepted,
            "invite_id": invite.id,
            "role": invite.role,
            "scope": InviteScope::Organization,
        })),
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

/// `POST /internal/v1/invites/accept` — spend the link, and become a member.
pub async fn accept_invite(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Json(req): Json<InviteTokenRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let token_hash = hash_invite_token(&req.token);
    let Some(invite) = invites::find_live_by_token(&mut tx, &token_hash).await? else {
        return Err(
            AuthError::NotFound("this invitation has expired or been withdrawn".into()).into(),
        );
    };
    let ceiling = ceiling_of(&state, &invite.organization_id).await?;
    let me = accepter(&mut tx, &actor).await?;
    if me.email != invite.email {
        return Err(not_yours());
    }
    seat(
        &state,
        tx,
        Accepted {
            id: invite.id,
            organization_id: &invite.organization_id,
            target: &invite.target,
        },
        ceiling,
        &actor,
        &crate::identity::display_for(me.display_name.as_deref(), &me.email),
    )
    .await?;

    tracing::info!(
        organization_id = %invite.organization_id,
        scope = %invite.target.scope(),
        member = %actor,
        role = invite.target.role_word(),
        "invitation accepted"
    );
    let on = match &invite.target {
        InviteTarget::Project { project_id, .. } => SeatedOn::ProjectId(project_id.clone()),
        InviteTarget::Organization { .. } => {
            SeatedOn::OrganizationId(invite.organization_id.clone())
        }
    };
    Ok((
        StatusCode::CREATED,
        Json(Seated {
            on,
            invite_id: invite.id,
            inviter_email: invite.inviter_email,
            owner_organization_id: invite.organization_id,
        }),
    ))
}

/// The roster an invitation seated somebody on, under its own key.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SeatedOn {
    ProjectId(ProjectId),
    OrganizationId(OrganizationId),
}

/// What an invitation accepted by its link answers: the roster seated, who
/// sent it, and whose console to tell.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Seated {
    #[serde(flatten)]
    pub on: SeatedOn,
    pub invite_id: Uuid,
    pub inviter_email: Option<String>,
    pub owner_organization_id: OrganizationId,
}

/// The URL that opens an invitation, on the console this service belongs to.
fn invite_link(state: &AppState, secret: &str) -> String {
    format!(
        "{}/invite/{secret}",
        state.config.app_url.trim_end_matches('/')
    )
}

/// `GET /internal/v1/me/invites` — list pending invitations addressed to the caller.
pub async fn list_my_incoming_invites(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let me = verified_accepter(&mut tx, &actor).await?;
    let rows = invites::list_incoming(&mut tx, &me.email, &actor).await?;
    tx.commit().await?;

    Ok(Json(Invites { invites: rows }))
}

/// `POST /internal/v1/me/invites/{invite_id}/accept` — accept an incoming invitation directly.
pub async fn accept_my_incoming_invite(
    State(state): State<Arc<AppState>>,
    Path(invite_id): Path<Uuid>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let me = verified_accepter(&mut tx, &actor).await?;
    let Some(invite) = invites::find_incoming(&mut tx, invite_id, &me.email).await? else {
        return Err(AuthError::NotFound("invitation not found or no longer active".into()).into());
    };
    let ceiling = ceiling_of(&state, &invite.owner_organization_id).await?;
    seat(
        &state,
        tx,
        Accepted {
            id: invite.id,
            organization_id: &invite.owner_organization_id,
            target: &invite.target,
        },
        ceiling,
        &actor,
        &crate::identity::display_for(me.display_name.as_deref(), &me.email),
    )
    .await?;

    tracing::info!(
        organization_id = %invite.owner_organization_id,
        scope = %invite.target.scope(),
        member = %actor,
        role = invite.target.role_word(),
        "incoming invitation accepted"
    );
    Ok((StatusCode::OK, Json(settled(Settlement::Accepted, &invite))))
}

/// Whether an incoming invitation was taken up or turned down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Settlement {
    Accepted,
    Declined,
}

/// What the console is answered once an incoming invitation is settled: who
/// to tell, and which organization's console.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settled {
    pub status: Settlement,
    pub scope: InviteScope,
    pub target_id: String,
    pub invite_id: Uuid,
    pub inviter_email: Option<String>,
    pub owner_organization_id: OrganizationId,
}

fn settled(status: Settlement, invite: &invites::Pending) -> Settled {
    Settled {
        status,
        scope: invite.target.scope(),
        target_id: target_id(invite),
        invite_id: invite.id,
        inviter_email: invite.inviter_email.clone(),
        owner_organization_id: invite.owner_organization_id.clone(),
    }
}

/// What the invitation seats somebody on, by id: the project, or the
/// organization it is recorded on.
fn target_id(invite: &invites::Pending) -> String {
    match &invite.target {
        InviteTarget::Project { project_id, .. } => project_id.to_string(),
        InviteTarget::Organization { .. } => invite.owner_organization_id.to_string(),
    }
}

/// `POST /internal/v1/me/invites/{invite_id}/decline` — decline an incoming invitation.
pub async fn decline_my_incoming_invite(
    State(state): State<Arc<AppState>>,
    Path(invite_id): Path<Uuid>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let me = verified_accepter(&mut tx, &actor).await?;
    let Some(invite) = invites::find_incoming(&mut tx, invite_id, &me.email).await? else {
        return Err(AuthError::NotFound("invitation not found or no longer active".into()).into());
    };
    let owner = &invite.owner_organization_id;
    // A project's: the organization it is recorded on held, and the project's
    // place in it read again under the lock, after the person's as the
    // accepts take them.
    if let InviteTarget::Project { project_id, .. } = &invite.target {
        lock_membership(tx.conn(), owner).await?;
        refuse_unless_still_in(&mut tx, owner, project_id).await?;
    }
    if !invites::decline_incoming(&mut tx, &invite).await? {
        return Err(AuthError::Conflict("this invitation was already resolved".into()).into());
    }
    let (in_project, details) = match &invite.target {
        InviteTarget::Project { project_id, .. } => (
            Some(project_id),
            json!({ "kind": AuditKind::InviteDeclined }),
        ),
        InviteTarget::Organization { .. } => (
            None,
            json!({
                "kind": AuditKind::OrganizationInviteDeclined,
                "scope": InviteScope::Organization,
            }),
        ),
    };
    let resource = invite_id.to_string();
    emit_audit(
        &mut tx,
        AuditEvent {
            in_project,
            ..AuditEvent::new(
                owner,
                Actor::User(actor.as_str()),
                AuditAction::Deleted,
                TelmoniResourceKind::Member,
            )
            .on(&resource)
            .details(details)
        },
    )
    .await?;
    tx.commit().await?;
    tracing::info!(
        organization_id = %owner,
        scope = %invite.target.scope(),
        invite = %invite_id,
        member = %actor,
        "invitation declined"
    );
    Ok(Json(settled(Settlement::Declined, &invite)))
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
