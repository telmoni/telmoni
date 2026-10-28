//! `/internal/v1/connectors` — the OAuth handshake, the webhook connect, its
//! rotation and its choice of events, the list, the disconnect, the test send,
//! and the delivery log with its resend. Every lane acts for a
//! PERSON under `Resource::Connector`: a connection is a credential like a
//! token, so every role reads and only an owner or admin writes.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use ring::rand::SecureRandom as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::envelope::{KEK_VERSION, Kek, KekError, Sealed};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, NotificationKind, OrganizationId, ParseEnumError, ProjectId, Redacted,
    TelmoniError, TelmoniResourceKind, TenantError, UserId,
};
use uuid::Uuid;

use crate::AppState;
use crate::connector::{
    ConnectionStatus, DeliveryError, Event, Grant, Installed, Provider, Terminal, webhook,
};
use crate::db::{self, NotificationsLane};
use crate::handler::{ProjectIdentity, authorize};
use crate::{delivery, notify, uninstall};

/// How long a handshake may take between the authorize and the callback.
const STATE_TTL_SECS: i64 = 600;

/// How long a test send or a resend waits for the far end. The console waits
/// 20 seconds for these two lanes, room for this and the key unwrap and the
/// transactions around them, so the service answers before the console gives
/// up on a send it made.
const TEST_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a resend waits for the key service to unwrap the row's data key.
const KEY_UNWRAP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The vendor named in the path, or a 400.
fn provider_of(raw: &str) -> Result<Provider, TelmoniError> {
    raw.parse()
        .map_err(|e: ParseEnumError| TelmoniError::BadRequest(format!("the provider {e}")))
}

/// A 400 for a provider this deployment has not configured — the console
/// never offers a Connect it was told is off.
fn configured(state: &AppState, provider: Provider) -> Result<(), TelmoniError> {
    if state.connectors.enabled(provider) {
        return Ok(());
    }
    Err(TelmoniError::BadRequest(format!(
        "{} is not configured on this deployment",
        provider.label()
    )))
}

/// The OAuth half of a provider. The webhook has none, and is refused before
/// a state row is minted for a handshake that cannot finish.
fn grant_for(state: &AppState, provider: Provider) -> Result<Arc<dyn Grant>, TelmoniError> {
    configured(state, provider)?;
    state.connectors.grant(provider).ok_or_else(|| {
        TelmoniError::BadRequest(format!(
            "{} has no install dialog; connect it with an endpoint URL",
            provider.label()
        ))
    })
}

/// Where the vendor sends the browser back, built from `APP_URL` and nothing
/// the request carried.
fn redirect_uri(state: &AppState, provider: Provider) -> String {
    format!("{}/connect/{provider}/callback", state.config.app_url)
}

/// The KEK, or a 500; boot already refuses vendor apps with no key.
fn kek_for(state: &AppState) -> Result<&Kek, TelmoniError> {
    state
        .kek
        .as_ref()
        .ok_or_else(|| TelmoniError::Internal("CONNECTOR_KEK is not configured".into()))
}

/// An install whose data key could not be wrapped: a 502 with a sentence and
/// nothing inserted, rather than a grant stored under a key we could never open.
fn kek_refused(e: &KekError) -> TelmoniError {
    tracing::error!(error = %e, "the connector grant could not be sealed; nothing is stored");
    TelmoniError::Upstream(Box::new(
        telmoni_shared::error_schema::ProblemDetails::new(
            "/errors/connectors/key-unavailable",
            "the grant could not be sealed",
            502,
        )
        .detail(
            "The key service that seals connector grants did not answer, so nothing was \
             stored. Try connecting again in a moment.",
        ),
    ))
}

/// `TelmoniError::FeatureOff` when the `connectors` flag is off. An
/// unreadable set is the error it came with, never a pass.
async fn require_flag(state: &AppState) -> Result<(), TelmoniError> {
    let flags = state.auth.global_flags().await?;
    if flags.is_on(telmoni_shared::Flag::Connectors) {
        Ok(())
    } else {
        Err(TenantError::FeatureOff {
            flag: telmoni_shared::Flag::Connectors,
        }
        .into())
    }
}

/// Thirty-two random bytes as hex — the `state` the vendor echoes back.
fn mint_state() -> Result<String, TelmoniError> {
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| TelmoniError::Internal("no randomness for an oauth state".into()))?;
    Ok(telmoni_shared::digest::hex_lower(&bytes))
}

/// What a handshake's start answers: the vendor's consent page, and the
/// `state` its callback brings back. No `Debug`: the state is a secret until
/// the callback spends it.
#[derive(Serialize)]
pub struct Authorize {
    pub url: String,
    pub state: String,
}

/// `POST /internal/v1/connectors/{provider}/authorize` — start a handshake.
pub async fn authorize_connector(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Authorize>, TelmoniError> {
    let identity = authorize(&state, &headers, Verb::Create, Resource::Connector).await?;
    let provider = provider_of(&provider)?;
    let grant = grant_for(&state, provider)?;
    require_flag(&state).await?;

    let raw_state = mint_state()?;
    // Built before the state row, so a URL that will not build mints nothing.
    let url = grant.authorize_url(&raw_state, &redirect_uri(&state, provider))?;
    let mut tx = project_scope(&state.db, &identity.project_id).await?;
    db::insert_oauth_state(
        &mut tx,
        &telmoni_shared::digest::sha256_hex(raw_state.as_bytes()),
        &identity.project_id,
        &identity.organization_id,
        &identity.user_id,
        provider,
        STATE_TTL_SECS,
    )
    .await?;
    tx.commit().await?;

    Ok(Json(Authorize {
        url,
        state: raw_state,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackRequest {
    /// The vendor's one-time code. Masked in `Debug`: it is spendable once.
    pub code: String,
    pub state: String,
}

impl std::fmt::Debug for CallbackRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackRequest")
            .field("code", &"***")
            .field("state", &self.state)
            .finish()
    }
}

struct SealedCredentials {
    wrapped_dek: Vec<u8>,
    target: Sealed,
    token: Option<Sealed>,
}

async fn seal_connection_credentials(
    state: &AppState,
    kek: &Kek,
    id: Uuid,
    target_raw: &Redacted,
    token_raw: Option<&Redacted>,
) -> Result<SealedCredentials, TelmoniError> {
    let dek = state.vault.new_dek()?;
    let wrapped_dek = kek
        .wrap(&state.http, &dek, id.as_bytes())
        .await
        .map_err(|e| kek_refused(&e))?;
    let target = state.vault.seal(&dek, target_raw, id.as_bytes())?;
    let token = match token_raw {
        Some(token) => Some(state.vault.seal(&dek, token, id.as_bytes())?),
        None => None,
    };
    drop(dek);
    Ok(SealedCredentials {
        wrapped_dek,
        target,
        token,
    })
}

async fn consume_verified_oauth_state(
    state: &AppState,
    identity: &ProjectIdentity,
    provider: Provider,
    raw_state: &str,
) -> Result<(), TelmoniError> {
    let mut tx = project_scope(&state.db, &identity.project_id).await?;
    let Some(begun) = db::consume_oauth_state(
        &mut tx,
        &telmoni_shared::digest::sha256_hex(raw_state.as_bytes()),
    )
    .await?
    else {
        return Err(TelmoniError::BadRequest(
            "this connection attempt has expired or was already used; start again".into(),
        ));
    };
    tx.commit().await?;
    if begun.user_id != identity.user_id
        || begun.project_id != identity.project_id
        || begun.provider != provider
    {
        return Err(telmoni_shared::AuthzError::Forbidden(
            "this connection attempt was started by somebody else".into(),
        )
        .into());
    }
    Ok(())
}

/// A connection, as a finished handshake or a change to its kinds answers it.
#[derive(Debug, Serialize)]
pub struct ConnectionAnswer {
    pub connection: db::ConnectionSummary,
}

/// `POST /internal/v1/connectors/{provider}/callback` — finish a handshake.
pub async fn callback(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    Json(req): Json<CallbackRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let identity = authorize(&state, &headers, Verb::Create, Resource::Connector).await?;
    let provider = provider_of(&provider)?;
    let grant = grant_for(&state, provider)?;
    let kek = kek_for(&state)?;

    consume_verified_oauth_state(&state, &identity, provider, &req.state).await?;

    let installed = grant
        .exchange(&state.egress, &req.code, &redirect_uri(&state, provider))
        .await?;

    let mut tx = project_scope(&state.db, &identity.project_id).await?;
    let existing = db::existing_connection_id(
        &mut tx,
        &identity.project_id,
        provider,
        &installed.workspace_id,
        &installed.channel_id,
    )
    .await?;
    tx.commit().await?;
    let id = existing.unwrap_or_else(Uuid::now_v7);

    let creds =
        seal_connection_credentials(&state, kek, id, &installed.url, installed.token.as_ref())
            .await?;
    let row = db::NewConnection {
        id,
        project_id: &identity.project_id,
        organization_id: &identity.organization_id,
        provider,
        external_workspace_id: &installed.workspace_id,
        external_workspace_name: installed.workspace_name.as_deref(),
        channel_id: &installed.channel_id,
        channel_name: &installed.channel_name,
        target: &creds.target,
        key_version: KEK_VERSION,
        wrapped_dek: &creds.wrapped_dek,
        token: creds.token.as_ref(),
        scopes: &installed.scopes,
        event_kinds: None,
        installed_by: &identity.user_id,
    };
    // A refusal past the exchange leaves the app installed with its token held by
    // nobody: its uninstall is filed as a delete's is, for the settle to decide.
    let (summary, notices) = match connect_installed(&state, &identity, &row, existing).await {
        Ok(connected) => connected,
        Err(refused) => {
            file_refused_install(
                &state,
                RefusedInstall {
                    id,
                    project_id: identity.project_id.clone(),
                    organization_id: identity.organization_id.clone(),
                    user_id: identity.user_id.clone(),
                    provider,
                    installed,
                    creds,
                    reconnect: existing.is_some(),
                },
            );
            return Err(refused);
        }
    };
    tracing::info!(
        connection_id = %summary.id,
        project_id = %identity.project_id,
        provider = %provider,
        reconnect = existing.is_some(),
        "connector connected"
    );
    delivery::spawn_first_attempts(&state, notices);
    Ok((
        StatusCode::CREATED,
        Json(ConnectionAnswer {
            connection: summary,
        }),
    ))
}

/// What a refused install leaves to file: the exchange's answer, and the
/// credentials sealed for the row never written.
struct RefusedInstall {
    id: Uuid,
    project_id: ProjectId,
    organization_id: OrganizationId,
    user_id: UserId,
    provider: Provider,
    installed: Installed,
    creds: SealedCredentials,
    reconnect: bool,
}

/// Files the uninstall a refused install's token may need, on a task of its own, so
/// a caller that stops waiting loses nothing. A reconnect's token is sealed under its
/// row's id, which the row's own delete files under too, so it is sealed again under
/// an id of this filing's own; one the key service will not seal is dropped, logged.
fn file_refused_install(state: &Arc<AppState>, refused: RefusedInstall) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let RefusedInstall {
            id,
            project_id,
            organization_id,
            user_id,
            provider,
            installed,
            creds,
            reconnect,
        } = refused;
        let resealed;
        let (id, creds) = if reconnect {
            let fresh = Uuid::now_v7();
            let sealed = async {
                let kek = kek_for(&state)?;
                seal_connection_credentials(
                    &state,
                    kek,
                    fresh,
                    &installed.url,
                    installed.token.as_ref(),
                )
                .await
            }
            .await;
            match sealed {
                Ok(sealed) => {
                    resealed = sealed;
                    (fresh, &resealed)
                }
                Err(e) => {
                    tracing::error!(
                        connection_id = %id,
                        provider = %provider,
                        error = %e,
                        "a refused reconnect's token could not be sealed again; the app stays \
                         installed in its workspace"
                    );
                    return;
                }
            }
        } else {
            (id, &creds)
        };
        let row = db::NewConnection {
            id,
            project_id: &project_id,
            organization_id: &organization_id,
            provider,
            external_workspace_id: &installed.workspace_id,
            external_workspace_name: installed.workspace_name.as_deref(),
            channel_id: &installed.channel_id,
            channel_name: &installed.channel_name,
            target: &creds.target,
            key_version: KEK_VERSION,
            wrapped_dek: &creds.wrapped_dek,
            token: creds.token.as_ref(),
            scopes: &installed.scopes,
            event_kinds: None,
            installed_by: &user_id,
        };
        uninstall::file_refused(&state, &row).await;
    });
}

/// The install's one transaction: the workspace's lock, auth's answer again under
/// the chain lock, the row written or reconnected, its audit row and its notices.
async fn connect_installed(
    state: &AppState,
    identity: &ProjectIdentity,
    row: &db::NewConnection<'_>,
    existing: Option<Uuid>,
) -> Result<(db::ConnectionSummary, Vec<Uuid>), TelmoniError> {
    let provider = row.provider;
    let mut tx = project_scope(&state.db, &identity.project_id)
        .await?
        .bind_organization(&identity.organization_id)
        .await?;
    // Held to the commit: an uninstall's count of the workspace waits for this row.
    if provider.installed_per_workspace() {
        db::lock_workspace(&mut tx, provider, row.external_workspace_id).await?;
    }
    super::hold_chain(state, &mut tx, identity, Verb::Create, Resource::Connector).await?;
    let now_existing = db::existing_connection_id(
        &mut tx,
        &identity.project_id,
        provider,
        row.external_workspace_id,
        row.channel_id,
    )
    .await?;
    if now_existing != existing {
        return Err(AuthError::Conflict(
            "this channel was connected by another request just now; refresh and try again".into(),
        )
        .into());
    }
    let summary = if existing.is_some() {
        db::reconnect_connection(&mut tx, row).await?
    } else {
        db::insert_connection(&mut tx, row).await?
    };
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &identity.organization_id,
            Actor::User(identity.user_id.as_str()),
            AuditAction::Created,
            TelmoniResourceKind::Connector,
        )
        .in_project(&identity.project_id)
        .on(&summary.id.to_string())
        .details(json!({
            "provider": provider.as_str(),
            "workspace": row.external_workspace_id,
            "channel": row.channel_name,
            "reconnect": existing.is_some(),
        })),
    )
    .await?;
    let notices = notify::connector_connected(
        &mut tx,
        &identity.project_id,
        &identity.organization_id,
        provider,
        row.external_workspace_name,
        row.channel_name,
    )
    .await?;
    tx.commit().await?;
    Ok((summary, notices))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWebhookRequest {
    /// The endpoint, as typed. Masked in `Debug`, because some receivers
    /// authenticate by a secret in the path.
    pub url: String,
    /// The notice kinds it receives; absent or `null` is every kind.
    #[serde(default)]
    pub event_kinds: Option<Vec<NotificationKind>>,
}

impl std::fmt::Debug for CreateWebhookRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateWebhookRequest")
            .field("url", &"***")
            .field("event_kinds", &self.event_kinds)
            .finish()
    }
}

/// A chosen set of kinds as the column stores it: once each, in the enum's
/// wire order, so two requests naming the same kinds store the same array.
/// `None` stays every kind. An empty choice is refused: a webhook that
/// receives nothing is a disconnect, and the page offers one of those.
fn event_kinds_column(
    chosen: Option<Vec<NotificationKind>>,
) -> Result<Option<Vec<NotificationKind>>, TelmoniError> {
    let Some(chosen) = chosen else {
        return Ok(None);
    };
    if chosen.is_empty() {
        return Err(TelmoniError::BadRequest(
            "choose at least one event, or every event; to stop receiving notices, disconnect \
             the endpoint"
                .into(),
        ));
    }
    Ok(Some(
        NotificationKind::all()
            .into_iter()
            .filter(|kind| chosen.contains(kind))
            .collect(),
    ))
}

/// A webhook's connection and the secret it signs with, which no read
/// returns again.
#[derive(Debug, Serialize)]
pub struct SigningSecret {
    pub connection: db::ConnectionSummary,
    #[serde(serialize_with = "telmoni_shared::types::redacted::exposed")]
    pub signing_secret: Redacted,
}

/// `POST /internal/v1/connectors/webhook` — connect a customer's own endpoint.
pub async fn create_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateWebhookRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let identity = authorize(&state, &headers, Verb::Create, Resource::Connector).await?;
    configured(&state, Provider::Webhook)?;
    let kek = kek_for(&state)?;
    require_flag(&state).await?;

    let url = webhook::validate_endpoint_url(&req.url).map_err(TelmoniError::BadRequest)?;
    let event_kinds = event_kinds_column(req.event_kinds)?;
    let host = webhook::host_label(&url);
    let fingerprint = webhook::fingerprint(&url);

    let mut tx = project_scope(&state.db, &identity.project_id).await?;
    let existing = db::existing_connection_id(
        &mut tx,
        &identity.project_id,
        Provider::Webhook,
        &host,
        &fingerprint,
    )
    .await?;
    tx.commit().await?;
    if existing.is_some() {
        return Err(AuthError::Conflict(
            "this endpoint is already connected to this project; rotate its secret or disconnect it \
             first"
                .into(),
        )
        .into());
    }

    let id = Uuid::now_v7();
    let secret = webhook::mint_signing_secret()?;
    let target_redacted = Redacted::from(url.as_str());
    let creds =
        seal_connection_credentials(&state, kek, id, &target_redacted, Some(&secret)).await?;

    let mut tx = project_scope(&state.db, &identity.project_id)
        .await?
        .bind_organization(&identity.organization_id)
        .await?;
    super::hold_chain(
        &state,
        &mut tx,
        &identity,
        Verb::Create,
        Resource::Connector,
    )
    .await?;
    if db::existing_connection_id(
        &mut tx,
        &identity.project_id,
        Provider::Webhook,
        &host,
        &fingerprint,
    )
    .await?
    .is_some()
    {
        return Err(AuthError::Conflict(
            "this endpoint was connected by another request just now; refresh and try again".into(),
        )
        .into());
    }
    let row = db::NewConnection {
        id,
        project_id: &identity.project_id,
        organization_id: &identity.organization_id,
        provider: Provider::Webhook,
        external_workspace_id: &host,
        external_workspace_name: None,
        channel_id: &fingerprint,
        channel_name: &host,
        target: &creds.target,
        key_version: KEK_VERSION,
        wrapped_dek: &creds.wrapped_dek,
        token: creds.token.as_ref(),
        scopes: webhook::SCHEME,
        event_kinds: event_kinds.as_deref(),
        installed_by: &identity.user_id,
    };
    let summary = db::insert_connection(&mut tx, &row).await?;
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &identity.organization_id,
            Actor::User(identity.user_id.as_str()),
            AuditAction::Created,
            TelmoniResourceKind::Connector,
        )
        .in_project(&identity.project_id)
        .on(&summary.id.to_string())
        .details(json!({
            "provider": Provider::Webhook.as_str(),
            "host": host,
            "fingerprint": fingerprint,
            "event_kinds": event_kinds,
        })),
    )
    .await?;
    let notices = notify::connector_connected(
        &mut tx,
        &identity.project_id,
        &identity.organization_id,
        Provider::Webhook,
        None,
        &host,
    )
    .await?;
    tx.commit().await?;
    tracing::info!(
        connection_id = %summary.id,
        project_id = %identity.project_id,
        provider = %Provider::Webhook,
        "connector connected"
    );
    delivery::spawn_first_attempts(&state, notices);
    Ok((
        StatusCode::CREATED,
        Json(SigningSecret {
            connection: summary,
            signing_secret: secret,
        }),
    ))
}

/// The longest a replaced secret keeps signing: a day, Stripe's ceiling for a
/// roll, which is room for a deploy and short enough that a leaked secret
/// rotated with an overlap does not stay useful for long.
const MAX_OVERLAP_HOURS: u8 = 24;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotateRequest {
    /// How long the replaced secret keeps signing beside the new one; 0 ends
    /// it now, the answer to a leaked secret.
    pub keep_previous_for_hours: u8,
}

/// `POST /internal/v1/connectors/{id}/rotate` — a new signing secret for a
/// webhook, answered once. With an overlap, every delivery until it ends is
/// signed with both secrets, so a team deploying the receiver's new secret
/// drops nothing on the way; with none it is a hard cut.
pub async fn rotate_webhook_secret(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<RotateRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let identity = authorize(&state, &headers, Verb::Update, Resource::Connector).await?;
    let kek = kek_for(&state)?;
    if req.keep_previous_for_hours > MAX_OVERLAP_HOURS {
        return Err(TelmoniError::BadRequest(format!(
            "the previous secret can keep working for at most {MAX_OVERLAP_HOURS} hours"
        )));
    }

    let mut tx = project_scope(&state.db, &identity.project_id).await?;
    let sealed = db::sealed_connection(&mut tx, &identity.project_id, id)
        .await?
        .ok_or_else(|| AuthError::NotFound(format!("connection not found: {id}")))?;
    tx.commit().await?;
    if sealed.provider != Provider::Webhook {
        return Err(TelmoniError::BadRequest(
            "only a webhook has a signing secret to rotate; reconnect a channel instead".into(),
        ));
    }
    let Some(replacing) = sealed.token() else {
        return Err(TelmoniError::Internal(
            "the webhook row holds no signing secret".into(),
        ));
    };

    let row = kek
        .open_row(
            &state.http,
            &state.vault,
            id,
            &sealed.wrapped_dek,
            sealed.key_version,
        )
        .await
        .map_err(|e| match &e {
            KekError::Unavailable(_) => kek_refused(&e),
            KekError::Invalid(why) => {
                TelmoniError::Internal(format!("the row's wrapped key would not open: {why}"))
            }
        })?;
    let secret = webhook::mint_signing_secret()?;
    let token = row.seal(&secret)?;
    drop(row);

    let mut tx = project_scope(&state.db, &identity.project_id)
        .await?
        .bind_organization(&identity.organization_id)
        .await?;
    super::lock_audit_chain(&mut tx, &identity.organization_id).await?;
    let Some(summary) = db::reseal_webhook_secret(
        &mut tx,
        &identity.project_id,
        id,
        replacing,
        &token,
        i32::from(req.keep_previous_for_hours),
    )
    .await?
    else {
        return Err(AuthError::Conflict(
            "this webhook's secret was rotated by another request just now; refresh and try again"
                .into(),
        )
        .into());
    };
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &identity.organization_id,
            Actor::User(identity.user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Connector,
        )
        .in_project(&identity.project_id)
        .on(&id.to_string())
        .details(json!({
            "provider": Provider::Webhook.as_str(),
            "host": sealed.external_workspace_id,
            "rotated": "signing_secret",
            "previous_kept_for_hours": req.keep_previous_for_hours,
        })),
    )
    .await?;
    tx.commit().await?;
    tracing::info!(
        connection_id = %id,
        project_id = %identity.project_id,
        previous_kept_for_hours = req.keep_previous_for_hours,
        "webhook signing secret rotated"
    );
    Ok(Json(SigningSecret {
        connection: summary,
        signing_secret: secret,
    }))
}

/// What the list answers: every provider, and whether this deployment can
/// connect it; and the project's connections.
#[derive(Debug, Serialize)]
pub struct ConnectorListing {
    pub enabled: HashMap<Provider, bool>,
    pub connections: Vec<db::ConnectionSummary>,
}

/// `GET /internal/v1/connectors` — the project's connections, and which providers
/// this deployment can connect at all.
pub async fn list_connectors(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ConnectorListing>, TelmoniError> {
    let identity = authorize(&state, &headers, Verb::Read, Resource::Connector).await?;
    let mut tx = project_scope(&state.db, &identity.project_id).await?;
    let connections = db::list_connections(&mut tx, &identity.project_id).await?;
    tx.commit().await?;
    Ok(Json(ConnectorListing {
        enabled: Provider::all()
            .into_iter()
            .map(|provider| (provider, state.connectors.enabled(provider)))
            .collect(),
        connections,
    }))
}

/// `DELETE /internal/v1/connectors/{id}` — disconnect.
pub async fn delete_connector(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let identity = authorize(&state, &headers, Verb::Delete, Resource::Connector).await?;

    let mut tx = project_scope(&state.db, &identity.project_id).await?;
    super::lock_audit_chain(&mut tx, &identity.organization_id).await?;
    let sealed = db::delete_connection(&mut tx, &identity.project_id, id)
        .await?
        .ok_or_else(|| AuthError::NotFound(format!("connection not found: {id}")))?;
    let filed = uninstall::file(&mut tx, std::slice::from_ref(&sealed)).await?;
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &identity.organization_id,
            Actor::User(identity.user_id.as_str()),
            AuditAction::Deleted,
            TelmoniResourceKind::Connector,
        )
        .in_project(&identity.project_id)
        .on(&id.to_string())
        .details(json!({
            "provider": sealed.provider,
            "workspace": sealed.external_workspace_id,
            "channel": sealed.channel_name,
        })),
    )
    .await?;
    tx.commit().await?;
    tracing::info!(connection_id = %id, project_id = %identity.project_id, provider = %sealed.provider, "connector disconnected");

    uninstall::settle_soon(&state, filed);
    Ok(StatusCode::NO_CONTENT)
}

/// What a send made now answers, a test or a resend: the far end's refusal,
/// if any.
#[derive(Debug, Serialize)]
pub struct SendAnswer {
    pub delivered: bool,
    pub error: Option<String>,
}

impl SendAnswer {
    fn of(error: Option<String>) -> Self {
        Self {
            delivered: error.is_none(),
            error,
        }
    }
}

/// `POST /internal/v1/connectors/{id}/test` — send one message now and report
/// what came back: 200 whenever the send was attempted, since the vendor's
/// answer IS the payload. `Verb::Update`, because it posts into a channel.
pub async fn test_connector(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let identity = authorize(&state, &headers, Verb::Update, Resource::Connector).await?;

    let mut tx = project_scope(&state.db, &identity.project_id).await?;
    let sealed = db::sealed_connection(&mut tx, &identity.project_id, id)
        .await?
        .ok_or_else(|| AuthError::NotFound(format!("connection not found: {id}")))?;
    tx.commit().await?;
    if sealed.status != ConnectionStatus::Active {
        return Err(AuthError::Conflict(format!(
            "this connection is {}; reconnect it before testing",
            sealed.status
        ))
        .into());
    }

    let provider = sealed.provider;
    let body = format!(
        "This is a test from {}. Notices for this project will arrive here.",
        telmoni_shared::PRODUCT_NAME
    );
    let event = Event {
        id: Uuid::now_v7(),
        kind: NotificationKind::ConnectorConnected,
        title: &format!("{} is connected", provider.label()),
        body: &body,
    };
    let outcome = tokio::time::timeout(TEST_SEND_TIMEOUT, delivery::send(&state, &sealed, &event))
        .await
        .unwrap_or_else(|_| {
            Err(DeliveryError::transient(
                "the far end did not answer in time",
            ))
        });

    let error = match outcome {
        Ok(_) => {
            let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
            db::touch_connection(&mut tx, id).await?;
            tx.commit().await?;
            None
        }
        Err(DeliveryError::Terminal { class, reason, .. }) => {
            if let Terminal::Retire(to) = class {
                delivery::retire(&state, id, to, &reason).await?;
            }
            Some(reason)
        }
        Err(DeliveryError::Transient { message, .. } | DeliveryError::Held { message }) => {
            Some(message)
        }
    };
    tracing::info!(
        connection_id = %id,
        project_id = %identity.project_id,
        delivered = error.is_none(),
        "connector test send"
    );
    Ok(Json(SendAnswer::of(error)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventKindsRequest {
    /// The notice kinds to receive; `null` is every kind. Required: a replace
    /// states its value, so `{}` cannot widen a webhook to everything by
    /// omission.
    #[serde(deserialize_with = "present")]
    pub event_kinds: Option<Vec<NotificationKind>>,
}

/// An `Option` field that must be present, `null` or not: serde treats a
/// missing `Option` as `None`, and a field with its own deserializer as
/// required.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// `PUT /internal/v1/connectors/{id}/events` — change which notices a webhook
/// receives. Takes effect for the next notice; what is already queued goes.
pub async fn update_webhook_events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<EventKindsRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let identity = authorize(&state, &headers, Verb::Update, Resource::Connector).await?;
    let event_kinds = event_kinds_column(req.event_kinds)?;

    let mut tx = project_scope(&state.db, &identity.project_id)
        .await?
        .bind_organization(&identity.organization_id)
        .await?;
    super::lock_audit_chain(&mut tx, &identity.organization_id).await?;
    let Some(summary) =
        db::set_event_kinds(&mut tx, &identity.project_id, id, event_kinds.as_deref()).await?
    else {
        return Err(AuthError::NotFound(format!(
            "no webhook {id} on this project; only a webhook chooses its events"
        ))
        .into());
    };
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &identity.organization_id,
            Actor::User(identity.user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Connector,
        )
        .in_project(&identity.project_id)
        .on(&id.to_string())
        .details(json!({
            "provider": Provider::Webhook.as_str(),
            "host": summary.external_workspace_id,
            "event_kinds": event_kinds,
        })),
    )
    .await?;
    tx.commit().await?;
    tracing::info!(
        connection_id = %id,
        project_id = %identity.project_id,
        "webhook events changed"
    );
    Ok(Json(ConnectionAnswer {
        connection: summary,
    }))
}

/// The delivery log's page size when the console asks for none, and the most
/// it may ask for.
const LOG_PAGE_DEFAULT: i64 = 25;
const LOG_PAGE_MAX: i64 = 100;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryLogQuery {
    /// The oldest delivery the console already shows.
    pub before: Option<Uuid>,
    pub limit: Option<i64>,
}

/// A page of one connection's delivery log, newest first, and the cursor to
/// the older page after it while there is one.
#[derive(Debug, Serialize)]
pub struct DeliveryLogPage {
    pub deliveries: Vec<db::DeliveryLogEntry>,
    pub next_before: Option<Uuid>,
}

/// `GET /internal/v1/connectors/{id}/deliveries` — one connection's deliveries,
/// newest first, each with every send it took. Every role reads it: it is the
/// record of what the project told the outside world.
pub async fn list_deliveries(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    telmoni_shared::extract::Query(query): telmoni_shared::extract::Query<DeliveryLogQuery>,
) -> Result<Json<DeliveryLogPage>, TelmoniError> {
    let identity = authorize(&state, &headers, Verb::Read, Resource::Connector).await?;
    let limit = query
        .limit
        .unwrap_or(LOG_PAGE_DEFAULT)
        .clamp(1, LOG_PAGE_MAX);

    let mut tx = project_scope(&state.db, &identity.project_id).await?;
    if !db::connection_exists(&mut tx, &identity.project_id, id).await? {
        return Err(AuthError::NotFound(format!("connection not found: {id}")).into());
    }
    // One past the page, so the cursor is only offered when there is more.
    let mut deliveries =
        db::delivery_log(&mut tx, &identity.project_id, id, query.before, limit + 1).await?;
    tx.commit().await?;
    let more = i64::try_from(deliveries.len()).unwrap_or(i64::MAX) > limit;
    deliveries.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let next_before = more.then(|| deliveries.last().map(|d| d.id)).flatten();
    Ok(Json(DeliveryLogPage {
        deliveries,
        next_before,
    }))
}

/// How long a manual resend holds its row: the test send's budget and then
/// some, so a slow far end cannot let the loop take the row mid-send.
const REDELIVERY_LEASE_SECS: i64 = 60;

/// How many times one delivery may be resent by hand. A resend is for a
/// receiver that was down, not a way to replay a notice at will, and the cap
/// also bounds how many sends one log entry can carry.
const MAX_MANUAL_RESENDS: i64 = 10;

/// `POST /internal/v1/connectors/{id}/deliveries/{delivery_id}/redeliver` — send
/// one delivery again, now, and report what came back. The same delivery id,
/// so a receiver that already has it deduplicates; the attempt is logged as
/// manual and spends none of the row's retry budget. `Verb::Update`, because
/// it posts into the destination, and audited, because what it posts is the
/// project's own text sent out again by a person.
async fn claim_webhook_for_redelivery(
    state: &AppState,
    project_id: &ProjectId,
    id: Uuid,
    delivery_id: Uuid,
) -> Result<(db::SealedConnection, db::Claimed), TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    if db::manual_resends(&mut tx, project_id, delivery_id).await? >= MAX_MANUAL_RESENDS {
        tx.commit().await?;
        return Err(AuthError::Conflict(format!(
            "this delivery has been resent {MAX_MANUAL_RESENDS} times already"
        ))
        .into());
    }
    let sealed = db::sealed_connection(&mut tx, project_id, id)
        .await?
        .ok_or_else(|| AuthError::NotFound(format!("connection not found: {id}")))?;
    // A webhook's receiver deduplicates on the delivery id; a chat channel
    // would simply show the message twice.
    if sealed.provider != Provider::Webhook {
        tx.commit().await?;
        return Err(TelmoniError::BadRequest(
            "only a webhook delivery can be resent; a chat channel would show it twice".into(),
        ));
    }
    if sealed.status != ConnectionStatus::Active {
        tx.commit().await?;
        return Err(AuthError::Conflict(format!(
            "this connection is {}; reconnect it before resending",
            sealed.status
        ))
        .into());
    }
    let claimed =
        db::claim_for_redelivery(&mut tx, project_id, id, delivery_id, REDELIVERY_LEASE_SECS)
            .await?;
    tx.commit().await?;
    let claim = match claimed {
        Ok(claim) => claim,
        Err(db::Unclaimed::Missing) => {
            return Err(AuthError::NotFound(format!("delivery not found: {delivery_id}")).into());
        }
        Err(db::Unclaimed::Busy) => {
            return Err(AuthError::Conflict(
                "this delivery is being sent right now; look again in a moment".into(),
            )
            .into());
        }
    };
    Ok((sealed, claim))
}

async fn dispatch_redelivery(
    state: &AppState,
    project_id: &ProjectId,
    sealed: &db::SealedConnection,
    claim: &db::Claimed,
) -> Result<Result<delivery::Sent, String>, TelmoniError> {
    let delivery = &claim.delivery;
    let event = Event {
        id: delivery.id,
        kind: delivery.kind,
        title: &delivery.subject,
        body: &delivery.body,
    };
    // Each step has its own budget, both inside the lease: a stalled unwrap
    // that outlived it would let the loop take the row while this request
    // still meant to send it. An unwrap that fails or stalls sent nothing, so
    // the row goes back and nothing is logged as a send.
    let opened = match tokio::time::timeout(KEY_UNWRAP_TIMEOUT, delivery::open(state, sealed)).await
    {
        Ok(Ok(opened)) => opened,
        Ok(Err(e)) => {
            release(state, project_id, claim).await?;
            return Ok(Err(e.message().to_owned()));
        }
        Err(_) => {
            release(state, project_id, claim).await?;
            return Ok(Err(
                "the key service did not answer in time; nothing was sent".into(),
            ));
        }
    };
    let sent = tokio::time::timeout(
        TEST_SEND_TIMEOUT,
        delivery::dispatch_timed(state, &opened, &event),
    )
    .await
    .unwrap_or_else(|_| delivery::Sent {
        result: Err(DeliveryError::transient(
            "the far end did not answer in time",
        )),
        duration_ms: i32::try_from(TEST_SEND_TIMEOUT.as_millis()).unwrap_or(i32::MAX),
    });
    Ok(Ok(sent))
}

async fn record_redelivery_outcome(
    state: &Arc<AppState>,
    identity: &ProjectIdentity,
    id: Uuid,
    claim: &db::Claimed,
    sealed: &db::SealedConnection,
    sent: &delivery::Sent,
) -> Result<Option<String>, TelmoniError> {
    let delivery = &claim.delivery;
    let attempt = sent.attempt(delivery.id, db::AttemptTrigger::Manual);
    let error = sent.result.as_ref().err().map(|e| e.message().to_owned());
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    super::lock_audit_chain(&mut tx, &identity.organization_id).await?;
    // The connection before its delivery: the order a purge's cascade, a retirement
    // and the loop's bookkeeping take them, so none can deadlock with this.
    if error.is_none() {
        db::touch_connection(&mut tx, id).await?;
    }
    db::record_attempts(&mut tx, std::slice::from_ref(&attempt)).await?;
    if !db::finish_redelivery(&mut tx, &identity.project_id, claim, error.as_deref()).await? {
        tracing::warn!(
            delivery_id = %delivery.id,
            "a resend outlived its lease, or its delivery is gone; the row is left to its holder"
        );
    }
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &identity.organization_id,
            Actor::User(identity.user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Connector,
        )
        .in_project(&identity.project_id)
        .on(&id.to_string())
        .details(json!({
            "provider": sealed.provider,
            "resent": delivery.id,
            "kind": delivery.kind,
            "delivered": error.is_none(),
            "status_code": attempt.status_code,
        })),
    )
    .await?;
    tx.commit().await?;
    if let Err(DeliveryError::Terminal {
        class: Terminal::Retire(to),
        reason,
        ..
    }) = &sent.result
    {
        delivery::retire(state, id, *to, reason).await?;
    }
    tracing::info!(
        connection_id = %id,
        delivery_id = %delivery.id,
        project_id = %identity.project_id,
        delivered = error.is_none(),
        "delivery resent by hand"
    );
    Ok(error)
}

pub async fn redeliver(
    State(state): State<Arc<AppState>>,
    Path((id, delivery_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let identity = authorize(&state, &headers, Verb::Update, Resource::Connector).await?;

    let (sealed, claim) =
        claim_webhook_for_redelivery(&state, &identity.project_id, id, delivery_id).await?;

    let sent = match dispatch_redelivery(&state, &identity.project_id, &sealed, &claim).await? {
        Ok(sent) => sent,
        Err(open_error) => return Ok(Json(SendAnswer::of(Some(open_error)))),
    };

    let error = record_redelivery_outcome(&state, &identity, id, &claim, &sealed, &sent).await?;

    Ok(Json(SendAnswer::of(error)))
}

/// Hand a resend's claim back without having sent anything.
async fn release(
    state: &AppState,
    project_id: &telmoni_shared::ProjectId,
    claim: &db::Claimed,
) -> Result<(), TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    db::release_redelivery(&mut tx, project_id, claim).await?;
    tx.commit().await?;
    Ok(())
}
