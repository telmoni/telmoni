//! Handing a project over — `/internal/v1/projects/{project_id}/transfer`.
//!
//! A project's owner is its organization's owner, so handing a project over
//! means moving it into an organization the new owner owns. The owner offers
//! it to one of the project's ADMINS; the admin accepts it into an
//! organization they own, or declines; the owner can withdraw it. On accept
//! the project row changes organization in one transaction: its API keys
//! follow (`api_tokens_project_fkey` cascades), its seats and invitations stay
//! on it and every seat holder is enrolled on the new organization's roster
//! as a project invitation would enrol them, the acceptor's seat is folded
//! into the ownership they now hold, and the previous owner is seated as an
//! admin — the mirror of the organization handover, where the previous owner
//! stays on as an admin. Its connectors do
//! not come with it: they are the old organization's vendor grants and webhook
//! secrets, purged in notifications before the move commits; the agent finds
//! what it holds of the project under the old organization and removes it
//! within the hour. Its telemetry does come with it: a span carries its
//! project and nothing above it, and the project's settings, its content mode
//! among them, move to the new organization before the commit. The audit chains
//! stay where they are — the old organization's keeps the project's history
//! and the new one's starts with its arrival — because a chain cannot be
//! spliced. A withdrawal, a replacement and an answer are mailed to whoever
//! the offer leaves waiting, as the organization's are.
//!
//! One live offer per project, dead after `OFFER_TTL_DAYS`. It lives on the
//! admin's seat (`transfer_offered_at`), so removing them, their leaving, or
//! demoting them ends it with no bookkeeping of its own.
//!
//! The owner of two organizations moves a project between them without an
//! offer — `/internal/v1/projects/{project_id}/move` — the accept's own steps with
//! nobody to ask and nothing changing hands.
//!
//! ⚠ **Offer, withdraw and decline take the owning organization's lock and refuse a project
//! moved while they waited for it; accept takes the caller's lock, then both organizations' by
//! id**, as account deletion does; every lane reads the roles again under its locks (`db::locks`).

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit, lock_chain};
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{
    Maintenance, ProjectAndOrganization, Scoped, maintenance_scope,
};
use telmoni_shared::error::lock_timed_out;
use telmoni_shared::extract::{Json, Path};
use telmoni_shared::person_token::Principal;
use telmoni_shared::slug::Slug;
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, OrganizationId, OrganizationRole, OrganizationStatus,
    ProjectId, Role, TelmoniError, TelmoniResourceKind, UserId,
};

use super::answer::Declined;
use super::handover::{OfferAnswer, OfferRequest, Offered, offer_expires_at};
use crate::{
    AppState,
    db::{
        AuthLane, identities, locks, members,
        offers::{self, Roster},
        organization_members, organizations, projects,
    },
    handler::{
        ActingProject, BEING_DELETED, acting_project, deletion, hold_project,
        projects::MAX_PROJECT_NAME, refuse_unless_active,
    },
    model::AuditKind,
};

/// Body of `POST /internal/v1/projects/{project_id}/transfer/accept`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptRequest {
    /// Which organization of the caller's the project lands in. Optional for
    /// somebody who owns exactly one; required, and refused unless theirs,
    /// for somebody who owns several.
    #[serde(default)]
    pub organization_id: Option<OrganizationId>,
}

/// The project's transaction, its organization bound and held ([`hold_project`]), and the
/// caller's role read before the lock, so no stranger queues on it, and again under it.
async fn locked<'a>(
    state: &'a AppState,
    project_id: &ProjectId,
    user_id: &UserId,
) -> Result<ActingProject<'a, ProjectAndOrganization>, TelmoniError> {
    let acting = acting_project(state, project_id, user_id).await?;
    let mut acting = acting.enter_owner_scope().await?;
    let organization = acting.organization.clone();
    let Some((role, organization_role)) =
        hold_project(&mut acting.tx, &organization, project_id, user_id).await?
    else {
        return Err(AuthzError::Forbidden("you are not a member of this project".into()).into());
    };
    refuse_unless_active(&mut acting.tx, &organization).await?;
    acting.role = role;
    acting.organization_role = organization_role;
    Ok(acting)
}

/// A name ending in a plain number from 2 up, as `"Payments 2"`: the part
/// before the number, and the number. `"Project 007"`, `"Project 1"` and
/// `"Project +5"` are not.
fn numbered(name: &str) -> Option<(&str, u32)> {
    let (base, digits) = name.rsplit_once(' ')?;
    let n: u32 = digits.parse().ok()?;
    let base = base.trim_end();
    (n >= 2 && n.to_string() == digits && !base.is_empty()).then_some((base, n))
}

/// The names a project may land under, best first: its own, then numbered
/// ones, each cut short to fit `MAX_PROJECT_NAME`. Two organizations name
/// their projects alike all the time — "Web", "API", "Payments" — so refusing
/// a clash would refuse the ordinary handover.
///
/// The numbers are `"{name} 2"`, `"{name} 3"`, … unless `family` names the
/// series the name already belongs to: `"Payments 2"` arriving where
/// "Payments" is continues as `"Payments 3"`. The caller only
/// passes a family whose base the destination holds, so `"Project 2024"`
/// becomes `"Project 2024 2"`, not a different year.
///
/// `held` is how many projects the destination holds. The numbered names end
/// in different numbers, so they differ from each other, and at most one
/// equals the name itself (a full-length name ending in " 2"): of the
/// `held + 2` names, one is free.
fn landing_names(wanted: &str, held: usize, family: Option<(&str, u32)>) -> Vec<String> {
    let (base, first) = family.map_or((wanted, 2), |(base, n)| (base, u64::from(n) + 1));
    std::iter::once(wanted.to_owned())
        .chain((first..).take(held + 1).map(|n| {
            let suffix = format!(" {n}");
            let room = MAX_PROJECT_NAME.saturating_sub(suffix.chars().count());
            let cut: String = base.chars().take(room).collect();
            format!("{}{suffix}", cut.trim_end())
        }))
        .collect()
}

/// The refusal an accept can still trip on the destination's name index, as
/// a 409 naming the project, not a 500: the destination's lock keeps creates
/// out, but a rename does not take it. It does not name the organization: the
/// page it is read on names it already.
fn name_taken_in(e: sqlx::Error, name: &str) -> TelmoniError {
    match e {
        sqlx::Error::Database(ref db)
            if db.constraint() == Some("projects_organization_name_key") =>
        {
            AuthError::Conflict(format!(
                "a project named \"{name}\" already exists in the organization it would land \
                 in — rename one of them first"
            ))
            .into()
        }
        other => other.into(),
    }
}

/// `POST /internal/v1/projects/{project_id}/transfer` — the owner offers the
/// project to one of its admins. Withdraws any offer already out, because
/// there is only ever one, and mails the admin who held it.
///
/// ⚠ **Nothing here asks whether the admin is deleting their account**, for
/// the reason the organization's offer does not: that is theirs, and a
/// refusal naming it would tell the owner. Accept refuses them under their
/// own lock.
pub async fn offer(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
    Json(req): Json<OfferRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;
    let target = req.member_id;
    if target == user_id {
        return Err(TelmoniError::BadRequest(
            "you already own this project".into(),
        ));
    }

    let mut acting = locked(&state, &project_id, &user_id).await?;
    if acting.role != Role::Owner {
        return Err(AuthzError::Forbidden(
            "only the project's owner — its organization's owner — can hand it over".into(),
        )
        .into());
    }
    match members::role_on(&mut acting.tx, &project_id, &target).await? {
        None => return Err(AuthError::NotFound("no such project member".into()).into()),
        Some(Role::Admin) => {}
        Some(_) => {
            return Err(AuthError::Conflict(
                "only an admin can be offered the project — make them an admin first".into(),
            )
            .into());
        }
    }
    let Some(standing) = projects::standing(&mut acting.tx, &project_id).await? else {
        return Err(AuthError::NotFound("project not found".into()).into());
    };
    let organization = acting.organization.clone();

    // Somebody else's live offer is withdrawn by this one, and they are told:
    // the chain names them, the console refreshes their notifications, and
    // they are mailed.
    let roster = Roster::Project(&project_id);
    let replaced = offers::live_holder(&mut acting.tx, roster)
        .await?
        .filter(|holder| *holder != target);
    offers::clear(&mut acting.tx, roster).await?;
    if !offers::offer_to(&mut acting.tx, roster, &target).await? {
        return Err(AuthError::Conflict("that member is no longer an admin".into()).into());
    }
    emit_audit(
        &mut acting.tx,
        AuditEvent::new(
            &organization,
            Actor::User(user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Project,
        )
        .in_project(&project_id)
        .on(project_id.as_str())
        .details(json!({
            "kind": AuditKind::ProjectOffered,
            "to": target.as_str(),
            "replaces": replaced.as_ref().map(UserId::as_str),
        })),
    )
    .await?;

    // Every seat holder is on the owning organization's roster, which is what
    // its scope may see of `auth.identities`.
    let recipient = identities::contact(&mut acting.tx, &target).await?;
    let owner = identities::contact(&mut acting.tx, &user_id).await?;
    let withdrawn_from_email = match &replaced {
        Some(holder) => identities::contact(&mut acting.tx, holder)
            .await?
            .map(|c| c.email),
        None => None,
    };
    let Some(organization_label) = organization_members::label_parts(&mut acting.tx, &organization)
        .await?
        .map(|parts| parts.name)
    else {
        return Err(AuthError::NotFound("organization not found".into()).into());
    };
    acting.tx.commit().await?;

    // For the console's live notices and the mail: from the roster, never from
    // the browser that asked.
    let recipient_email = recipient.map(|r| r.email);
    if let Some(owner) = owner.map(|o| o.display()) {
        Offered::Project {
            id: &project_id,
            name: &standing.name,
            organization: &organization_label,
        }
        .mail(
            &state,
            &owner,
            recipient_email.as_deref(),
            withdrawn_from_email.as_deref(),
        )
        .await;
    }

    tracing::info!(project_id = %project_id, to = %target, "project offered");
    Ok((
        StatusCode::OK,
        Json(OfferAnswer {
            offered_to: target,
            offered_to_email: recipient_email,
            withdrawn_from_email,
            expires_at: Some(offer_expires_at()),
            owner_organization_id: organization,
        }),
    ))
}

/// `DELETE /internal/v1/projects/{project_id}/transfer` — the owner withdraws
/// the offer, and the admin who held it is mailed.
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;

    let mut acting = locked(&state, &project_id, &user_id).await?;
    if acting.role != Role::Owner {
        return Err(AuthzError::Forbidden(
            "only the project's owner can withdraw its offer".into(),
        )
        .into());
    }
    let roster = Roster::Project(&project_id);
    let Some(holder) = offers::live_holder(&mut acting.tx, roster).await? else {
        return Err(AuthError::NotFound("there is no offer to withdraw".into()).into());
    };
    let organization = acting.organization.clone();
    offers::clear(&mut acting.tx, roster).await?;
    emit_audit(
        &mut acting.tx,
        AuditEvent::new(
            &organization,
            Actor::User(user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Project,
        )
        .in_project(&project_id)
        .on(project_id.as_str())
        .details(json!({
            "kind": AuditKind::ProjectOfferCancelled,
            "to": holder.as_str(),
        })),
    )
    .await?;
    let holder_email = identities::contact(&mut acting.tx, &holder)
        .await?
        .map(|c| c.email);
    let owner = identities::contact(&mut acting.tx, &user_id).await?;
    let standing = projects::standing(&mut acting.tx, &project_id).await?;
    acting.tx.commit().await?;

    if let (Some(to), Some(owner), Some(standing)) = (&holder_email, owner, standing)
        && let Err(e) = state
            .mailer
            .send_project_offer_withdrawn(to, &owner.display(), &standing.name)
            .await
    {
        // The withdrawal stands without the mail: their console no longer
        // lists the offer.
        tracing::warn!(project_id = %project_id, error = %e, "project offer withdrawal mail failed");
    }

    Ok(Json(OfferAnswer {
        offered_to: holder,
        offered_to_email: holder_email,
        withdrawn_from_email: None,
        expires_at: None,
        owner_organization_id: organization,
    }))
}

/// `POST /internal/v1/projects/{project_id}/transfer/decline` — the admin
/// holding the offer turns it down, and the owner is mailed.
pub async fn decline(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;

    let mut acting = locked(&state, &project_id, &user_id).await?;
    if !offers::decline(&mut acting.tx, Roster::Project(&project_id), &user_id).await? {
        return Err(AuthError::NotFound("you hold no offer for this project".into()).into());
    }
    let organization = acting.organization.clone();
    emit_audit(
        &mut acting.tx,
        AuditEvent::new(
            &organization,
            Actor::User(user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Project,
        )
        .in_project(&project_id)
        .on(project_id.as_str())
        .details(json!({ "kind": AuditKind::ProjectOfferDeclined })),
    )
    .await?;
    let owner = match organization_members::owner_of(&mut acting.tx, &organization).await? {
        Some(owner) => identities::contact(&mut acting.tx, &owner).await?,
        None => None,
    };
    let admin = identities::contact(&mut acting.tx, &user_id).await?;
    let standing = projects::standing(&mut acting.tx, &project_id).await?;
    acting.tx.commit().await?;

    if let (Some(owner), Some(admin), Some(standing)) = (owner, admin, standing)
        && let Err(e) = state
            .mailer
            .send_project_declined(&owner.email, &admin.display(), &standing.name)
            .await
    {
        // The decline stands without the mail: the owner's roster no longer
        // lists the offer.
        tracing::warn!(project_id = %project_id, error = %e, "project declined mail failed");
    }

    Ok(Json(Declined {
        owner_organization_id: organization,
    }))
}

/// Where a project landed, and the seats that came with it.
struct Landed {
    name: String,
    slug: Slug,
    organization_slug: Option<Slug>,
    staying: Vec<members::Seat>,
}

impl Landed {
    /// The name it had, when a clash in the destination renamed it on the way in.
    fn renamed_from<'a>(&self, standing: &'a projects::Standing) -> Option<&'a str> {
        (self.name != standing.name).then_some(standing.name.as_str())
    }
}

/// The steps an accept and a move share once their own checks pass, under
/// both organizations' locks: a free name in the destination, the move, the
/// seats marked seen, and the seats that stay, read before an accept seats
/// the previous owner so theirs is recorded under its own kind.
async fn land(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    project_id: &ProjectId,
    standing: &projects::Standing,
    destination: &OrganizationId,
) -> Result<Landed, TelmoniError> {
    let held = usize::try_from(projects::count_in(tx, destination).await?).unwrap_or(0);
    let family = match numbered(&standing.name) {
        Some((base, n)) => projects::holds_name(tx, destination, base)
            .await?
            .then_some((base, n)),
        None => None,
    };
    let candidates = landing_names(&standing.name, held, family);
    // `None` cannot happen (see `landing_names`); the move's own refusal
    // covers it if it ever does.
    let name = projects::first_free_name(tx, destination, &candidates)
        .await?
        .unwrap_or_else(|| standing.name.clone());
    let source = &standing.organization_id;
    let Some(slug) = projects::move_to_organization(tx, project_id, source, destination, &name)
        .await
        .map_err(|e| name_taken_in(e, &name))?
    else {
        return Err(AuthError::Conflict("the project is no longer where it was".into()).into());
    };
    members::mark_seen_on_project(tx, project_id).await?;
    let organization_slug = organizations::slug_of(tx, destination).await?;
    let staying = members::seats_on(tx, project_id).await?;
    Ok(Landed {
        name,
        slug,
        organization_slug,
        staying,
    })
}

/// Every seat holder on the destination's roster, as a project invitation
/// would enrol them: `/me` lists a seat only in an organization its holder
/// can open. Answers the seats whose holders were not on it already.
async fn enrol<'a>(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    destination: &OrganizationId,
    seats: &'a [members::Seat],
    by: &UserId,
) -> Result<Vec<&'a members::Seat>, TelmoniError> {
    let mut enrolled = Vec::new();
    for seat in seats {
        if organization_members::add_member(
            tx,
            destination,
            &seat.user_id,
            OrganizationRole::Member,
            by,
        )
        .await?
        {
            enrolled.push(seat);
        }
    }
    Ok(enrolled)
}

/// The project's move lock once every check has passed, so a lane a check
/// refuses never waits on an export, and the chain locks after it, so none
/// is held while it does.
async fn lock_for_audits(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    project_id: &ProjectId,
    source: &OrganizationId,
    destination: &OrganizationId,
) -> Result<(), TelmoniError> {
    lock_project_for_move(tx, project_id).await?;
    lock_transfer_chains(tx, source, destination).await
}

/// `details` with `renamed_from`, only when it happened: the console's audit
/// table prints metadata whole, and a `null` on every handover is noise.
fn renamed(mut details: serde_json::Value, renamed_from: Option<&str>) -> serde_json::Value {
    if let (Some(from), Some(fields)) = (renamed_from, details.as_object_mut()) {
        fields.insert("renamed_from".into(), json!(from));
    }
    details
}

struct TransferAudits<'a> {
    source: &'a OrganizationId,
    destination: &'a OrganizationId,
    project_id: &'a ProjectId,
    user_id: &'a UserId,
    previous: &'a UserId,
    project_name: &'a str,
    /// The name it was offered under, when a clash in the destination
    /// renamed it on the way in.
    renamed_from: Option<&'a str>,
    seated: bool,
    enrolled: bool,
    holders_enrolled: &'a [&'a members::Seat],
}

async fn emit_project_transfer_audits(
    conn: &mut sqlx::PgConnection,
    audits: TransferAudits<'_>,
) -> Result<(), TelmoniError> {
    // Both chains, the source's first, their locks taken already by id.
    emit_audit(
        conn,
        AuditEvent::new(
            audits.source,
            Actor::User(audits.user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Project,
        )
        .in_project(audits.project_id)
        .on(audits.project_id.as_str())
        .details(json!({
            "kind": AuditKind::ProjectTransferred,
            "to_organization": audits.destination.as_str(),
            "to": audits.user_id.as_str(),
            "from": audits.previous.as_str(),
        })),
    )
    .await?;
    emit_audit(
        conn,
        AuditEvent::new(
            audits.destination,
            Actor::User(audits.user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Project,
        )
        .in_project(audits.project_id)
        .on(audits.project_id.as_str())
        .details(renamed(
            json!({
                "kind": AuditKind::ProjectReceived,
                "from_organization": audits.source.as_str(),
                "from": audits.previous.as_str(),
                "to": audits.user_id.as_str(),
                "name": audits.project_name,
            }),
            audits.renamed_from,
        )),
    )
    .await?;
    // The seat the new owner held is folded into the ownership.
    emit_audit(
        conn,
        AuditEvent::new(
            audits.destination,
            Actor::User(audits.user_id.as_str()),
            AuditAction::Deleted,
            TelmoniResourceKind::Member,
        )
        .in_project(audits.project_id)
        .on(audits.user_id.as_str())
        .details(json!({
            "kind": AuditKind::SeatFoldedIntoOwnership,
            "role": Role::Admin,
        })),
    )
    .await?;
    if audits.seated {
        emit_audit(
            conn,
            AuditEvent::new(
                audits.destination,
                Actor::User(audits.user_id.as_str()),
                AuditAction::Created,
                TelmoniResourceKind::Member,
            )
            .in_project(audits.project_id)
            .on(audits.previous.as_str())
            .details(json!({
                "kind": AuditKind::PreviousOwnerSeated,
                "role": Role::Admin,
            })),
        )
        .await?;
    }
    if audits.enrolled {
        emit_audit(
            conn,
            AuditEvent::new(
                audits.destination,
                Actor::User(audits.user_id.as_str()),
                AuditAction::Created,
                TelmoniResourceKind::Member,
            )
            .on(audits.previous.as_str())
            .details(json!({
                "kind": AuditKind::PreviousOwnerEnrolled,
                "role": OrganizationRole::Member,
            })),
        )
        .await?;
    }
    emit_holders_enrolled(
        conn,
        audits.destination,
        audits.user_id,
        audits.holders_enrolled,
    )
    .await
}

/// Each seat holder's enrolment on the destination's roster, on its chain.
async fn emit_holders_enrolled(
    conn: &mut sqlx::PgConnection,
    destination: &OrganizationId,
    user_id: &UserId,
    holders: &[&members::Seat],
) -> Result<(), TelmoniError> {
    for seat in holders {
        emit_audit(
            conn,
            AuditEvent::new(
                destination,
                Actor::User(user_id.as_str()),
                AuditAction::Created,
                TelmoniResourceKind::Member,
            )
            .on(seat.user_id.as_str())
            .details(json!({
                "kind": AuditKind::SeatHolderEnrolled,
                "role": OrganizationRole::Member,
                "seat": seat.role,
            })),
        )
        .await?;
    }
    Ok(())
}

fn resolve_destination_organization(
    named: Option<OrganizationId>,
    owned: &[OrganizationId],
    source: &OrganizationId,
) -> Result<OrganizationId, TelmoniError> {
    let destination = match (named, owned) {
        (Some(named), owned) => {
            if !owned.contains(&named) {
                return Err(AuthzError::Forbidden(
                    "you do not own that organization — a project lands only in one of yours"
                        .into(),
                )
                .into());
            }
            named
        }
        (None, [only]) => only.clone(),
        (None, []) => {
            return Err(AuthError::Conflict(
                "you own no organization to take the project into".into(),
            )
            .into());
        }
        (None, _) => {
            return Err(TelmoniError::BadRequest(
                "you own several organizations — name the one to take the project into".into(),
            ));
        }
    };
    if destination == *source {
        return Err(TelmoniError::BadRequest(
            "the project is already in that organization".into(),
        ));
    }
    Ok(destination)
}

/// The two organizations by id, by byte as `str` compares: the order every
/// lane that locks several takes them in, so no two can cross into a deadlock.
fn by_id<'a>(
    source: &'a OrganizationId,
    destination: &'a OrganizationId,
) -> [&'a OrganizationId; 2] {
    if source.as_str() <= destination.as_str() {
        [source, destination]
    } else {
        [destination, source]
    }
}

async fn lock_transfer_organizations(
    conn: &mut sqlx::PgConnection,
    source: &OrganizationId,
    destination: &OrganizationId,
) -> Result<(), TelmoniError> {
    for organization in by_id(source, destination) {
        locks::lock_organization(conn, organization)
            .await
            .inspect_err(|e| {
                if lock_timed_out(e) {
                    tracing::warn!(
                        organization_id = %organization,
                        "a move of a project gave up waiting for an organization's lock"
                    );
                }
            })?;
    }
    Ok(())
}

/// Both organizations' audit chain locks, by id, before either audit row:
/// Slack's revocation takes several without the organizations' locks.
async fn lock_transfer_chains(
    conn: &mut sqlx::PgConnection,
    source: &OrganizationId,
    destination: &OrganizationId,
) -> Result<(), TelmoniError> {
    for organization in by_id(source, destination) {
        lock_chain(conn, organization).await.inspect_err(|e| {
            if lock_timed_out(e) {
                tracing::warn!(
                    organization_id = %organization,
                    "a move of a project gave up waiting for an organization's chain lock"
                );
            }
        })?;
    }
    Ok(())
}

/// The project's move lock, alone, for a move of its rows: a wait past the
/// `lock_timeout` — an export holding the project while it files the
/// project's settings row — answers 409, named here in the log.
async fn lock_project_for_move(
    conn: &mut sqlx::PgConnection,
    project_id: &ProjectId,
) -> Result<(), TelmoniError> {
    project_move::lock_for_move(conn, project_id)
        .await
        .inspect_err(|e| {
            if lock_timed_out(e) {
                tracing::warn!(
                    project_id = %project_id,
                    "a move of a project gave up waiting for the writes holding it in place"
                );
            }
        })?;
    Ok(())
}

/// `POST /internal/v1/projects/{project_id}/transfer/accept` — the admin holding
/// a live offer takes the project into an organization they own. The
/// previous owner keeps a seat on it, as an admin, and is mailed.
///
/// The one lane that writes across two tenants, so it runs in the
/// maintenance lane: `tenant_isolation` on `auth.projects` would refuse the
/// moved row under either organization's binding. Everything it decides on is
/// read again under the locks, and the seat is folded and the offer spent in
/// one statement whose `WHERE` re-checks the offer is still live; if that
/// matches nothing the transaction rolls back.
///
/// ⚠ **The caller's own lock first, then both organizations' by id** — the
/// order account deletion takes them in. Without the caller's, an accept
/// could land after their account deletion read what they own, and leave a
/// project in an organization whose owner can never sign in to hand it on.
/// Under the lock, somebody on their way out is refused.
///
/// ⚠ **The project's move lock, alone, once every check has passed and
/// before any chain lock**, held to the commit
/// ([`telmoni_shared::db::project_move`]): an accept a check refuses never
/// waits on an export, the accept waits for an export filing the project's
/// settings row — as long as the role's `lock_timeout` lets it, then a 409 —
/// and an export kept out meanwhile is tried again, filing the row under the
/// organization it then finds the project in.
///
/// ⚠ **Telemetry's move, then notifications' purge, then the commit, and a
/// step that fails moves nothing.** Telemetry's move of the project's
/// settings comes first because it can be undone, so a refused move leaves
/// everything as it was. Notifications' purge cannot be: the project's
/// connectors are the old organization's Slack grants and webhook
/// secrets, and a project arriving with them would post the new
/// owner's events into the old owner's channels. Only a purge that answered
/// `Ok` is followed by the commit. Until the commit answers, a guard
/// (`SettleTelemetry`) puts telemetry's settings back with the project,
/// wherever auth then holds it.
pub async fn accept(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
    Json(req): Json<AcceptRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;
    let named = req.organization_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;

    // Membership before any lock: the project's id arrives from the caller,
    // and a stranger must not stand in a queue holding a pooled connection.
    let Some(standing) = projects::standing(&mut tx, &project_id).await? else {
        return Err(AuthError::NotFound("project not found".into()).into());
    };
    let source = standing.organization_id.clone();
    if organization_members::role_on_organization(&mut tx, &source, &user_id).await?
        == Some(OrganizationRole::Owner)
    {
        return Err(TelmoniError::BadRequest(
            "you already own this project".into(),
        ));
    }
    if members::role_on(&mut tx, &project_id, &user_id)
        .await?
        .is_none()
    {
        return Err(AuthzError::Forbidden("you are not a member of this project".into()).into());
    }

    locks::lock_person(&mut tx, &user_id).await?;
    let Some(person) = identities::get(&mut tx, &user_id).await? else {
        return Err(AuthError::NotFound("sign in again to accept this offer".into()).into());
    };
    if person.deletion_requested_at.is_some() {
        return Err(AuthError::Conflict(
            "your account is being deleted — it cannot take a project on".into(),
        )
        .into());
    }

    let owned = organization_members::active_organizations_owned_by(&mut tx, &user_id).await?;
    let destination = resolve_destination_organization(named, &owned, &source)?;

    lock_transfer_organizations(&mut tx, &source, &destination).await?;

    // Under the locks, everything again.
    if organizations::status(&mut tx, &source).await? != Some(OrganizationStatus::Active) {
        return Err(AuthzError::Forbidden(BEING_DELETED.to_string()).into());
    }
    if organizations::status(&mut tx, &destination).await? != Some(OrganizationStatus::Active) {
        return Err(AuthzError::Forbidden("that organization is being deleted".into()).into());
    }
    if organization_members::role_on_organization(&mut tx, &destination, &user_id).await?
        != Some(OrganizationRole::Owner)
    {
        return Err(AuthError::Conflict("you no longer own that organization".into()).into());
    }
    let Some(previous) = organization_members::owner_of(&mut tx, &source).await? else {
        return Err(AuthError::Conflict(
            "the project's organization is changing hands — try again in a moment".into(),
        )
        .into());
    };
    // What the mails call the destination by.
    let Some(destination_label) = organization_members::label_parts(&mut tx, &destination)
        .await?
        .map(|parts| parts.name)
    else {
        return Err(AuthError::NotFound("organization not found".into()).into());
    };

    if !members::fold_offered_seat(&mut tx, &project_id, &user_id).await? {
        return Err(AuthError::Conflict(
            "this offer is no longer open — it was withdrawn, it lapsed, or you are no longer \
             an admin of the project"
                .into(),
        )
        .into());
    }
    let landed = land(&mut tx, &project_id, &standing, &destination).await?;
    // The previous owner keeps a seat, and with it a row on the destination's
    // roster, as a project invitation would give them: every seat holder is
    // on the owning organization's roster, or `/me` would list a seat in an
    // organization they cannot switch to and the console could never open it.
    let seated =
        members::insert_if_absent(&mut tx, &project_id, &previous, Role::Admin, &user_id).await?;
    let enrolled = organization_members::add_member(
        &mut tx,
        &destination,
        &previous,
        OrganizationRole::Member,
        &user_id,
    )
    .await?;
    let holders_enrolled = enrol(&mut tx, &destination, &landed.staying, &user_id).await?;

    lock_for_audits(&mut tx, &project_id, &source, &destination).await?;
    emit_project_transfer_audits(
        &mut tx,
        TransferAudits {
            source: &source,
            destination: &destination,
            project_id: &project_id,
            user_id: &user_id,
            previous: &previous,
            project_name: &landed.name,
            renamed_from: landed.renamed_from(&standing),
            seated,
            enrolled,
            holders_enrolled: &holders_enrolled,
        },
    )
    .await?;

    // The previous owner and the new one, for the mail: the lane may read both.
    let previous_contact = identities::get(&mut tx, &previous).await?;
    let new_owner = crate::identity::display_for(person.display_name.as_deref(), &person.email);

    move_purge_and_commit(&state, tx, &project_id, &source, &destination).await?;

    if let Some(previous_contact) = previous_contact
        && let Err(e) = state
            .mailer
            .send_project_accepted(
                &previous_contact.email,
                &new_owner,
                &standing.name,
                &destination_label,
            )
            .await
    {
        // The transfer stands without the mail: the previous owner's console
        // shows the project under its new organization from their next page.
        tracing::warn!(project_id = %project_id, error = %e, "project accepted mail failed");
    }

    tracing::info!(
        project_id = %project_id,
        from = %source,
        to = %destination,
        "project transferred"
    );
    Ok(Json(ProjectAccepted {
        moved: ProjectMoved {
            project_id,
            slug: landed.slug,
            organization_id: destination,
            organization_slug: landed.organization_slug,
            previous_organization_id: source,
            name: landed.name,
        },
        previous_owner: previous,
    }))
}

/// A project moved to another organization: where it landed, by the slugs
/// its pages go by, where it came from, and its name.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMoved {
    pub project_id: ProjectId,
    pub slug: Slug,
    pub organization_id: OrganizationId,
    pub organization_slug: Option<Slug>,
    pub previous_organization_id: OrganizationId,
    pub name: String,
}

/// A project taken over: the move, and who owned it until now.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAccepted {
    #[serde(flatten)]
    pub moved: ProjectMoved,
    pub previous_owner: UserId,
}

/// Body of `POST /internal/v1/projects/{project_id}/move`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveRequest {
    /// The organization of the caller's the project moves into.
    pub organization_id: OrganizationId,
}

/// `POST /internal/v1/projects/{project_id}/move` — the owner of two
/// organizations moves a project from one into the other, as an accept would
/// hand it over but with nobody to ask: its API keys follow, its telemetry
/// moves with it, its connectors are purged, its seats stay on it and their
/// holders join the destination's roster as members, and a live offer of it
/// ends. Nothing changes hands, so nothing is mailed. What a deployment calls
/// merging two organizations is this, once for each project, and the emptied
/// one deleted.
///
/// ⚠ **The caller's own lock first, then both organizations' by id**, as the
/// accept takes them, and every role and status read again under them; then
/// the project's move lock, as the accept takes it.
pub async fn move_project(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
    Json(req): Json<MoveRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;
    let destination = req.organization_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;

    // Ownership before any lock: the project's id arrives from the caller,
    // and a stranger must not stand in a queue holding a pooled connection.
    let Some(standing) = projects::standing(&mut tx, &project_id).await? else {
        return Err(AuthError::NotFound("project not found".into()).into());
    };
    let source = standing.organization_id.clone();
    if organization_members::role_on_organization(&mut tx, &source, &user_id).await?
        != Some(OrganizationRole::Owner)
    {
        return Err(AuthzError::Forbidden(
            "only the project's owner — its organization's owner — can move it".into(),
        )
        .into());
    }
    if destination == source {
        return Err(TelmoniError::BadRequest(
            "the project is already in that organization".into(),
        ));
    }

    locks::lock_person(&mut tx, &user_id).await?;
    let Some(person) = identities::get(&mut tx, &user_id).await? else {
        return Err(AuthError::NotFound("sign in again to move this project".into()).into());
    };
    if person.deletion_requested_at.is_some() {
        return Err(AuthError::Conflict(
            "your account is being deleted — it cannot move a project".into(),
        )
        .into());
    }
    let owned = organization_members::active_organizations_owned_by(&mut tx, &user_id).await?;
    if !owned.contains(&destination) {
        return Err(AuthzError::Forbidden(
            "you do not own that organization — a project moves only into one of yours".into(),
        )
        .into());
    }

    lock_transfer_organizations(&mut tx, &source, &destination).await?;

    // Under the locks, everything again.
    if organizations::status(&mut tx, &source).await? != Some(OrganizationStatus::Active) {
        return Err(AuthzError::Forbidden(BEING_DELETED.to_string()).into());
    }
    if organizations::status(&mut tx, &destination).await? != Some(OrganizationStatus::Active) {
        return Err(AuthzError::Forbidden("that organization is being deleted".into()).into());
    }
    for organization in [&source, &destination] {
        if organization_members::role_on_organization(&mut tx, organization, &user_id).await?
            != Some(OrganizationRole::Owner)
        {
            return Err(AuthError::Conflict("you no longer own both organizations".into()).into());
        }
    }

    // An offer of the project to one of its admins was an offer to take it
    // out of the organization it is leaving.
    offers::clear(&mut tx, Roster::Project(&project_id)).await?;
    let landed = land(&mut tx, &project_id, &standing, &destination).await?;
    let holders_enrolled = enrol(&mut tx, &destination, &landed.staying, &user_id).await?;

    lock_for_audits(&mut tx, &project_id, &source, &destination).await?;
    emit_project_move_audits(
        &mut tx,
        MoveAudits {
            source: &source,
            destination: &destination,
            project_id: &project_id,
            user_id: &user_id,
            project_name: &landed.name,
            renamed_from: landed.renamed_from(&standing),
            holders_enrolled: &holders_enrolled,
        },
    )
    .await?;

    move_purge_and_commit(&state, tx, &project_id, &source, &destination).await?;

    tracing::info!(
        project_id = %project_id,
        from = %source,
        to = %destination,
        "project moved"
    );
    Ok(Json(ProjectMoved {
        project_id,
        slug: landed.slug,
        organization_id: destination,
        organization_slug: landed.organization_slug,
        previous_organization_id: source,
        name: landed.name,
    }))
}

struct MoveAudits<'a> {
    source: &'a OrganizationId,
    destination: &'a OrganizationId,
    project_id: &'a ProjectId,
    user_id: &'a UserId,
    project_name: &'a str,
    /// The name it had, when a clash in the destination renamed it.
    renamed_from: Option<&'a str>,
    holders_enrolled: &'a [&'a members::Seat],
}

async fn emit_project_move_audits(
    conn: &mut sqlx::PgConnection,
    audits: MoveAudits<'_>,
) -> Result<(), TelmoniError> {
    // Both chains, the source's first, their locks taken already by id.
    emit_audit(
        conn,
        AuditEvent::new(
            audits.source,
            Actor::User(audits.user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Project,
        )
        .in_project(audits.project_id)
        .on(audits.project_id.as_str())
        .details(json!({
            "kind": AuditKind::ProjectMoved,
            "to_organization": audits.destination.as_str(),
        })),
    )
    .await?;
    emit_audit(
        conn,
        AuditEvent::new(
            audits.destination,
            Actor::User(audits.user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Project,
        )
        .in_project(audits.project_id)
        .on(audits.project_id.as_str())
        .details(renamed(
            json!({
                "kind": AuditKind::ProjectMoved,
                "from_organization": audits.source.as_str(),
                "name": audits.project_name,
            }),
            audits.renamed_from,
        )),
    )
    .await?;
    emit_holders_enrolled(
        conn,
        audits.destination,
        audits.user_id,
        audits.holders_enrolled,
    )
    .await
}

/// The last steps of an accept and a move, in order: telemetry's move, the
/// one that can be undone, then notifications' purge, which cannot, then the
/// commit. The transaction ends here whatever happens, rolled back before a
/// refusal is answered rather than when its connection goes back to the pool,
/// so the settling after finds its locks free; telemetry's settings are
/// settled ([`SettleTelemetry`]) when any step fails.
async fn move_purge_and_commit(
    state: &Arc<AppState>,
    tx: Scoped<'_, Maintenance<AuthLane>>,
    project_id: &ProjectId,
    source: &OrganizationId,
    destination: &OrganizationId,
) -> Result<(), TelmoniError> {
    let settle = SettleTelemetry::arm(state, project_id, source, destination);
    let ready = async {
        deletion::move_project_telemetry(state, project_id, destination).await?;
        deletion::purge_project_connectors(state, project_id, deletion::Teardown::ForMove).await
    }
    .await;
    if let Err(e) = ready {
        // A rollback that fails is a connection gone, whose close undoes it.
        if tx.rollback().await.is_err() {
            tracing::warn!(
                project_id = %project_id,
                "a refused transfer's rollback failed; its connection's close undoes it"
            );
        }
        settle.settle().await;
        return Err(e);
    }
    if let Err(e) = tx.commit().await {
        settle.settle().await;
        return Err(e.into());
    }
    settle.disarm();
    Ok(())
}

/// Telemetry's settings put back with their project, unless the accept
/// commits: armed before telemetry's move, disarmed once the commit answers.
///
/// ⚠ **Two stores cannot commit together.** The move may have landed before
/// a step after it failed, one over its budget may still land, and a commit
/// may land with its answer lost. So the settling reads where auth holds the
/// project, under both organizations' locks, as an accept takes them, and
/// moves the settings there under the project's move lock — right whichever
/// happened, a retry's accept in between included; a project moved on since,
/// beyond both, is the later accept's to settle. An export let in between
/// the two moves filed the project's settings row under the organization
/// the first one named: the row goes back with the rest. The request settles
/// telemetry itself before it answers, so a rollout that waits for
/// it waits for that too; a settling that fails, or a request dropped before
/// it could, leaves the guard armed, and its drop tries again in a task of
/// its own ([`SETTLE_RETRIES`]).
struct SettleTelemetry(Option<Settle>);

struct Settle {
    state: Arc<AppState>,
    project_id: ProjectId,
    source: OrganizationId,
    destination: OrganizationId,
}

/// The waits before each try of the task settling them: at once, then past
/// the locks' and the move's two seconds a few times over, since what failed
/// it is most often an accept or a commit still holding them; and last past
/// two minutes, the `idle_in_transaction_session_timeout` that frees the
/// locks of an accept whose connection died mid-commit.
const SETTLE_RETRIES: [std::time::Duration; 4] = [
    std::time::Duration::ZERO,
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(30),
    std::time::Duration::from_secs(150),
];

impl SettleTelemetry {
    /// Armed, unless no telemetry module holds settings to settle.
    fn arm(
        state: &Arc<AppState>,
        project_id: &ProjectId,
        source: &OrganizationId,
        destination: &OrganizationId,
    ) -> Self {
        Self(state.siblings.telemetry.is_some().then(|| Settle {
            state: state.clone(),
            project_id: project_id.clone(),
            source: source.clone(),
            destination: destination.clone(),
        }))
    }

    fn disarm(mut self) {
        self.0 = None;
    }

    /// Settle them in the request, once its transaction is spent; armed
    /// still if that fails, for the drop to try again.
    async fn settle(mut self) {
        if let Some(settle) = &self.0
            && settle.run().await.is_ok()
        {
            self.0 = None;
        }
    }
}

impl Drop for SettleTelemetry {
    fn drop(&mut self) {
        let Some(settle) = self.0.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!(
                project_id = %settle.project_id,
                "telemetry's settings could not be settled after a transfer that did not finish: no runtime"
            );
            return;
        };
        runtime.spawn(settle.run_until_settled());
    }
}

impl Settle {
    async fn run_until_settled(self) {
        let mut failure = None;
        for wait in SETTLE_RETRIES {
            tokio::time::sleep(wait).await;
            match self.run().await {
                Ok(()) => return,
                Err(e) => failure = Some(e),
            }
        }
        if let Some(e) = failure {
            tracing::error!(
                project_id = %self.project_id,
                retries = SETTLE_RETRIES.len(),
                error = %e,
                "telemetry's settings could not be settled after a transfer that did not finish"
            );
        }
    }

    async fn run(&self) -> Result<(), TelmoniError> {
        let mut tx = maintenance_scope(&self.state.db, AuthLane).await?;
        lock_transfer_organizations(&mut tx, &self.source, &self.destination).await?;
        // A project deleted since: its settings are its delete's to purge.
        let Some(standing) = projects::standing(&mut tx, &self.project_id).await? else {
            return Ok(());
        };
        // Moved on since, to an organization whose lock this does not hold:
        // the accept that moved it moved the settings too, and settles them
        // itself if it did not finish.
        if standing.organization_id != self.source && standing.organization_id != self.destination {
            return Ok(());
        }
        // So that no export files the project's settings row beside the
        // move, under the organization the project is leaving.
        lock_project_for_move(&mut tx, &self.project_id).await?;
        crate::handler::deletion::move_project_telemetry(
            &self.state,
            &self.project_id,
            &standing.organization_id,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_own_name_comes_first_then_the_numbers_in_order() {
        assert_eq!(
            landing_names("Default Project", 1, None),
            ["Default Project", "Default Project 2", "Default Project 3"]
        );
        assert_eq!(
            landing_names("Payments", 0, None),
            ["Payments", "Payments 2"]
        );
    }

    #[test]
    fn a_plain_trailing_number_from_two_up_is_a_series() {
        assert_eq!(numbered("Default Project 2"), Some(("Default Project", 2)));
        assert_eq!(numbered("Project 2024"), Some(("Project", 2024)));
        for not_numbered in [
            "Default Project",
            "Project 1",
            "Project 0",
            "Project 007",
            "Project +5",
            "Project -3",
            "Project 2.5",
            " 2",
            "2",
            "Project 99999999999",
        ] {
            assert_eq!(numbered(not_numbered), None, "{not_numbered:?}");
        }
    }

    /// "Default Project 2" carries on its series instead of growing a second
    /// number.
    #[test]
    fn a_name_in_a_series_continues_it() {
        let wanted = "Default Project 2";
        assert_eq!(
            landing_names(wanted, 2, numbered(wanted)),
            [
                "Default Project 2",
                "Default Project 3",
                "Default Project 4",
                "Default Project 5"
            ]
        );
        assert_eq!(
            landing_names("Project 2024", 0, None),
            ["Project 2024", "Project 2024 2"]
        );
        let top = format!("Project {}", u32::MAX);
        assert_eq!(
            landing_names(&top, 0, numbered(&top))[1],
            format!("Project {}", u64::from(u32::MAX) + 1)
        );
    }

    /// The guarantee the accept leans on: more distinct names than the
    /// destination holds, even when the name itself is one of the numbered
    /// ones and must not be counted twice.
    #[test]
    fn there_is_always_one_more_distinct_name_than_the_destination_holds() {
        let full_length_two = format!("{} 2", "x".repeat(MAX_PROJECT_NAME - 2));
        let full_length_series = format!("{} 99", "x".repeat(MAX_PROJECT_NAME - 3));
        for wanted in [
            "Default Project",
            "Default Project 2",
            full_length_two.as_str(),
            full_length_series.as_str(),
        ] {
            for family in [None, numbered(wanted)] {
                for held in [0, 1, 9, 10, 99] {
                    let names = landing_names(wanted, held, family);
                    let distinct: std::collections::HashSet<String> =
                        names.iter().map(|n| n.to_lowercase()).collect();
                    assert!(
                        distinct.len() > held,
                        "{wanted:?} in {family:?} with {held} held"
                    );
                }
            }
        }
    }

    #[test]
    fn every_name_fits_and_a_long_one_gives_up_characters_for_its_number() {
        let wanted = "x".repeat(MAX_PROJECT_NAME);
        for name in landing_names(&wanted, 120, None) {
            assert!(name.chars().count() <= MAX_PROJECT_NAME, "{name}");
        }
        assert_eq!(
            landing_names(&wanted, 0, None)[1],
            format!("{} 2", "x".repeat(MAX_PROJECT_NAME - 2))
        );
        let series = format!("{} 99", "x".repeat(MAX_PROJECT_NAME - 3));
        assert_eq!(
            landing_names(&series, 0, numbered(&series))[1],
            format!("{} 100", "x".repeat(MAX_PROJECT_NAME - 4))
        );
        let multibyte = "é".repeat(MAX_PROJECT_NAME);
        assert_eq!(
            landing_names(&multibyte, 0, None)[1].chars().count(),
            MAX_PROJECT_NAME
        );
    }

    /// A cut that lands on a space would leave two before the number.
    #[test]
    fn a_cut_short_name_does_not_end_in_a_double_space() {
        let wanted = format!("{} y", "x".repeat(MAX_PROJECT_NAME - 3));
        assert_eq!(
            landing_names(&wanted, 0, None)[1],
            format!("{} 2", "x".repeat(MAX_PROJECT_NAME - 3))
        );
    }
}
