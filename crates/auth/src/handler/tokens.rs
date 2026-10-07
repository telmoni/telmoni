use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration as ChronoDuration, Utc};
use uuid::Uuid;

use serde_json::json;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::rbac::{Resource, Verb, can};
use telmoni_shared::types::API_TOKEN_PREFIX;
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, Flag, Role, TelmoniError, TelmoniResourceKind, TenantError,
};

use crate::{
    AppState,
    db::{AuthLane, tokens},
    handler::{acting_project, project_of},
    model::{
        CreateTokenRequest, CreateTokenResponse, RotateTokenRequest, ValidateTokenRequest,
        ValidateTokenResponse,
    },
};

/// Default TTL for newly-minted tokens when the caller doesn't specify one.
const DEFAULT_TTL_DAYS: i64 = 90;

/// Default grace window for a rotation: 24 hours for a deploy to ship the new
/// token. An emergency rotation sends `grace_seconds: 0`.
const DEFAULT_ROTATE_GRACE_SECS: i64 = 24 * 60 * 60;

/// Upper bound for caller-supplied durations: 10 years, which keeps the
/// arithmetic far from where `chrono` saturates and Postgres refuses. "Never"
/// must be said as `null`.
const MAX_DURATION_SECS: u32 = 10 * 365 * 24 * 60 * 60;

/// `now + secs` as a concrete expiry, refusing anything past the cap.
fn bounded_expiry(secs: u32) -> Result<chrono::DateTime<Utc>, TelmoniError> {
    if secs > MAX_DURATION_SECS {
        return Err(AuthError::BadRequest(format!(
            "expires_in_seconds must be at most {MAX_DURATION_SECS} (10 years); send null for no expiry"
        ))
        .into());
    }
    ChronoDuration::try_seconds(i64::from(secs))
        .and_then(|d| Utc::now().checked_add_signed(d))
        .ok_or_else(|| {
            TelmoniError::internal(
                "expiry arithmetic failed inside the ten-year cap",
                std::io::Error::other("unreachable: bounded_expiry overflow"),
            )
        })
}

pub(crate) fn hash_token(raw_token: &str) -> String {
    telmoni_shared::digest::sha256_hex(raw_token.as_bytes())
}

pub(crate) fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    format!("{API_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// Gate a token verb on the actor's **authoritative** role. ⚠ Without this a
/// Member could mint a full-access token: the policy was defined and
/// matrix-tested but never enforced on this path. The role is resolved in the
/// same transaction as the write, so there is no check-then-act race.
fn ensure_token_authz(role: Role, verb: Verb) -> Result<(), TelmoniError> {
    if !can(role, verb, Resource::Token) {
        return Err(
            AuthzError::Forbidden(format!("role {role} may not {verb:?} API tokens")).into(),
        );
    }
    Ok(())
}

/// Refuse to mint or rotate while `api_tokens` is off for the project's
/// organization. The console refuses first, with its own sentence; this is
/// where the switch holds for a console built on it, or any other caller.
/// The flag is read under the owning organization's scope, where its own
/// override row is visible, and the project scope restored after, as
/// `acting_project` handed it over. Revoking is never gated: a leaked key
/// must be revocable whatever the switch says.
async fn refuse_unless_tokens_on(
    access: super::ActingProject<'_>,
) -> Result<super::ActingProject<'_>, TelmoniError> {
    let organization = access.organization.clone();
    let mut access = access.enter_owner_scope().await?;
    let flags = crate::db::flags::resolve_for_organization(&mut access.tx, &organization).await?;
    if !flags.is_on(Flag::ApiTokens) {
        return Err(TenantError::FeatureOff {
            flag: Flag::ApiTokens,
        }
        .into());
    }
    access.leave_owner_scope().await
}

/// `GET /internal/tokens` — the organization's `telmoni_` tokens, metadata only.
pub async fn list_tokens(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;
    let project_id = project_of(&headers)?;
    let mut access = acting_project(&state, &project_id, &actor).await?;
    ensure_token_authz(access.role, Verb::Read)?;
    let token_list = tokens::list(&mut access.tx, &project_id).await?;
    access.tx.commit().await?;
    Ok(Json(token_list))
}

pub async fn create_token(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<CreateTokenRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = project_of(&headers)?;
    if req.name.is_empty() || req.created_by.is_empty() {
        return Err(AuthError::BadRequest("name and created_by are required".into()).into());
    }
    if matches!(req.expires_in_seconds, Some(Some(0))) {
        return Err(AuthError::BadRequest(
            "expires_in_seconds must be positive; send null for no expiry, or omit it for the 90-day default".into(),
        )
        .into());
    }
    let expires_at = match req.expires_in_seconds {
        None => default_ttl_expiry(),
        Some(None) => None,
        Some(Some(secs)) => Some(bounded_expiry(secs)?),
    };
    let raw_token = generate_token();
    let token_hash = hash_token(&raw_token);
    let description = req.description.as_deref();

    let actor = principal.user_id;
    if req.created_by != actor.as_str() {
        return Err(AuthError::BadRequest("created_by must be the acting user".into()).into());
    }
    let access = acting_project(&state, &project_id, &actor).await?;
    ensure_token_authz(access.role, Verb::Create)?;
    let access = refuse_unless_tokens_on(access).await?;
    let token_organization = access.organization.clone();
    let mut tx = access.tx;
    let record = tokens::create(
        &mut tx,
        &token_organization,
        &project_id,
        &req.name,
        description,
        &crate::handler::parse_user_id(&req.created_by)?,
        expires_at,
        &token_hash,
    )
    .await
    .map_err(|e| -> TelmoniError {
        match e {
            sqlx::Error::Database(ref db_err) if db_err.constraint().is_some() => {
                AuthError::Conflict("token collision — retry".into()).into()
            }
            other => other.into(),
        }
    })?;
    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &token_organization,
            in_project: Some(&project_id),
            actor: Actor::User(&req.created_by),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Token,
            resource_id: Some(&record.id.to_string()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "name": req.name,
                "description": description,
                "expires_at": expires_at,
            })),
        },
    )
    .await?;
    tx.commit().await?;

    tracing::info!(project_id = %project_id, token_id = %record.id, created_by = %req.created_by, "API token created");

    Ok((
        StatusCode::CREATED,
        Json(CreateTokenResponse {
            id: record.id,
            name: record.name,
            token: raw_token,
        }),
    ))
}

/// The default token expiry, applied only when `expires_in_seconds` is
/// omitted; an explicit `null` means never.
fn default_ttl_expiry() -> Option<chrono::DateTime<Utc>> {
    Utc::now().checked_add_signed(ChronoDuration::seconds(DEFAULT_TTL_DAYS * 24 * 60 * 60))
}

pub async fn rotate_token(
    State(state): State<Arc<AppState>>,
    Path(token_id): Path<Uuid>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<RotateTokenRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;
    if let Some(n) = req.grace_seconds
        && !(0..=i64::from(MAX_DURATION_SECS)).contains(&n)
    {
        return Err(AuthError::BadRequest(format!(
            "grace_seconds must be between 0 and {MAX_DURATION_SECS} (10 years)"
        ))
        .into());
    }
    let grace = req.grace_seconds.unwrap_or(DEFAULT_ROTATE_GRACE_SECS);

    let project_id = project_of(&headers)?;
    let access = acting_project(&state, &project_id, &actor).await?;
    ensure_token_authz(access.role, Verb::Update)?;
    let access = refuse_unless_tokens_on(access).await?;
    let token_organization = access.organization.clone();
    let mut tx = access.tx;

    let ctx = tokens::begin_rotation(&mut tx, &project_id, token_id, grace).await?;
    let ctx = ctx.ok_or_else(|| {
        AuthError::NotFound(format!("token not found or already revoked: {token_id}"))
    })?;

    let raw_token = generate_token();
    let token_hash = hash_token(&raw_token);
    let expires_at = default_ttl_expiry();

    // The replacement is the rotator's: they alone ever see its secret, and
    // since ownership can move, the old key's minter may be somebody who has
    // since handed the organization on or left it.
    let new_record = tokens::create(
        &mut tx,
        &token_organization,
        &project_id,
        &ctx.name,
        ctx.description.as_deref(),
        &actor,
        expires_at,
        &token_hash,
    )
    .await
    .map_err(|e| -> TelmoniError {
        match e {
            sqlx::Error::Database(ref db_err) if db_err.constraint().is_some() => {
                AuthError::Conflict("token collision — retry".into()).into()
            }
            other => other.into(),
        }
    })?;

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &token_organization,
            in_project: Some(&project_id),
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Token,
            resource_id: Some(&new_record.id.to_string()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "rotated_from": token_id,
                "grace_seconds": grace,
            })),
        },
    )
    .await?;

    tx.commit().await?;

    tracing::info!(
        project_id = %project_id,
        old_token = %token_id,
        new_token = %new_record.id,
        grace_secs = %grace,
        "API token rotated",
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateTokenResponse {
            id: new_record.id,
            name: new_record.name,
            token: raw_token,
        }),
    ))
}

pub async fn validate_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ValidateTokenRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    if !req.token.starts_with(API_TOKEN_PREFIX) {
        return Err(AuthError::Unauthenticated.into());
    }

    let token_hash = hash_token(&req.token);

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let result = tokens::validate(&mut tx, &token_hash).await?;
    let flags = match &result {
        Some(v) => crate::db::flags::resolve_for_organization(&mut tx, &v.organization_id).await?,
        None => telmoni_shared::FlagSet::all_on(),
    };
    tx.commit().await?;

    let validated = result.ok_or(AuthError::Unauthenticated)?;

    Ok(Json(ValidateTokenResponse {
        name: validated.name,
        organization_id: validated.organization_id,
        flags,
        token_id: validated.id,
    }))
}

pub async fn revoke_token(
    State(state): State<Arc<AppState>>,
    Path(token_id): Path<Uuid>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let actor = principal.user_id;
    let project_id = project_of(&headers)?;
    let access = acting_project(&state, &project_id, &actor).await?;
    ensure_token_authz(access.role, Verb::Delete)?;
    let token_organization = access.organization.clone();
    let mut tx = access.tx;
    let revoked = tokens::revoke_in_project(&mut tx, &project_id, token_id).await?;
    if !revoked {
        return Err(AuthError::NotFound(format!("token not found: {token_id}")).into());
    }
    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &token_organization,
            in_project: Some(&project_id),
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::Token,
            resource_id: Some(&token_id.to_string()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: None,
        },
    )
    .await?;
    tx.commit().await?;

    tracing::info!(project_id = %project_id, token_id = %token_id, "API token revoked");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_token_has_tor_prefix() {
        let t = generate_token();
        assert!(
            t.starts_with("telmoni_"),
            "expected telmoni_ prefix, got: {t}"
        );
    }

    #[test]
    fn test_generate_token_sufficient_length() {
        let t = generate_token();
        assert!(t.len() >= 47, "token too short: {}", t.len());
    }

    #[test]
    fn test_generate_token_unique() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2, "two generated tokens must not be identical");
    }

    #[test]
    fn test_hash_token_deterministic() {
        let tok = "telmoni_test_input";
        assert_eq!(hash_token(tok), hash_token(tok));
    }

    #[test]
    fn test_hash_token_distinct_inputs() {
        assert_ne!(hash_token("telmoni_aaa"), hash_token("telmoni_bbb"));
    }

    #[test]
    fn test_hash_token_is_sha256_hex() {
        let h = hash_token("tor_anything");
        assert_eq!(
            h, "03cbb8777a5c0a4051ebe87e1b001f63eeca99f04111ec6c99c2178ffe02a2d0",
            "the at-rest token hash changed; every stored token just stopped validating"
        );
        assert_eq!(h.len(), 64, "SHA-256 hex digest must be 64 chars");
        assert!(
            h.chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
            "the digest must be LOWERCASE hex: {h}"
        );
    }

    #[test]
    fn bounded_expiry_accepts_the_cap() {
        let at = bounded_expiry(MAX_DURATION_SECS).expect("cap itself is valid");
        assert!(at > Utc::now());
    }

    #[test]
    fn bounded_expiry_refuses_past_the_cap() {
        assert!(bounded_expiry(MAX_DURATION_SECS + 1).is_err());
        assert!(bounded_expiry(u32::MAX).is_err());
    }

    /// The privilege escalation this gate closes, proven end to end: no member
    /// may mint, rotate or revoke a full-access token.
    #[sqlx::test(migrations = "./migrations")]
    async fn no_member_reaches_a_token_however_they_got_there(
        pool: sqlx::PgPool,
    ) -> sqlx::Result<()> {
        use telmoni_shared::Role;
        use telmoni_shared::db::tenant_session::{organization_scope, project_scope};

        let organization = telmoni_shared::OrganizationId::try_new("org_authz")
            .expect("valid test organization id");
        let project =
            telmoni_shared::ProjectId::try_new("project-authz").expect("valid test project id");
        let added_by = telmoni_shared::UserId::try_new("user_authz").expect("valid test user id");

        let mut tx = organization_scope(&pool, &organization).await?;
        crate::db::organizations::create(&mut tx, &organization, "Acme", "acme").await?;
        crate::db::projects::create(&mut tx, &project, &organization, "Default project").await?;
        tx.commit().await?;
        for (who, role) in [("member", Role::Member), ("admin", Role::Admin)] {
            let member_id = format!("user_{who}");
            telmoni_shared::test_util::seed_identity(
                &pool,
                &member_id,
                &format!("{who}@example.com"),
            )
            .await;
            let member = telmoni_shared::UserId::try_new(member_id).expect("valid test user id");
            let mut tx = project_scope(&pool, &project).await?;
            crate::db::members::insert(&mut tx, &project, &member, role, &added_by).await?;
            tx.commit().await?;
        }

        /// The membership row is the resolution — no implication on top.
        async fn resolve(
            pool: &sqlx::PgPool,
            project: &telmoni_shared::ProjectId,
            member: &str,
        ) -> Option<Role> {
            let mut tx = project_scope(pool, project).await.expect("scope");
            let member = telmoni_shared::UserId::try_new(member).expect("valid test user id");
            let role = crate::db::members::role_on(&mut tx, project, &member)
                .await
                .expect("role_on");
            tx.commit().await.expect("commit");
            role
        }

        for verb in [Verb::Create, Verb::Update, Verb::Delete] {
            let member_role = resolve(&pool, &project, "user_member")
                .await
                .expect("a member");
            assert!(
                ensure_token_authz(member_role, verb).is_err(),
                "user_member ({member_role}) must not {verb:?} tokens"
            );
            let admin_role = resolve(&pool, &project, "user_admin")
                .await
                .expect("an admin");
            assert!(
                ensure_token_authz(admin_role, verb).is_ok(),
                "user_admin ({admin_role}) can {verb:?} tokens"
            );
            assert!(ensure_token_authz(Role::Owner, verb).is_ok());
        }

        assert!(
            resolve(&pool, &project, "user_stranger").await.is_none(),
            "no membership row, no role"
        );
        Ok(())
    }
}
