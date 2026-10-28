//! The person's own lanes — `/internal/v1/me/…`: the things somebody does to
//! THEIR account rather than to an organization. Password reset, the address
//! they sign in with, analytics consent, the organization a sign-in opens them
//! in, and deleting the account.
//!
//! The person is the bearer and nothing else, so there is nothing in the
//! path to forge.
//!
//! The address, consent and deletion audit their change on the chain of the
//! organization the console names in `x-organization-id` — after checking the
//! person is in it (`acting_person_in`). Somebody in no organization names
//! none, and the act goes to the structured log alone. The default
//! organization is recorded on the chain of the one chosen, which is never
//! none: another organization's chain has no business with it.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::Redacted;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::person_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, OrganizationId, OrganizationStatus, TelmoniError,
    TelmoniResourceKind, TenantError,
};

use crate::{
    AppState,
    db::{
        confirmation_codes::{self, Act, Purpose},
        identities, locks, organization_members, organizations, sessions, tokens,
    },
    handler::{
        acting_person_in,
        answer::Sent,
        deletion::{AccountDeleted, FINALIZE_GRACE_SECONDS, deletion_metadata, request_deletion},
        log_act_outside_every_organization, recording_organization_of,
    },
    model::{AuditKind, DeletionKind},
};

/// How many email-change codes one person may be issued in a day.
const EMAIL_CHANGE_DAILY_MAX: i64 = 5;

/// Body for `POST /internal/v1/me/email-change` — the address to move to.
/// Derived `Debug`: an address is not a credential.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestEmailChangeRequest {
    pub new_email: String,
}

/// Body for `POST /internal/v1/me/email-change/confirm` — one code from each
/// inbox. Both are required; each proves half of what this lane needs.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmEmailChangeRequest {
    /// OURS, mailed to the address on the account. Proves the asker holds the
    /// account, which a session cookie does not.
    pub current_code: Redacted,
    /// The identity provider's, mailed to the new address. Proves that address
    /// is reachable; never minted, stored or seen by this service.
    pub new_code: Redacted,
}

/// Body of `PUT /internal/v1/me/analytics`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnalyticsPreferenceRequest {
    pub opt_in: bool,
}

/// Body of `PUT /internal/v1/me/default-organization`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefaultOrganizationRequest {
    pub organization_id: OrganizationId,
}

/// Body for `DELETE /internal/v1/me` and `DELETE /internal/v1/organization` — the
/// emailed confirmation code that gates the irreversible cascade.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeletionRequest {
    pub code: Redacted,
}

/// The refusal every code lane gives a wrong or spent code.
pub(crate) fn wrong_code(burned: bool) -> TelmoniError {
    TelmoniError::BadRequest(if burned {
        "too many incorrect attempts — that code is no longer valid, ask for a new one".to_owned()
    } else {
        "invalid or expired confirmation code".to_owned()
    })
}

/// The caller's identity, or a 401 when auth never recorded one.
async fn person_of(
    state: &AppState,
    user_id: &telmoni_shared::UserId,
) -> Result<identities::Person, TelmoniError> {
    let mut tx = person_scope(&state.db, user_id).await?;
    let person = identities::get(&mut tx, user_id).await?;
    tx.commit().await?;
    person.ok_or_else(|| AuthError::Unauthenticated.into())
}

/// `POST /internal/v1/me/password-reset` — ask the identity provider for a
/// one-time link, which it mails itself. Not audited: nothing has changed yet.
pub async fn request_password_reset(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let person = person_of(&state, &principal.user_id).await?;

    // ⚠ A person on their way out gets no credential: without this, somebody
    // who had confirmed erasure could be mailed a live reset link for an
    // account the saga is deleting at the provider in the same seconds.
    if person.deletion_requested_at.is_some() {
        return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
    }

    let link = state
        .account_holder(&principal.user_id)
        .await?
        .create_password_reset(&person.email)
        .await?;

    tracing::info!(user_id = %principal.user_id, expires_at = %link.expires_at,
        "password-reset link minted");
    Ok((StatusCode::ACCEPTED, Json(Sent::MAILED)))
}

/// `POST /internal/v1/me/email-change` — open a pending address change: the
/// provider mails a code to the NEW address, and we mail ours to the CURRENT
/// one.
///
/// ⚠ **Two codes, because this lane REWRITES the inbox every other emailed gate
/// mails to.** Without the old-inbox factor, one stolen cookie moves the
/// address and then holds the deletion codes and the reset link a minute later.
///
/// ⚠ **The provider is asked FIRST.** Its refusals are the likely ones — an
/// address another of its users holds among them — and minting ours first
/// would leave a live, useless code in an inbox each time.
///
/// ⚠ **NOT AUDITED, and it must stay that way**: nothing has changed yet. The
/// audit-coverage gate holds both halves of that.
pub async fn request_email_change(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Json(req): Json<RequestEmailChangeRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;
    let new_email = crate::identity::validate_email(&req.new_email)?;
    let person = person_of(&state, &user_id).await?;

    if person.deletion_requested_at.is_some() {
        return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
    }
    if new_email == person.email {
        return Err(TelmoniError::BadRequest(
            "that is already the address on this account".into(),
        ));
    }

    let since = Utc::now() - Duration::days(1);
    let mut tx = person_scope(&state.db, &user_id).await?;
    let issued =
        confirmation_codes::count_since(&mut tx, &user_id, Purpose::EmailChange, since).await?;
    tx.commit().await?;
    if issued >= EMAIL_CHANGE_DAILY_MAX {
        // A full day, matching the rolling window: a shorter `Retry-After`
        // would be a promise the next press cannot keep.
        return Err(TenantError::RateLimited {
            retry_after_secs: 24 * 60 * 60,
        }
        .into());
    }

    let challenge = state
        .account_holder(&user_id)
        .await?
        .send_email_change(&user_id, &new_email)
        .await?;

    if !challenge.new_email.eq_ignore_ascii_case(&new_email) {
        tracing::warn!(user_id = %user_id,
            "the provider opened an email change against a different address than the one sent");
    }

    let code = confirmation_codes::generate_code();
    let expires_at = Utc::now() + Duration::minutes(confirmation_codes::CODE_TTL_MINUTES);
    let mut tx = person_scope(&state.db, &user_id).await?;
    confirmation_codes::create(
        &mut tx,
        &user_id,
        Act::EmailChange,
        &crate::issuer::hash(&code),
        expires_at,
    )
    .await?;
    tx.commit().await?;

    if let Err(e) = state
        .mailer
        .send_email_change_code(&person.email, &code, &new_email)
        .await
    {
        tracing::error!(user_id = %user_id, error = %e,
            "email-change code mail failed — check MAIL_FROM and the mail transport");
        return Err(TelmoniError::MailDelivery {
            context: "email-change confirmation code".into(),
        });
    }

    tracing::info!(
        user_id = %user_id,
        provider_expires_at = %challenge.expires_at,
        "email change requested; a code was issued to each address"
    );
    Ok((StatusCode::ACCEPTED, Json(Sent::MAILED)))
}

/// `POST /internal/v1/me/email-change/confirm` — spend both codes and move the
/// address.
///
/// Three phases, because the provider's response is what we write: verify our
/// code (without consuming it), let the provider spend its code, then write,
/// consume, revoke sessions and audit in one transaction.
pub async fn confirm_email_change(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<ConfirmEmailChangeRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;
    let organization = recording_organization_of(&headers)?;

    // ⚠ **VERIFIED HERE, CONSUMED IN PHASE 3** — unlike the deletion lanes.
    // Two codes are typed into one form, and consuming ours before the
    // provider accepts theirs would burn the old-inbox factor on a typo in the
    // other field. The provider's own single-use code makes the read-then-act
    // window worth nothing. The membership check comes first too: after the
    // provider commits, there is no refusing.
    let mut tx = acting_person_in(&state, &user_id, organization.as_ref()).await?;
    let pending = identities::get(&mut tx, &user_id)
        .await?
        .is_none_or(|p| p.deletion_requested_at.is_some());
    if pending {
        return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
    }
    let live = confirmation_codes::find_live(
        &mut tx,
        &user_id,
        Act::EmailChange,
        &crate::issuer::hash(req.current_code.expose().trim()),
    )
    .await?;
    if live.is_none() {
        // ⚠ The count is COMMITTED before the refusal: a counter that rolled
        // back with the failed request would guard a million-code space by
        // counting to one forever.
        let burned =
            confirmation_codes::register_failure(&mut tx, &user_id, Purpose::EmailChange).await?;
        tx.commit().await?;
        return Err(wrong_code(burned));
    }
    tx.commit().await?;

    let confirmed = state
        .account_holder(&user_id)
        .await?
        .confirm_email_change(&user_id, req.new_code.expose().trim())
        .await?;
    let new_email = identities::normalize_email(&confirmed.email);
    if !confirmed.email_verified {
        tracing::warn!(user_id = %user_id,
            "the provider confirmed an email change and reports the address unverified; \
             `/me` refuses this person until the provider verifies it");
    }

    // ⚠ **The provider has committed, so nothing below may refuse.** Phase 1
    // showed the person is in `organization` (or in none), which is what
    // licenses recording this on its chain (or in the log alone). Asking again
    // would turn a removal in the seconds since into a moved address with the
    // old inbox's codes still live and every session still standing.
    let mut tx = person_scope(&state.db, &user_id).await?;
    // The person's lock, as every lane that writes their rows takes it (see
    // `acting_person_in`).
    locks::lock_person(&mut tx, &user_id).await?;
    // The provider holds the new address now, and it proved it with its own
    // code. Every inbox-gated code in flight went to the OLD address, so they
    // all go.
    identities::set_email(&mut tx, &user_id, &new_email, confirmed.email_verified).await?;
    confirmation_codes::delete_all_for_person(&mut tx, &user_id).await?;
    let sids = sessions::revoke_all_for_person(&mut tx, &user_id).await?;
    let revoked = sids.len();

    let metadata = json!({
        "kind": AuditKind::EmailChange,
        "email": new_email,
        "sessions_revoked": revoked,
    });
    match &organization {
        Some(organization) => {
            emit_audit(
                &mut tx,
                AuditEvent::new(
                    organization,
                    Actor::User(user_id.as_str()),
                    AuditAction::Updated,
                    TelmoniResourceKind::Member,
                )
                .on(user_id.as_str())
                .details(metadata),
            )
            .await?;
        }
        None => log_act_outside_every_organization(&user_id, "email changed", &metadata),
    }
    tx.commit().await?;

    for sid in &sids {
        if let Err(e) = state.issuer.revoke_sid(sid).await {
            tracing::warn!(error = %e,
                "a session's tokens survive the email change; the row is revoked, which \
                 refuses its bearers and its next refresh");
        }
    }

    tracing::info!(user_id = %user_id, sessions_revoked = revoked, "account email changed");
    Ok((
        StatusCode::OK,
        Json(EmailChanged {
            email: new_email,
            sessions_revoked: revoked,
        }),
    ))
}

/// An address changed: the one the account goes by now, and how many of its
/// sessions were ended for it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailChanged {
    pub email: String,
    pub sessions_revoked: usize,
}

/// Whether the person lets the console count their use.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsPreference {
    pub analytics_opt_in: bool,
}

/// The organization the console opens on now.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultOrganization {
    pub default_organization_id: OrganizationId,
}

/// `PUT /internal/v1/me/analytics` — record whether this person wants to be
/// counted. ⚠ **OPT-IN**: the column starts `false` and nothing reaches a
/// provider until this lane says so. The preference is the person's and
/// follows them into every organization they are in.
pub async fn set_analytics_preference(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<AnalyticsPreferenceRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;
    let organization = recording_organization_of(&headers)?;

    let mut tx = acting_person_in(&state, &user_id, organization.as_ref()).await?;
    if !identities::set_analytics_opt_in(&mut tx, &user_id, req.opt_in).await? {
        tx.commit().await?;
        return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
    }

    let metadata = json!({ "analytics_opt_in": req.opt_in });
    match &organization {
        Some(organization) => {
            emit_audit(
                &mut tx,
                AuditEvent::new(
                    organization,
                    Actor::User(user_id.as_str()),
                    AuditAction::Updated,
                    TelmoniResourceKind::Member,
                )
                .on(user_id.as_str())
                .details(metadata),
            )
            .await?;
        }
        None => log_act_outside_every_organization(&user_id, "analytics consent set", &metadata),
    }

    tx.commit().await?;
    Ok(Json(AnalyticsPreference {
        analytics_opt_in: req.opt_in,
    }))
}

/// `PUT /internal/v1/me/default-organization` — the organization a sign-in opens
/// this person in, and the one a request naming none acts in: one of the
/// active organizations they hold a seat in. `/me` answers it as
/// `defaultOrganizationId`.
pub async fn set_default_organization(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Json(req): Json<DefaultOrganizationRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;
    let organization = req.organization_id;

    // Their seat there, and the organization being active, are proved under
    // the person's lock, which their own deletion takes too.
    let mut tx = acting_person_in(&state, &user_id, Some(&organization)).await?;
    let Some(person) = identities::get(&mut tx, &user_id).await? else {
        return Err(AuthError::Unauthenticated.into());
    };
    if person.deletion_requested_at.is_some() {
        return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
    }
    // Nothing written past those checks is a seat removed since: a removal
    // takes no lock of the person's.
    if !identities::set_default_organization(&mut tx, &user_id, &organization).await? {
        return Err(
            AuthzError::Forbidden("you are not a member of this organization".into()).into(),
        );
    }

    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Member,
        )
        .on(user_id.as_str())
        .details(json!({ "default_organization": true })),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(DefaultOrganization {
        default_organization_id: organization,
    }))
}

/// `POST /internal/v1/me/deletion-code` — mail the code that authorizes deleting
/// this account.
pub async fn request_account_deletion_code(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;
    let person = person_of(&state, &user_id).await?;
    if person.deletion_requested_at.is_some() {
        return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
    }

    let code = confirmation_codes::generate_code();
    let expires_at = Utc::now() + Duration::minutes(confirmation_codes::CODE_TTL_MINUTES);
    let mut tx = person_scope(&state.db, &user_id).await?;
    confirmation_codes::create(
        &mut tx,
        &user_id,
        Act::AccountDeletion,
        &crate::issuer::hash(&code),
        expires_at,
    )
    .await?;
    tx.commit().await?;

    if let Err(e) = state
        .mailer
        .send_account_deletion_code(&person.email, &code)
        .await
    {
        tracing::error!(user_id = %user_id, error = %e,
            "account-deletion code mail failed — check MAIL_FROM, the mail transport and the sending domain's DNS");
        return Err(TelmoniError::MailDelivery {
            context: "account-deletion confirmation code".into(),
        });
    }

    tracing::info!(user_id = %user_id, "account-deletion confirmation code issued");
    Ok((StatusCode::ACCEPTED, Json(Sent::MAILED)))
}

/// `DELETE /internal/v1/me` — erase the account, gated by its emailed code.
///
/// **Refused while the person owns an organization anybody else is in** (409,
/// naming them): that organization is those people's too, and deleting it
/// with its owner is not the owner's call alone. Hand it over or empty it
/// first. Organizations they own alone go with them.
///
/// One transaction marks everything — the code consumed, each owned
/// organization `pending_deletion` with its keys revoked and its own audit
/// row, the person pending, every session revoked — then the tails run inline.
/// Somebody who owns nothing is erased there and then. An owned
/// organization's purges run inline too, but its row waits out the finalize
/// grace window (`deletion::FINALIZE_GRACE_SECONDS`) and the person's erasure
/// waits for the row, so the deletion sweep finishes that account: a 202.
///
/// ⚠ **Locks: the person, then each owned organization in id order.** The
/// person's lock is what keeps `/me` provisioning, ownership accept and the
/// self-service lanes from changing what this reads the person owns, or
/// writing their rows, while it runs; the organizations' are what the
/// membership lanes take. Same order everywhere, so nothing deadlocks.
pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Json(req): Json<DeletionRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;

    let mut tx = person_scope(&state.db, &user_id).await?;
    locks::lock_person(&mut tx, &user_id).await?;

    let Some(person) = identities::get(&mut tx, &user_id).await? else {
        return Err(AuthError::Unauthenticated.into());
    };
    if person.deletion_requested_at.is_some() {
        // Only a double submit gets here: the person gate refuses anyone
        // pending, so this request passed it before the first one committed.
        // That one is running the tails, and the sweeps finish what it cannot.
        tx.commit().await?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(AccountDeleted { deleted: false }),
        ));
    }

    let Some(code_id) = confirmation_codes::find_live(
        &mut tx,
        &user_id,
        Act::AccountDeletion,
        &crate::issuer::hash(req.code.expose().trim()),
    )
    .await?
    else {
        let burned =
            confirmation_codes::register_failure(&mut tx, &user_id, Purpose::AccountDeletion)
                .await?;
        tx.commit().await?;
        return Err(wrong_code(burned));
    };

    let owned = organization_members::owned_by(&mut tx, &user_id).await?;
    // Label and count: two organizations may carry one name, and must not
    // read as one.
    let mut blockers: Vec<(String, usize)> = Vec::new();
    for organization in &owned {
        locks::lock_organization(&mut tx, organization).await?;
        let mut otx = tx.bind_organization(organization).await?;
        let row = organizations::get(&mut otx, organization).await?;
        // One already being deleted goes anyway, and the erasure waits for its
        // tail; telling the person to hand it over would name something the
        // transfer lane refuses.
        let shared = match &row {
            Some(row) if row.status == OrganizationStatus::Active => {
                organization_members::has_others(&mut otx, organization, &user_id).await?
            }
            _ => false,
        };
        tx = otx.clear_organization().await?;
        let Some(row) = row.filter(|_| shared) else {
            continue;
        };
        let label = row.name;
        match blockers.iter_mut().find(|(seen, _)| *seen == label) {
            Some((_, count)) => *count += 1,
            None => blockers.push((label, 1)),
        }
    }
    if !blockers.is_empty() {
        let named: Vec<String> = blockers
            .into_iter()
            .map(|(label, count)| {
                if count > 1 {
                    format!("{label} ({count} organizations)")
                } else {
                    label
                }
            })
            .collect();
        // Rolled back, not committed: the code stays live for when the person
        // has handed those organizations over.
        return Err(AuthError::Conflict(format!(
            "you own organizations other people are in: {}. Transfer ownership of each, or \
             remove everyone else from it, then delete your account.",
            named.join(", ")
        ))
        .into());
    }

    confirmation_codes::consume(&mut tx, code_id).await?;
    for organization in &owned {
        let mut otx = tx.bind_organization(organization).await?;
        // Revoked outright, unlike an organization its owner deletes, and
        // the count recorded on the chain: the person's erasure is what the
        // chain must account for, down to the keys their organizations held.
        let revoked = tokens::revoke_all_in_organization(&mut otx, organization).await?;
        // One already pending — deleted by its owner, or terminated — goes
        // with the account too: its kind becomes `account`, so its chain says
        // what took it, and its wait is never lengthened. Audited as `taken`
        // rather than `requested`: its chain already holds the request.
        let marked = match request_deletion(&mut otx, organization, DeletionKind::Account).await? {
            Some(erase_after) => Some((erase_after, "requested")),
            None => organizations::take_for_account(&mut otx, organization, FINALIZE_GRACE_SECONDS)
                .await?
                .map(|erase_after| (erase_after, "taken")),
        };
        if let Some((erase_after, step)) = marked {
            emit_audit(
                &mut otx,
                AuditEvent::new(
                    organization,
                    Actor::User(user_id.as_str()),
                    AuditAction::Updated,
                    TelmoniResourceKind::Organization,
                )
                .on(organization.as_str())
                .details(deletion_metadata(
                    step,
                    DeletionKind::Account,
                    erase_after,
                    Some(revoked),
                )),
            )
            .await?;
        }
        tx = otx.clear_organization().await?;
    }
    identities::mark_pending_deletion(&mut tx, &user_id).await?;
    let sids = sessions::revoke_all_for_person(&mut tx, &user_id).await?;
    tx.commit().await?;

    for sid in &sids {
        if let Err(e) = state.issuer.revoke_sid(sid).await {
            tracing::warn!(error = %e,
                "a session's tokens survive the account deletion; the row is revoked, which \
                 refuses them");
        }
    }

    tracing::info!(user_id = %user_id, organizations = owned.len(), "account deletion requested");
    Ok(crate::handler::deletion::account_deletion_response(&state, &user_id, &owned).await)
}
