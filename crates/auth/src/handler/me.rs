//! `/me` — who the bearer is, which organizations they belong to, and which one
//! they are acting in; and, for a person who belongs to none, the organization
//! every sign-up starts with — or, while sign-ups are closed, an answer with
//! no organization in it at all.
//!
//! ⚠ **A person with no organization is still a person who signed in.** Their
//! invitations and their account's deletion must reach them whatever gate is
//! up, so a closed sign-up is not a refusal here: it is an answer with
//! `activeOrganizationId: null`, on which the console shows exactly those.
//!
//! The SUBJECT is the resolved bearer. What the provider says about them is
//! `auth.identities`, recorded from the provider's own answer at each
//! sign-in's code exchange — a refresh asks the provider nothing; nothing in
//! the body may name or describe the person.

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::IntoResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{maintenance_scope, person_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, Flag, FlagSet, OrganizationId, OrganizationRole,
    TelmoniError, TelmoniResourceKind, UserId, slug,
};

use crate::{
    AppState,
    db::{
        AuthLane, flags, identities, invites, locks, members, organization_members, organizations,
        sessions,
    },
};

/// Body for `POST /me`. `deny_unknown_fields` is what refuses an `email` or a
/// `userId` here: nothing in the body may name or describe the person.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeRequest {
    /// The browser's `User-Agent`, for the sessions page's Device column.
    #[serde(default)]
    pub user_agent: Option<String>,
}

/// The person behind the bearer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonSummary {
    pub user_id: UserId,
    /// The address this person signs in with.
    pub email: String,
    pub display_name: Option<String>,
    /// Whether this person opted in to product analytics; served here so the
    /// privacy page costs no second fetch.
    pub analytics_opt_in: bool,
    /// Whether the address was ever proved — the verification link, an email
    /// change, a provider's word — as the column records it, not as the
    /// session reports it with `VERIFY_EMAIL` off. The console tells an
    /// unproved address nothing about invitations to it, live events included.
    pub email_verified: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub person: PersonSummary,
    /// Every active organization the person belongs to, oldest membership
    /// first, with their role in each and any ownership offer made to them.
    pub organizations: Vec<organization_members::OrganizationMembership>,
    /// The organizations the person owns that are being deleted, soonest to
    /// go first, so the privacy page can offer to restore the ones that still
    /// can be. Never active, never listed above, never chosen.
    pub deleted_organizations: Vec<organization_members::DeletedOrganization>,
    /// The organization this request acts in: the one the console asked for
    /// when the person belongs to it, else their default. The console sends it
    /// back as `x-organization-id`.
    /// `None` for a person in no organization — sign-ups closed, or their last
    /// one deleted a moment ago — whose invitations and account are all the
    /// console may show.
    pub active_organization_id: Option<OrganizationId>,
    /// The organization a sign-in opens in, and the one a request naming none
    /// acts in: the one the person chose, while they hold its seat and it is
    /// active, else the oldest they own, else the oldest they belong to — as
    /// Vercel opens on a default team, and picks another for whoever leaves
    /// theirs. `None` exactly when `active_organization_id` is.
    pub default_organization_id: Option<OrganizationId>,
    /// The project seats the person holds.
    pub memberships: Vec<members::Membership>,
    /// Pending invitations addressed to the person's email.
    pub incoming_invites: Vec<invites::IncomingInvite>,
    /// Projects whose owner has offered them to the person and whose offer
    /// is still open, each labelled by the organization it would leave.
    pub project_offers: Vec<members::ProjectOffer>,
    /// Feature flags for the active organization.
    pub flags: FlagSet,
    /// True only on the response that provisioned an organization for them.
    pub first_login: bool,
    /// The `auth.sessions` row for the session the bearer belongs to, which
    /// the console seals into the cookie and names on refresh.
    pub session_row_id: Uuid,
}

/// Body for `POST /test/session` — who the test door signs in, as an
/// exchange would have recorded them.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestSessionRequest {
    pub user_id: String,
    pub email: String,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

/// `POST /test/session` — record the person as a code exchange would, the
/// address taken as verified, and open a session for them: the same tokens
/// a sign-in answers with. Mounted only under `ALLOW_TEST_SESSION` (see the
/// router), which the binary refuses in a pod.
pub async fn test_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TestSessionRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = UserId::try_new(req.user_id.as_str())
        .map_err(|_| AuthError::BadRequest("userId is not a usable user id".into()))?;
    let email = crate::identity::validate_email(&req.email)?;
    let mut tx = person_scope(&state.db, &user_id).await?;
    identities::record(
        &mut tx,
        &user_id,
        &identities::Identity {
            email,
            email_verified: true,
            first_name: crate::identity::sanitize_display_name(req.first_name.as_deref()),
            last_name: crate::identity::sanitize_display_name(req.last_name.as_deref()),
        },
    )
    .await?;
    tx.commit().await?;
    let tokens = state.issuer.begin_session(&user_id, None, None).await?;
    tracing::info!(user_id = %user_id, "test session opened");
    Ok(Json(crate::handler::session::AuthnResult::from(tokens)))
}

pub async fn me(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<MeRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;

    let mut tx = person_scope(&state.db, &user_id).await?;
    let Some(person) = identities::get(&mut tx, &user_id).await? else {
        // A bearer's row references the identity and goes with it, so this is
        // a race with an erasure: nothing here says who the bearer belonged
        // to any more, and the body is not allowed to.
        tracing::warn!(user_id = %user_id, "/me for a subject with no recorded identity");
        return Err(AuthError::Unauthenticated.into());
    };
    if state.issuer.verify_email() && !person.email_verified {
        return Err(AuthzError::Forbidden("verify your email address to continue".into()).into());
    }
    if person.deletion_requested_at.is_some() {
        return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
    }
    let belongs = organization_members::belongs_anywhere(&mut tx, &user_id).await?;
    tx.commit().await?;

    // ⚠ **Every refusal BEFORE the session row** — provisioning's, and the
    // ones above. A refused `/me` seals no cookie, so a row written first
    // would list a session no browser holds, and a cookie sealed without its
    // id can never be signed out by it.
    let first_login = if belongs {
        false
    } else {
        matches!(
            provision_first_organization(&state, &user_id).await?,
            Provisioning::Provisioned
        )
    };

    // ⚠ **The maintenance lane, for a question about the caller.** Each
    // organization comes with its owner to contact — another person's row and
    // identity — and every invitation is another organization's. There is no
    // one GUC that opens exactly those, as `/internal/projects/everywhere`
    // found before this.
    let mut mtx = maintenance_scope(&state.db, AuthLane).await?;
    let organizations = organization_members::organizations_of(&mut mtx, &user_id).await?;
    let deleted_organizations =
        organization_members::pending_organizations_owned_by(&mut mtx, &user_id).await?;
    // An invitation is listed only to a verified address: the in-console
    // accept needs one, and an unproven address learns nothing about who
    // invited its holder.
    let incoming_invites = if person.email_verified {
        invites::list_incoming(&mut mtx, &person.email, &user_id).await?
    } else {
        Vec::new()
    };
    let project_offers = members::project_offers_to(&mut mtx, &user_id).await?;
    let requested = requested_organization(&headers);
    let default = default_organization(&organizations, person.default_organization_id.as_ref());
    let active = choose_active(&organizations, requested.as_ref(), default.as_ref());
    let flags = match &active {
        Some(active) => {
            let mut flags = flags::resolve_for_organization(&mut mtx, active).await?;
            // ⚠ **BetaAccess is the person's, not the active organization's.**
            // An invitee's own new organization lacks it while the one that
            // invited them has it, and walling them in theirs would hide the
            // switcher they need to leave it. On when ANY organization they
            // belong to has it.
            if !flags.is_on(Flag::BetaAccess) {
                let others: Vec<_> = organizations
                    .iter()
                    .filter(|o| &o.organization_id != active)
                    .map(|o| o.organization_id.clone())
                    .collect();
                if flags::on_for_any_organization(&mut mtx, Flag::BetaAccess, &others).await? {
                    flags.set(Flag::BetaAccess, true);
                }
            }
            flags
        }
        // No organization to resolve against: sign-ups are closed, or their
        // last one went away between the reads above. The global set, so the
        // console can say which.
        None => flags::resolve_global(&mut mtx).await?,
    };
    mtx.commit().await?;

    let mut tx = person_scope(&state.db, &user_id).await?;
    // The session row, keyed on the bearer's `sid`: this is the first bearer
    // call of every session, which is why the row is written here rather than
    // at the code exchange.
    let session_row_id = sessions::find_or_create_for_sid(
        &mut tx,
        &user_id,
        &principal.session_id,
        req.user_agent.as_deref(),
    )
    .await?;
    let mut memberships = members::memberships_of(&mut tx, &user_id).await?;
    tx.commit().await?;
    // A seat in an organization being deleted goes the way of the
    // organization itself, which the list above leaves out: a seat naming an
    // organization the console cannot see would be a dangling reference. The
    // erasure reads the same seats unfiltered, because it must remove them.
    memberships.retain(|seat| {
        organizations
            .iter()
            .any(|organization| organization.organization_id == seat.organization_id)
    });

    Ok(Json(MeResponse {
        person: PersonSummary {
            user_id,
            email: person.email,
            display_name: person.display_name,
            analytics_opt_in: person.analytics_opt_in,
            email_verified: person.email_verified,
        },
        organizations,
        deleted_organizations,
        active_organization_id: active,
        default_organization_id: default,
        memberships,
        incoming_invites,
        project_offers,
        flags,
        first_login,
        session_row_id,
    }))
}

/// The organization a caller asked to act in: by id, as the CLI names one, or
/// by slug, as the console reads one off the path it is rendering.
enum Requested {
    Id(OrganizationId),
    Slug(String),
}

impl Requested {
    fn names(&self, organization: &organization_members::OrganizationMembership) -> bool {
        match self {
            Self::Id(id) => &organization.organization_id == id,
            Self::Slug(slug) => &organization.slug == slug,
        }
    }
}

/// `x-organization-id`, else `x-organization-slug`. Unparseable is the same as
/// absent — a stale cookie must never cost somebody the console — and one the
/// person does not belong to is ignored by [`choose_active`].
fn requested_organization(headers: &HeaderMap) -> Option<Requested> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    header("x-organization-id")
        .and_then(|v| OrganizationId::try_new(v).ok())
        .map(Requested::Id)
        .or_else(|| {
            header("x-organization-slug")
                .filter(|v| slug::is_slug(v))
                .map(|v| Requested::Slug(v.to_owned()))
        })
}

/// The organization the person chose to open in, while it is among
/// `organizations` — the active ones they hold a seat in — else the oldest one
/// they own, else the oldest they belong to. `organizations` is in membership
/// order, oldest first.
fn default_organization(
    organizations: &[organization_members::OrganizationMembership],
    chosen: Option<&OrganizationId>,
) -> Option<OrganizationId> {
    chosen
        .and_then(|chosen| organizations.iter().find(|o| &o.organization_id == chosen))
        .or_else(|| {
            organizations
                .iter()
                .find(|o| o.role == OrganizationRole::Owner)
        })
        .or_else(|| organizations.first())
        .map(|o| o.organization_id.clone())
}

/// The requested organization when the person belongs to it, else their
/// default ([`default_organization`]).
fn choose_active(
    organizations: &[organization_members::OrganizationMembership],
    requested: Option<&Requested>,
    default: Option<&OrganizationId>,
) -> Option<OrganizationId> {
    requested
        .and_then(|r| organizations.iter().find(|o| r.names(o)))
        .map(|o| o.organization_id.clone())
        .or_else(|| default.cloned())
}

/// What [`provision_first_organization`] did for a person who belonged nowhere.
enum Provisioning {
    /// This call made them an organization.
    Provisioned,
    /// Somebody else's `/me` got there first.
    AlreadyBelongs,
    /// Sign-ups are closed: no organization, and none refused either — they
    /// sign in to their account alone.
    SignupsClosed,
}

/// Give a person who belongs to no active organization one of their own: the
/// organization and their owner row, audited, in one transaction. Nothing
/// else: the organization has no name until its owner gives it one, and no
/// project until somebody makes one, as a Vercel team starts empty.
///
/// ⚠ **Serialised on the person, and re-checked under the lock.** Two
/// concurrent first renders would otherwise each see "no organization" and
/// provision one each. The account deletion takes the same lock, so it and a
/// provisioning cannot interleave either.
async fn provision_first_organization(
    state: &AppState,
    user_id: &UserId,
) -> Result<Provisioning, TelmoniError> {
    // The sign-up flag gates CREATING an organization, read before anything
    // is written: there is no organization yet to override it on. Off is not
    // a refusal — the person still signs in, to an answer with no
    // organization — because their invitations and their account's deletion
    // must reach them whatever gate is up.
    let mut ftx = maintenance_scope(&state.db, AuthLane).await?;
    let global = flags::resolve_global(&mut ftx).await?;
    ftx.commit().await?;
    if !global.is_on(Flag::Signup) {
        return Ok(Provisioning::SignupsClosed);
    }

    let mut tx = person_scope(&state.db, user_id).await?;
    locks::lock_person(&mut tx, user_id).await?;
    if organization_members::belongs_anywhere(&mut tx, user_id).await? {
        tx.commit().await?;
        return Ok(Provisioning::AlreadyBelongs);
    }
    let pending = identities::get(&mut tx, user_id)
        .await?
        .is_some_and(|p| p.deletion_requested_at.is_some());
    if pending {
        return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
    }

    // ⚠ MINTED: an organization is nobody's id.
    let organization = OrganizationId::new();
    // Both GUCs from here: the person's for the check above, the new
    // organization's for the rows below, whose policies' WITH CHECK name it.
    let mut tx = tx.bind_organization(&organization).await?;
    organizations::create(&mut tx, &organization).await?;
    organization_members::insert_owner(&mut tx, &organization, user_id).await?;
    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(serde_json::json!({ "kind": "auto_provision" })),
        },
    )
    .await?;
    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(user_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(serde_json::json!({
                "kind": "auto_provision",
                "role": OrganizationRole::Owner.to_string(),
            })),
        },
    )
    .await?;
    tx.commit().await?;
    tracing::info!(organization_id = %organization, user_id = %user_id, "organization provisioned");
    Ok(Provisioning::Provisioned)
}
