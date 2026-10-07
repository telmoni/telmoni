//! The organization itself: its founding — the first one `/me` provisions, or
//! another a person asks for — its name, its deletion by its owner and the
//! owner's restore. An operator's termination and restore, and the steps the
//! deletion sweep drives it through, are [`crate::sweep`]. Its name and its
//! URL are two settings: a new organization takes the URL asked for, or one
//! derived from the name it is born with, and after that a rename moves
//! nothing.

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
use telmoni_shared::{
    AuthError, Flag, OrganizationId, OrganizationRole, OrganizationStatus, TelmoniError,
    TenantError, UserId,
};

use crate::{
    AppState,
    db::{
        AuthLane,
        confirmation_codes::{self, Act, Purpose},
        flags, identities, locks, organization_members, organizations,
    },
    handler::{
        ActingOrganization, account::DeletionRequest, account::wrong_code, acting_organization,
        deletion::RESTORE_WINDOW_SECONDS, organization_of, organization_role_or_forbidden,
    },
    model::DeletionKind,
};
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{
    self, Scoped, maintenance_scope, organization_scope, person_scope,
};
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

/// Body of `POST /internal/organizations`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOrganizationRequest {
    pub name: String,
    /// Absent or blank, the URL is derived from the name, as a first
    /// organization's is.
    #[serde(default)]
    pub slug: Option<String>,
}

/// `POST /internal/organizations` — the caller founds another organization,
/// as its owner. It names no existing organization, so it reads no
/// `x-organization-id`. Nothing caps how many a person makes or owns; the
/// global `signup` flag closes it, as it closes provisioning, and a closed one
/// is a refusal here, since the person asked.
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
        .map(asked_url)
        .transpose()?;
    if !signups_open(&state).await? {
        return Err(TenantError::FeatureOff { flag: Flag::Signup }.into());
    }
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

    tracing::info!(organization_id = %founded.id, user_id = %founder, "organization created");
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": founded.id, "slug": founded.slug, "name": name })),
    ))
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
    candidates: &[String],
) -> Result<Option<String>, TelmoniError> {
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
    Chosen(String),
    /// The one [`free_url`] found; with none, a placeholder.
    Derived(Option<String>),
}

/// How an organization came to be, as its first two audit events record it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Founding {
    /// By `/me`, for a person in no organization.
    Provisioned,
    /// By its founder's asking (`POST /internal/organizations`).
    OnRequest,
}

impl Founding {
    const fn kind(self) -> &'static str {
        match self {
            Self::Provisioned => "auto_provision",
            Self::OnRequest => "on_request",
        }
    }
}

/// A new organization, as its founder is answered.
pub(crate) struct Founded {
    pub(crate) id: OrganizationId,
    pub(crate) slug: String,
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
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(founder.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "kind": founding.kind() })),
        },
    )
    .await?;
    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(founder.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(founder.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": founding.kind(),
                "role": OrganizationRole::Owner.to_string(),
            })),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Founded {
        id: organization,
        slug,
    })
}

/// Body of `PATCH /internal/organization`: the name, the slug, or both.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateOrganizationRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
}

/// `PATCH /internal/organization` — what the organization is called, and the
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
        return Err(AuthError::BadRequest("give a name, a slug, or both".into()).into());
    }
    let name = req.name.as_deref().map(organization_name).transpose()?;
    let asked_slug = req.slug.as_deref().map(asked_url).transpose()?;

    let ActingOrganization {
        mut tx,
        role: caller_role,
    } = acting_organization(&state, &organization, &actor).await?;
    if !caller_role.can_manage_org_settings() {
        tx.commit().await?;
        return Err(AuthzError::Forbidden(
            "only an organization owner or admin can change the organization's name or URL".into(),
        )
        .into());
    }

    let Some(now) = organizations::update(
        &mut tx,
        &organization,
        name.as_deref(),
        asked_slug.as_deref(),
    )
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
            metadata: Some(json!({ "name": now.name, "slug": now.slug })),
        },
    )
    .await?;

    tx.commit().await?;
    tracing::info!(organization_id = %organization, "organization updated");
    Ok(Json(json!({ "name": now.name, "slug": now.slug })))
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
    let Some(name) = crate::identity::sanitize_organization_name(raw) else {
        return Err(AuthError::BadRequest("give the organization a name".into()).into());
    };
    if name.chars().count() > MAX_ORGANIZATION_NAME {
        return Err(AuthError::BadRequest(format!(
            "an organization name is at most {MAX_ORGANIZATION_NAME} characters"
        ))
        .into());
    }
    Ok(name)
}

/// A URL asked for, at creation or on Settings: a slug's shape, and no word
/// the console's own paths use. Whether another organization holds it is the
/// write's to say.
fn asked_url(raw: &str) -> Result<String, TelmoniError> {
    let asked = raw.trim();
    if !slug::is_slug(asked) {
        return Err(AuthError::BadRequest(format!(
            "a URL is lowercase letters and digits, in words joined by single hyphens, at \
             most {} characters",
            slug::MAX_LEN
        ))
        .into());
    }
    if slug::Scope::Organization.reserves(asked) {
        return Err(AuthError::BadRequest(format!(
            "{asked} is a word the console's own paths use — choose another URL"
        ))
        .into());
    }
    Ok(asked.to_owned())
}
