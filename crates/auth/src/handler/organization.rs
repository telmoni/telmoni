//! The organization itself: its founding — the first one `/me` provisions, or
//! another a person asks for — its name, and its deletion by its owner. An
//! operator's termination, and the steps the deletion sweep drives it
//! through, are [`crate::sweep`]. Its name and its
//! URL are two settings: a new organization takes the URL asked for, or one
//! derived from the name it is born with, and after that a rename moves
//! nothing.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::OrganizationAction;
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::seam::{Closing, FoundedOrganization, OrganizationFounding, OwnershipChange};
use telmoni_shared::{
    AuthError, Flag, OrganizationId, OrganizationRole, TelmoniError, TenantError, UserId,
};

use super::authorize_organization;
use crate::{
    AppState,
    db::{
        AuthLane,
        confirmation_codes::{self, Act, Purpose},
        flags, identities, locks, organization_members, organizations, projects,
    },
    handler::{
        ActingOrganization,
        account::DeletionRequest,
        account::wrong_code,
        acting_organization,
        answer::Sent,
        deletion::{deletion_metadata, request_deletion},
        organization_of, organization_role_or_forbidden,
    },
    model::{AuditKind, DeletionKind},
};
use telmoni_shared::AuthzError;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{
    self, PersonAndOrganization, Scoped, maintenance_scope, person_scope,
};
use telmoni_shared::slug::{self, Slug};
use telmoni_shared::{AuditAction, TelmoniResourceKind};

/// The longest name worth storing: a real company with a qualifier, still
/// whole in a rail row rather than an ellipsis.
use crate::db::organizations::MAX_ORGANIZATION_NAME;

/// `POST /internal/v1/organization/deletion-code` — mail the owner the code that
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
    authorize_organization(role, OrganizationAction::Delete)?;
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
        &crate::issuer::hash(&code),
        expires_at,
    )
    .await?;
    tx.commit().await?;

    if let Err(e) = state
        .mailer
        .send_organization_deletion_code(&person.email, &code, &row.name)
        .await
    {
        tracing::error!(organization_id = %organization, error = %e,
            "organization-deletion code mail failed — check MAIL_FROM, the mail transport and the sending domain's DNS");
        return Err(TelmoniError::MailDelivery {
            context: "organization-deletion confirmation code".into(),
        });
    }

    tracing::info!(organization_id = %organization, "organization-deletion confirmation code issued");
    Ok((StatusCode::ACCEPTED, Json(Sent::MAILED)))
}

/// `DELETE /internal/v1/organization` — the owner deletes the organization in
/// `x-organization-id`, gated by the emailed code minted for it. It is marked
/// `pending_deletion` in one transaction — from which nothing acts in it and
/// none of its keys validates — then the purge hook runs inline, and the
/// sweep deletes the row once the grace has passed. 202 either way, naming
/// when the row goes. There is no restore. Nobody's account is touched: its
/// members simply stop being in it.
///
/// Its keys are not revoked, unlike an account deletion's: the status test
/// in `tokens::validate` refuses every key of a pending organization from
/// the mark, and the row's deletion cascades to them.
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
    let tx = open_deletion(&state, &principal.user_id, &organization).await?;
    let erase_after = spend_code(tx, &principal.user_id, &organization, &req.code).await?;
    Ok(crate::handler::deletion::deletion_response(&state, &organization, erase_after).await)
}

/// A deletion's transaction, under the organization's lock, once `user_id` is
/// read again there as the one who may delete it.
async fn open_deletion<'a>(
    state: &'a AppState,
    user_id: &UserId,
    organization: &OrganizationId,
) -> Result<Scoped<'a, PersonAndOrganization>, TelmoniError> {
    // Both keys bound: the person's for their code, the organization's for its
    // roster, its projects and its audit chain.
    let tx = person_scope(&state.db, user_id).await?;
    let mut tx = tx.bind_organization(organization).await?;
    // Membership before the lock, so a stranger naming any organization's id
    // cannot queue on its lock holding a pooled connection; the owner is then
    // read again under it.
    if organization_members::role_on_organization(&mut tx, organization, user_id)
        .await?
        .is_none()
    {
        return Err(
            AuthzError::Forbidden("you are not a member of this organization".into()).into(),
        );
    }
    locks::lock_organization(&mut tx, organization).await?;
    let role = organization_role_or_forbidden(&mut tx, organization, user_id).await?;
    authorize_organization(role, OrganizationAction::Delete)?;
    Ok(tx)
}

/// Spend `code` on the organization's deletion and mark it, answering when
/// the row goes. A wrong code is counted, and changes nothing else.
async fn spend_code(
    mut tx: Scoped<'_, PersonAndOrganization>,
    user_id: &UserId,
    organization: &OrganizationId,
    code: &str,
) -> Result<DateTime<Utc>, TelmoniError> {
    let Some(code_id) = confirmation_codes::find_live(
        &mut tx,
        user_id,
        Act::OrganizationDeletion(organization),
        &crate::issuer::hash(code.trim()),
    )
    .await?
    else {
        let burned =
            confirmation_codes::register_failure(&mut tx, user_id, Purpose::OrganizationDeletion)
                .await?;
        tx.commit().await?;
        return Err(wrong_code(burned));
    };
    confirmation_codes::consume(&mut tx, code_id).await?;

    let Some(erase_after) = request_deletion(&mut tx, organization, DeletionKind::Owner).await?
    else {
        // The role check above refused anything but an active organization,
        // under the same lock this still holds.
        return Err(TelmoniError::Internal(format!(
            "{organization} changed status under its lock"
        )));
    };
    emit_audit(
        &mut tx,
        AuditEvent::new(
            organization,
            Actor::User(user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Organization,
        )
        .on(organization.as_str())
        .details(deletion_metadata(
            "requested",
            DeletionKind::Owner,
            erase_after,
            None,
        )),
    )
    .await?;
    tx.commit().await?;

    tracing::info!(organization_id = %organization, erase_after = %erase_after,
        "organization deletion requested");
    Ok(erase_after)
}

/// [`delete_organization`] for a deployment's seam, unless a project is in
/// the organization: counted under the lock a project made or landing there
/// takes, and before the code is read, so the code stays good for the next
/// try. Its tail runs inline as the lane runs it; the sweep finishes a purge
/// that does not land.
pub(crate) async fn close_if_empty(
    state: &AppState,
    user_id: &UserId,
    organization: &OrganizationId,
    code: &str,
) -> Result<Closing, TelmoniError> {
    let mut tx = open_deletion(state, user_id, organization).await?;
    if projects::count_in(&mut tx, organization).await? > 0 {
        tx.rollback().await?;
        return Ok(Closing::Holding);
    }
    spend_code(tx, user_id, organization, code).await?;
    crate::handler::deletion::run_inline_tail(state, organization).await;
    Ok(Closing::Closed)
}

/// Body of `POST /internal/v1/organizations`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOrganizationRequest {
    pub name: String,
    /// Absent or blank, the URL is derived from the name, as a first
    /// organization's is.
    #[serde(default)]
    pub slug: Option<String>,
}

/// `POST /internal/v1/organizations` — the caller founds another organization,
/// as its owner. It names no existing organization, so it reads no
/// `x-organization-id`. Nothing caps how many a person makes or owns; the
/// global `signup` flag closes it, as it closes provisioning, and a closed one
/// is a refusal here, since the person asked. A deployment's ownership hook
/// is told once it is founded, and decides what, if anything, follows.
///
/// ⚠ **On the person's lock, which their account's deletion holds while it
/// reads what they own.** A creation that waited on it is refused once the
/// deletion lands, rather than adding an organization the deletion never saw.
///
/// ⚠ **The person's default organization stays where it was.** With none
/// chosen it is the oldest they own, else the oldest they belong to, so for
/// somebody who owns none the one founded here would take it over: their
/// current one is recorded as chosen instead, under the lock. A choice already
/// recorded is left alone, even one passed over while its organization waits
/// out a deletion.
pub async fn create_organization(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Json(req): Json<CreateOrganizationRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let founder = principal.user_id;
    let name = organization_name(&req.name)?;
    // A blank URL asks for none, as the console's field reads it: a client
    // that sends the field empty is not asking for a URL of no letters.
    let asked = req
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|asked| !asked.is_empty())
        .map(slug::asked_url)
        .transpose()?;
    if !signups_open(&state).await? {
        return Err(TenantError::FeatureOff { flag: Flag::Signup }.into());
    }
    // The deployment's say on a second organization at the person's asking:
    // one that has it paid for first refuses here, and founds it itself.
    crate::handler::ownership::may_found(&state, &founder).await?;
    // ⚠ MINTED: an organization is nobody's id.
    let organization = OrganizationId::new();
    let url = match asked {
        Some(asked) => NewUrl::Chosen(asked),
        None => {
            let candidates = slug::candidates(slug::Scope::Organization, &name);
            NewUrl::Derived(free_url(&state, &organization, &candidates).await?)
        }
    };

    let mut tx = person_scope(&state.db, &founder).await?;
    locks::lock_person(&mut tx, &founder).await?;
    let person = founder_under_lock(&state, &mut tx, &founder).await?;
    if person.default_organization_id.is_none()
        && let Some(current) =
            organization_members::default_while_owning_none(&mut tx, &founder).await?
    {
        identities::set_default_organization(&mut tx, &founder, &current).await?;
    }
    let founded =
        found_organization(tx, organization, &founder, &name, url, Founding::OnRequest).await?;
    crate::handler::ownership::tell_ownership_hook(
        &state,
        OwnershipChange::Founded {
            organization_id: founded.id.clone(),
            owner: founder.clone(),
        },
    )
    .await;

    tracing::info!(organization_id = %founded.id, user_id = %founder, "organization created");
    Ok((
        StatusCode::CREATED,
        Json(OrganizationCreated {
            id: founded.id,
            slug: founded.slug,
            name,
        }),
    ))
}

/// An organization founded: its id, and the slug and name it goes by.
#[derive(Debug, Serialize)]
pub struct OrganizationCreated {
    pub id: OrganizationId,
    pub slug: Slug,
    pub name: String,
}

/// An organization renamed: the name and slug it goes by now.
#[derive(Debug, Serialize)]
pub struct OrganizationNamed {
    pub name: String,
    pub slug: Slug,
}

/// What a deployment that founds an organization itself asks first
/// ([`telmoni_shared::seam::Auth::check_founding`]): the name and the URL,
/// refused as the founder's own asking would refuse them. Whether new
/// organizations are open is the seam's to read, in the lane, before this.
pub(crate) fn check_founding(name: &str, asked: Option<&Slug>) -> Result<(), TelmoniError> {
    organization_name(name)?;
    if let Some(asked) = asked {
        slug::admit_url(asked)?;
    }
    Ok(())
}

/// Found an organization for a deployment that has what it asks for first
/// ([`telmoni_shared::seam::Auth::found_organization`]): as
/// [`create_organization`] founds one at the person's asking, under the same
/// checks, but for the deployment's own say, which it has had; for new
/// organizations closed, which [`check_founding`] refused before anything
/// was taken of the founder and which does not refuse now, since the
/// founding is owed; and for the URL, where one asked for that is no longer
/// free is derived from the name rather than refused, for the same reason.
/// Idempotent by the id the deployment minted: one already founded is
/// answered as it stands.
pub(crate) async fn found_for_deployment(
    state: &AppState,
    founding: &OrganizationFounding,
) -> Result<FoundedOrganization, TelmoniError> {
    let organization = &founding.organization_id;
    let mut lane = maintenance_scope(&state.db, AuthLane).await?;
    let standing = organizations::slug_of(&mut lane, organization).await?;
    lane.commit().await?;
    if let Some(slug) = standing {
        return Ok(FoundedOrganization {
            organization_id: organization.clone(),
            slug,
        });
    }
    let name = organization_name(&founding.name)?;
    let asked = founding.slug.clone();
    if let Some(asked) = &asked {
        slug::admit_url(asked)?;
    }
    let mut candidates: Vec<Slug> = asked.into_iter().collect();
    candidates.extend(slug::candidates(slug::Scope::Organization, &name));
    let url = NewUrl::Derived(free_url(state, organization, &candidates).await?);

    let founder = &founding.founder;
    let mut tx = person_scope(&state.db, founder).await?;
    locks::lock_person(&mut tx, founder).await?;
    let person = founder_under_lock(state, &mut tx, founder).await?;
    if person.default_organization_id.is_none()
        && let Some(current) =
            organization_members::default_while_owning_none(&mut tx, founder).await?
    {
        identities::set_default_organization(&mut tx, founder, &current).await?;
    }
    let founded = found_organization(
        tx,
        organization.clone(),
        founder,
        &name,
        url,
        Founding::OnRequest,
    )
    .await?;
    crate::handler::ownership::tell_ownership_hook(
        state,
        OwnershipChange::Founded {
            organization_id: founded.id.clone(),
            owner: founder.clone(),
        },
    )
    .await;
    tracing::info!(organization_id = %founded.id, user_id = %founder, "organization founded for the deployment");
    Ok(FoundedOrganization {
        organization_id: founded.id,
        slug: founded.slug,
    })
}

/// Whether new organizations may be made: the global `signup` flag, read in
/// the lane before anything is written, since there is no organization yet to
/// override it on.
pub(crate) async fn signups_open(state: &AppState) -> Result<bool, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let global = flags::resolve_global(&mut tx).await?;
    tx.commit().await?;
    Ok(global.is_on(Flag::Signup))
}

/// The first of `candidates`, best first, that no other organization goes by,
/// read in the lane, since no binding of the new organization's sees another's
/// slug. `None` when none is free, or the name gave none.
///
/// ⚠ **Read before the person's lock is taken, never under it.** A request
/// holding the lock must not wait on a second connection: the requests queued
/// behind it each hold one, and enough of them empty the pool. A slug taken
/// between this read and the write is settled at the write, by a placeholder.
pub(crate) async fn free_url(
    state: &AppState,
    organization: &OrganizationId,
    candidates: &[Slug],
) -> Result<Option<Slug>, TelmoniError> {
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut lane = maintenance_scope(&state.db, AuthLane).await?;
    let free = organizations::first_free_slug(&mut lane, organization, candidates).await?;
    lane.commit().await?;
    Ok(free)
}

/// The founder, read again under the person's lock, which the caller holds.
/// Refused once their account's deletion is confirmed: it has read, or is
/// about to read, what they own. Refused while `VERIFY_EMAIL` waits on their
/// address, as `/me` refuses them, so nobody unproved comes to own anything.
/// And refused when their identity is gone, a race with its erasure that the
/// owner row's foreign key would fail anyway.
pub(crate) async fn founder_under_lock(
    state: &AppState,
    tx: &mut Scoped<'_, tenant_session::Person>,
    user_id: &UserId,
) -> Result<identities::Person, TelmoniError> {
    let Some(person) = identities::get(tx, user_id).await? else {
        return Err(AuthError::Unauthenticated.into());
    };
    if person.deletion_requested_at.is_some() {
        return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
    }
    if state.issuer.verify_email() && !person.email_verified {
        return Err(AuthzError::Forbidden("verify your email address to continue".into()).into());
    }
    Ok(person)
}

/// The URL a new organization takes, settled before the person's lock.
pub(crate) enum NewUrl {
    /// The one its founder asked for: written as asked, or refused as taken.
    Chosen(Slug),
    /// The one [`free_url`] found; with none, a placeholder.
    Derived(Option<Slug>),
}

/// How an organization came to be, as its first two audit events record it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Founding {
    /// By `/me`, for a person in no organization.
    Provisioned,
    /// By its founder's asking (`POST /internal/v1/organizations`).
    OnRequest,
}

impl Founding {
    const fn kind(self) -> AuditKind {
        match self {
            Self::Provisioned => AuditKind::AutoProvision,
            Self::OnRequest => AuditKind::OnRequest,
        }
    }
}

/// A new organization, as its founder is answered.
pub(crate) struct Founded {
    pub(crate) id: OrganizationId,
    pub(crate) slug: Slug,
}

/// Make `founder` the owner of `organization`, minted by the caller and
/// called `name`: the row, the owner row and both audit events, committed
/// together on the transaction that holds the person's lock, after the caller
/// has checked them under it. It takes no second connection: see
/// [`free_url`].
pub(crate) async fn found_organization(
    tx: Scoped<'_, tenant_session::Person>,
    organization: OrganizationId,
    founder: &UserId,
    name: &str,
    url: NewUrl,
    founding: Founding,
) -> Result<Founded, TelmoniError> {
    // Both GUCs from here: the person's for the check made under their lock,
    // the new organization's for the rows below, whose policies' WITH CHECK
    // name it.
    let mut tx = tx.bind_organization(&organization).await?;
    let slug = match url {
        NewUrl::Chosen(asked) => {
            if organizations::create(&mut tx, &organization, name, &asked)
                .await?
                .is_none()
            {
                return Err(url_taken());
            }
            asked
        }
        NewUrl::Derived(free) => {
            let slug = free.unwrap_or_else(|| slug::placeholder(slug::Scope::Organization));
            if organizations::create(&mut tx, &organization, name, &slug)
                .await?
                .is_some()
            {
                slug
            } else {
                // Another organization took the slug since the lane read it.
                // A placeholder is drawn at random, until the owner picks a URL.
                let placeholder = slug::placeholder(slug::Scope::Organization);
                if organizations::create(&mut tx, &organization, name, &placeholder)
                    .await?
                    .is_none()
                {
                    return Err(TelmoniError::Internal(
                        "a fresh placeholder slug was taken".into(),
                    ));
                }
                placeholder
            }
        }
    };
    organization_members::insert_owner(&mut tx, &organization, founder).await?;
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(founder.as_str()),
            AuditAction::Created,
            TelmoniResourceKind::Organization,
        )
        .on(organization.as_str())
        .details(json!({ "kind": founding.kind() })),
    )
    .await?;
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(founder.as_str()),
            AuditAction::Created,
            TelmoniResourceKind::Member,
        )
        .on(founder.as_str())
        .details(json!({
            "kind": founding.kind(),
            "role": OrganizationRole::Owner,
        })),
    )
    .await?;
    tx.commit().await?;
    Ok(Founded {
        id: organization,
        slug,
    })
}

/// Body of `PATCH /internal/v1/organization`: the name, the slug, or both.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateOrganizationRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
}

/// `PATCH /internal/v1/organization` — what the organization is called, and the
/// slug its paths begin with, as two settings. A name moves no slug, so a
/// rename leaves every link to the organization's pages standing; the slug
/// moves only when asked to, and the answer carries both so the console can
/// follow.
pub async fn update_organization(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<UpdateOrganizationRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;

    if req.name.is_none() && req.slug.is_none() {
        return Err(TelmoniError::BadRequest(
            "give a name, a slug, or both".into(),
        ));
    }
    let name = req.name.as_deref().map(organization_name).transpose()?;
    let asked_slug = req.slug.as_deref().map(slug::asked_url).transpose()?;

    let ActingOrganization {
        mut tx,
        role: caller_role,
    } = acting_organization(&state, &organization, &actor).await?;
    if let Err(refused) = authorize_organization(caller_role, OrganizationAction::ManageSettings) {
        tx.commit().await?;
        return Err(refused);
    }

    let Some(now) =
        organizations::update(&mut tx, &organization, name.as_deref(), asked_slug.as_ref())
            .await
            .map_err(slug_taken)?
    else {
        tx.commit().await?;
        return Err(AuthError::NotFound("organization not found".into()).into());
    };

    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(actor.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Organization,
        )
        .on(organization.as_str())
        .details(json!({ "name": now.name, "slug": now.slug })),
    )
    .await?;

    tx.commit().await?;
    tracing::info!(organization_id = %organization, "organization updated");
    Ok(Json(OrganizationNamed {
        name: now.name,
        slug: now.slug,
    }))
}

/// A slug another organization holds, chosen on Settings, as a 409 rather than
/// a 500.
fn slug_taken(e: sqlx::Error) -> TelmoniError {
    match e {
        sqlx::Error::Database(ref db) if db.constraint() == Some("organizations_slug_key") => {
            url_taken()
        }
        other => other.into(),
    }
}

/// The refusal of a URL another organization goes by, asked for on Settings
/// or at creation.
fn url_taken() -> TelmoniError {
    AuthError::Conflict("another organization already has that URL".into()).into()
}

/// A name as an organization carries it, or the 400 that says why not: one
/// rule for a new organization and a rename, so the two cannot drift.
fn organization_name(raw: &str) -> Result<String, TelmoniError> {
    crate::identity::name(raw, "organization", MAX_ORGANIZATION_NAME)
}
