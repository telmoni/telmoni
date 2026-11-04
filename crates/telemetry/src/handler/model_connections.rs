//! A project's model connections, the customer's own keys its evaluators
//! judge with: owners and admins connect, change and delete one, each change
//! filed in the chain; every member reads them, never a key. A key is sealed
//! under its own data key, wrapped by `MODEL_CONNECTION_KEK`, and opened only
//! by a save's test, an evaluator's save and try, and the evaluation loop.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::envelope::{KEK_VERSION, SealedRef};
use telmoni_shared::extract::Json;
use telmoni_shared::model::{self, Ask, Connection, Credentials, Role, systemone};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, ModelProvider, OrganizationId, ProjectId, Redacted, TelmoniError,
    TelmoniResourceKind,
};

use crate::AppState;
use crate::handler::chain;

/// The most connections a project holds, and the most models one names.
pub const CONNECTIONS_MAX: usize = 10;
pub const MODELS_MAX: usize = 20;
/// The longest name a connection or a model goes by.
pub const NAME_MAX: usize = 60;
pub const MODEL_MAX: usize = 128;
/// The longest secret taken: a service account's JSON key is a few kilobytes.
pub const SECRET_MAX: usize = 16_384;

/// A connection as the lanes answer it: what it reaches, never its key.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct ModelConnection {
    pub id: Uuid,
    pub name: String,
    pub provider: ModelProvider,
    pub base_url: Option<String>,
    pub region: Option<String>,
    pub models: Vec<String>,
    pub is_default: bool,
    /// The key's last four characters, as an API key's are shown; for one
    /// that signs in, the role or the account it signs in as.
    pub secret_hint: String,
    /// It stores no key, signing in as its project instead.
    pub signs_in: bool,
    /// When its provider last refused its key, and its words: its evaluators
    /// wait until a save proves it again.
    pub refused_at: Option<DateTime<Utc>>,
    pub refused_reason: Option<String>,
    /// When its key last went out with a request, a minute stale at most.
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The project's connections, its default first, and what a provider is set
/// to trust for one that stores no key: none where this deployment signs none.
#[derive(Debug, Serialize)]
pub struct ModelConnections {
    pub connections: Vec<ModelConnection>,
    pub federation: Option<FederationSetup>,
}

/// What a customer's AWS IAM or Google Cloud pool provider trusts: the
/// project's own issuer, the subject its tokens name, and the audience AWS's
/// exchange takes.
#[derive(Debug, Serialize)]
pub struct FederationSetup {
    pub issuer: String,
    pub subject: String,
    pub aws_audience: &'static str,
}

/// The sign-in a connection that stores no key goes out with; none for one
/// that holds a key, or where this deployment signs nothing.
fn sign_in_for(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    id: Uuid,
    credentials: &Credentials,
) -> Option<model::SignIn> {
    if !matches!(credentials, Credentials::Federated(_)) {
        return None;
    }
    Some(model::SignIn {
        federation: state.federation.clone()?,
        organization: organization_id.clone(),
        project: project_id.clone(),
        connection: id,
    })
}

/// What a connection's save sends: its id where it changes one, and its
/// secret where it is new or the key changes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionChange {
    #[serde(default)]
    pub id: Option<Uuid>,
    pub name: String,
    pub provider: ModelProvider,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    pub models: Vec<String>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub secret: Option<Redacted>,
}

/// A change held to a connection's rules: its name, its models, and the
/// endpoint and region its provider needs, each trimmed.
fn checked(mut change: ConnectionChange) -> Result<ConnectionChange, TelmoniError> {
    let refuse = |why: &str| Err(TelmoniError::BadRequest(why.to_owned()));
    change.name = change.name.trim().to_owned();
    if change.name.is_empty() || change.name.chars().count() > NAME_MAX {
        return refuse("a connection's name is 1 to 60 characters");
    }
    change.models = change
        .models
        .iter()
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty())
        .collect();
    change.models.dedup();
    if change.models.is_empty()
        || change.models.len() > MODELS_MAX
        || change.models.iter().any(|m| m.chars().count() > MODEL_MAX)
    {
        return refuse("a connection names 1 to 20 models, each up to 128 characters");
    }
    let needs_base = matches!(
        change.provider,
        ModelProvider::AzureOpenai
            | ModelProvider::OpenaiCompatible
            | ModelProvider::SystemOneCompatible
    );
    change.base_url = match (
        change
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty()),
        needs_base,
    ) {
        (Some(base), true) => Some(model::base_url(base).map_err(TelmoniError::BadRequest)?),
        (None, true) => return refuse("this provider's connection names its endpoint's base URL"),
        (Some(_), false) => {
            return refuse("this provider answers at its own host; a connection names no base URL");
        }
        (None, false) => None,
    };
    change.region = match (
        change
            .region
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty()),
        change.provider,
    ) {
        (Some(region), ModelProvider::Bedrock | ModelProvider::VertexAi) => Some(
            model::Region::try_new(region)
                .map_err(TelmoniError::BadRequest)?
                .as_str()
                .to_owned(),
        ),
        (None, ModelProvider::Bedrock) => return refuse("a Bedrock connection names its region"),
        (None, _) => None,
        (Some(_), _) => return refuse("only Bedrock and Vertex AI connections name a region"),
    };
    if change
        .secret
        .as_ref()
        .is_some_and(|s| s.expose().len() > SECRET_MAX)
    {
        return refuse("a connection's key is up to 16 KB");
    }
    Ok(change)
}

/// The last four characters a secret is shown by; a service account's by its email's.
fn hint(provider: ModelProvider, credentials: &Credentials, secret: &str) -> String {
    let tail = |s: &str| {
        s.chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>()
    };
    match credentials {
        Credentials::ServiceAccount(account) => account.client_email.clone(),
        Credentials::Federated(federated) => federated.shown(),
        Credentials::GoogleToken { .. } => String::new(),
        Credentials::AccessKeys { access_key_id, .. } => tail(access_key_id),
        Credentials::ApiKey(_)
            if secret.trim().is_empty()
                && matches!(
                    provider,
                    ModelProvider::OpenaiCompatible | ModelProvider::SystemOneCompatible
                ) =>
        {
            String::new()
        }
        Credentials::ApiKey(key) => tail(key.expose()),
    }
}

/// The schema a save's test asks for: one yes.
fn test_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "ok": { "type": "boolean" } },
        "required": ["ok"],
        "additionalProperties": false,
    })
}

/// One small judgement of `connection`'s first model, a System One one's a
/// single question: the save's proof that the key, the endpoint and the
/// model answer.
async fn try_connection(
    state: &AppState,
    connection: &Connection,
    model: &str,
) -> Result<(), TelmoniError> {
    let refused = |refused: model::Refused| {
        TelmoniError::BadRequest(format!(
            "the connection was not saved: {}",
            refused.sentence()
        ))
    };
    if model::speaks_system_one(connection.provider) {
        let questions = [systemone::Question {
            name: "ok".into(),
            instructions: "Is this a greeting?".into(),
            criteria: systemone::Criteria::Noul {
                yes_means: String::new(),
                no_means: String::new(),
            },
        }];
        let decide = systemone::Decide {
            model,
            state: "Hello.",
            questions: &questions,
        };
        return model::decide(&state.egress, connection, &decide)
            .await
            .map(|_| ())
            .map_err(refused);
    }
    let schema = test_schema();
    let messages = [(Role::User, "Answer {\"ok\": true}.".to_owned())];
    let ask = Ask {
        model,
        system: "You check that a connection answers. Answer with the JSON asked for.",
        messages: &messages,
        schema: &schema,
        temperature: None,
        max_tokens: 64,
        media: &[],
    };
    model::judge(&state.egress, connection, &ask)
        .await
        .map(|_| ())
        .map_err(refused)
}

/// What a connection's key is sealed bound to: its row, project, provider
/// and endpoint, so a row moved by any path but a save with the key opens nothing.
fn sealed_for(
    id: Uuid,
    project_id: &ProjectId,
    provider: ModelProvider,
    base_url: Option<&str>,
    region: Option<&str>,
) -> Vec<u8> {
    [
        id.as_bytes().as_slice(),
        project_id.as_str().as_bytes(),
        provider.as_str().as_bytes(),
        base_url.unwrap_or_default().as_bytes(),
        region.unwrap_or_default().as_bytes(),
    ]
    .join(&0)
}

/// The key is refused before anything else where no `MODEL_CONNECTION_KEK` seals it.
fn kek(state: &AppState) -> Result<&telmoni_shared::envelope::Kek, TelmoniError> {
    state.model_kek.as_ref().ok_or_else(|| {
        TelmoniError::BadRequest(
            "this deployment keeps no model's key: MODEL_CONNECTION_KEK is unset".into(),
        )
    })
}

/// A connection's row as the loop and a save open it.
#[derive(sqlx::FromRow)]
struct Sealed {
    provider: ModelProvider,
    base_url: Option<String>,
    region: Option<model::Region>,
    secret_ciphertext: Vec<u8>,
    secret_nonce: Vec<u8>,
    wrapped_dek: Vec<u8>,
    key_version: i16,
}

/// Connection `id` of `project_id`, its key opened.
///
/// # Errors
///
/// No such connection, the key service did not answer, or the key no longer
/// reads as its provider takes it.
pub async fn open(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    id: Uuid,
) -> Result<Connection, TelmoniError> {
    let kek = kek(state)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let row: Option<Sealed> = sqlx::query_as(
        "SELECT provider, base_url, region, secret_ciphertext, secret_nonce, wrapped_dek, key_version
           FROM telemetry.model_connections WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await?;
    tx.commit().await?;
    let row = row.ok_or_else(|| {
        AuthError::NotFound("no model connection with that id in this project".into())
    })?;
    let dek = kek
        .unwrap_dek(
            &state.http,
            &row.wrapped_dek,
            id.as_bytes(),
            row.key_version,
        )
        .await
        .map_err(|e| {
            TelmoniError::Internal(format!(
                "a model connection's key could not be unwrapped: {e}"
            ))
        })?;
    let bound = sealed_for(
        id,
        project_id,
        row.provider,
        row.base_url.as_deref(),
        row.region.as_ref().map(model::Region::as_str),
    );
    let sealed = SealedRef {
        ciphertext: &row.secret_ciphertext,
        nonce: &row.secret_nonce,
    };
    let secret = state.vault.open(&dek, sealed, &bound).map_err(|_| {
        tracing::error!(project_id = %project_id, connection_id = %id,
            "telemetry: a model connection's key does not open for the endpoint its row names");
        TelmoniError::Internal(
            "a model connection's key does not open for its endpoint; save it again with its key"
                .into(),
        )
    })?;
    let credentials = Credentials::read(row.provider, &secret).map_err(TelmoniError::BadRequest)?;
    Ok(Connection {
        sign_in: sign_in_for(state, organization_id, project_id, id, &credentials),
        provider: row.provider,
        base_url: row.base_url,
        region: row.region,
        credentials,
    })
}

/// [`open`], for a request about to go: its key's last use stamped, a minute
/// stale at most; a save stamps its own as it writes.
///
/// # Errors
///
/// As [`open`].
pub async fn open_to_send(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    id: Uuid,
) -> Result<Connection, TelmoniError> {
    let connection = open(state, organization_id, project_id, id).await?;
    let mut tx = project_scope(&state.db, project_id).await?;
    sqlx::query(
        "UPDATE telemetry.model_connections SET last_used_at = now()
          WHERE project_id = $1 AND id = $2
            AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
    )
    .bind(project_id)
    .bind(id)
    .execute(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(connection)
}

/// Every connection of `project_id`, its default first.
async fn list(
    state: &AppState,
    project_id: &ProjectId,
) -> Result<Vec<ModelConnection>, TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = sqlx::query_as(
        "SELECT id, name, provider, base_url, region, models, is_default, secret_hint, signs_in, refused_at, refused_reason, last_used_at, created_at, updated_at
           FROM telemetry.model_connections WHERE project_id = $1
          ORDER BY is_default DESC, name",
    )
    .bind(project_id)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(rows)
}

/// `GET /internal/v1/telemetry/model-connections` — the project's connections,
/// every member's to read, no key among them.
pub async fn get_connections(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ModelConnections>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id
        .clone();
    let federation = state.federation.as_ref().map(|federation| FederationSetup {
        issuer: federation.issuer(&project_id),
        subject: model::federation::Federation::subject(&acting.organization_id, &project_id),
        aws_audience: model::federation::AWS_AUDIENCE,
    });
    Ok(Json(ModelConnections {
        connections: list(&state, &project_id).await?,
        federation,
    }))
}

/// `PUT /internal/v1/telemetry/model-connections` — a connection made, or
/// changed by its id, by an owner or admin. Its first model is asked one
/// small judgement first, and a refusal saves nothing, naming the
/// provider's answer; then it is filed in the chain by its provider and its
/// models' count, never its key, and written. A default set unsets the last.
pub async fn put_connection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(change): Json<ConnectionChange>,
) -> Result<Json<ModelConnection>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Project)?
        .project_id
        .clone();
    let kek = kek(&state)?;
    let change = checked(change)?;
    let region = change
        .region
        .as_deref()
        .map(model::Region::try_new)
        .transpose()
        .map_err(TelmoniError::BadRequest)?;
    let id = change.id.unwrap_or_else(Uuid::new_v4);
    let fresh = change.id.is_none();
    let (connection, sealed) = match change.secret.as_ref() {
        Some(secret) => {
            let credentials =
                Credentials::read(change.provider, secret).map_err(TelmoniError::BadRequest)?;
            if matches!(credentials, Credentials::Federated(_)) && state.federation.is_none() {
                return Err(TelmoniError::BadRequest(
                    "this deployment signs no project in, so a connection holds a key: \
                     MODEL_FEDERATION_SIGNER is unset"
                        .into(),
                ));
            }
            let hint = hint(change.provider, &credentials, secret.expose());
            let dek = state.vault.new_dek()?;
            let wrapped = kek
                .wrap(&state.http, &dek, id.as_bytes())
                .await
                .map_err(|e| {
                    TelmoniError::Internal(format!(
                        "a model connection's key could not be sealed: {e}"
                    ))
                })?;
            let bound = sealed_for(
                id,
                &project_id,
                change.provider,
                change.base_url.as_deref(),
                change.region.as_deref(),
            );
            let field = state.vault.seal(&dek, secret, &bound)?;
            let connection = Connection {
                sign_in: sign_in_for(
                    &state,
                    &acting.organization_id,
                    &project_id,
                    id,
                    &credentials,
                ),
                provider: change.provider,
                base_url: change.base_url.clone(),
                region: region.clone(),
                credentials,
            };
            (connection, Some((hint, field, wrapped)))
        }
        None if fresh => {
            return Err(TelmoniError::BadRequest(
                "a new connection carries its key".into(),
            ));
        }
        None => {
            let kept = open(&state, &acting.organization_id, &project_id, id).await?;
            if kept.provider != change.provider {
                return Err(TelmoniError::BadRequest(
                    "a connection's provider changes with a key of its own".into(),
                ));
            }
            // A stolen session cannot point a saved key at a host of its own.
            if kept.base_url != change.base_url || kept.region != region {
                return Err(TelmoniError::BadRequest(
                    "a connection's endpoint or region changes with its key: give the key again"
                        .into(),
                ));
            }
            (kept, None)
        }
    };
    let signs_in = matches!(connection.credentials, Credentials::Federated(_));
    let first = change.models.first().cloned().unwrap_or_default();
    try_connection(&state, &connection, &first).await?;

    let auth = &*state.auth;
    let written = chain::write(&state.db, &acting, &project_id, "a model connection", async |tx| {
        chain::held(auth, &acting, tx, Verb::Update, Resource::Project).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM telemetry.model_connections WHERE project_id = $1 AND id != $2")
            .bind(&project_id)
            .bind(id)
            .fetch_one(tx.conn())
            .await?;
        if !fresh {
            let held: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM telemetry.model_connections WHERE project_id = $1 AND id = $2)",
            )
            .bind(&project_id)
            .bind(id)
            .fetch_one(tx.conn())
            .await?;
            if !held {
                return Err(AuthError::NotFound("no model connection with that id in this project".into()).into());
            }
        }
        if fresh && usize::try_from(count).unwrap_or(usize::MAX) >= CONNECTIONS_MAX {
            return Err(TelmoniError::BadRequest(format!("a project holds {CONNECTIONS_MAX} connections at most")));
        }
        // The first connection is the default, whatever it asks.
        let is_default = change.is_default || count == 0;
        if is_default {
            sqlx::query("UPDATE telemetry.model_connections SET is_default = false WHERE project_id = $1 AND id != $2")
                .bind(&project_id)
                .bind(id)
                .execute(tx.conn())
                .await?;
        }
        // Where a new key's or sign-in's predecessor went, for the notice and the chain.
        let before: Option<Before> = match (&sealed, fresh) {
            (Some(_), false) => sqlx::query_as(
                "SELECT secret_hint, base_url, region, signs_in FROM telemetry.model_connections
                  WHERE project_id = $1 AND id = $2",
            )
            .bind(&project_id)
            .bind(id)
            .fetch_optional(tx.conn())
            .await?,
            _ => None,
        };
        let row: Option<ModelConnection> = match &sealed {
            Some((hint, field, wrapped)) => sqlx::query_as(
                "INSERT INTO telemetry.model_connections
                     (id, project_id, name, provider, base_url, region, models, is_default, secret_hint,
                      secret_ciphertext, secret_nonce, wrapped_dek, key_version, created_by, signs_in,
                      last_used_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, now())
                 ON CONFLICT (id) DO UPDATE SET
                     name = excluded.name, provider = excluded.provider, base_url = excluded.base_url,
                     region = excluded.region, models = excluded.models, is_default = excluded.is_default,
                     secret_hint = excluded.secret_hint, signs_in = excluded.signs_in,
                     secret_ciphertext = excluded.secret_ciphertext,
                     secret_nonce = excluded.secret_nonce, wrapped_dek = excluded.wrapped_dek,
                     key_version = excluded.key_version, refused_at = NULL, refused_reason = NULL,
                     last_used_at = now(), updated_at = now()
                 WHERE telemetry.model_connections.project_id = excluded.project_id
                 RETURNING id, name, provider, base_url, region, models, is_default, secret_hint, signs_in, refused_at, refused_reason, last_used_at, created_at, updated_at",
            )
            .bind(id)
            .bind(&project_id)
            .bind(&change.name)
            .bind(change.provider)
            .bind(&change.base_url)
            .bind(&change.region)
            .bind(&change.models)
            .bind(is_default)
            .bind(hint)
            .bind(&field.ciphertext)
            .bind(&field.nonce)
            .bind(wrapped)
            .bind(KEK_VERSION)
            .bind(acting.user_id.as_str())
            .bind(signs_in)
            .fetch_optional(tx.conn())
            .await?,
            None => sqlx::query_as(
                "UPDATE telemetry.model_connections
                    SET name = $3, base_url = $4, region = $5, models = $6, is_default = $7,
                        refused_at = NULL, refused_reason = NULL, last_used_at = now(),
                        updated_at = now()
                  WHERE project_id = $1 AND id = $2
              RETURNING id, name, provider, base_url, region, models, is_default, secret_hint, signs_in, refused_at, refused_reason, last_used_at, created_at, updated_at",
            )
            .bind(&project_id)
            .bind(id)
            .bind(&change.name)
            .bind(&change.base_url)
            .bind(&change.region)
            .bind(&change.models)
            .bind(is_default)
            .fetch_optional(tx.conn())
            .await?,
        };
        let Some(row) = row else {
            return Err(AuthError::NotFound("no model connection with that id in this project".into()).into());
        };
        let details = json!({
            "provider": row.provider, "models": row.models.len(), "default": row.is_default,
            "key_changed": sealed.is_some(), "endpoint": endpoint_of(&row),
            "signs_in_as": row.signs_in.then(|| row.secret_hint.clone()),
            "before": before.as_ref().map(|b| json!({
                "endpoint": b.endpoint(), "signs_in_as": b.signs_in.then(|| b.secret_hint.clone()),
            })),
        });
        if fresh {
            emit_audit(
                tx,
                AuditEvent::new(
                    &acting.organization_id,
                    Actor::User(acting.user_id.as_str()),
                    AuditAction::Created,
                    TelmoniResourceKind::ModelConnection,
                )
                .in_project(&project_id)
                .on(&id.to_string())
                .details(details),
            )
            .await?;
        } else {
            emit_audit(
                tx,
                AuditEvent::new(
                    &acting.organization_id,
                    Actor::User(acting.user_id.as_str()),
                    AuditAction::Updated,
                    TelmoniResourceKind::ModelConnection,
                )
                .in_project(&project_id)
                .on(&id.to_string())
                .details(details),
            )
            .await?;
        }
        Ok((row, before))
    })
    .await
    .map_err(|e| match e {
        TelmoniError::Database(sqlx::Error::Database(db)) if db.constraint() == Some("model_connections_name_key") => {
            TelmoniError::BadRequest("another connection of this project goes by that name".into())
        }
        other => other,
    })?;
    let (written, before) = written;
    tracing::info!(project_id = %project_id, connection_id = %id, provider = %written.provider, "telemetry: a model connection saved");
    if sealed.is_some() {
        tell_saved(
            &state,
            &acting.organization_id,
            &project_id,
            &written,
            before.as_ref(),
        );
    }
    Ok(Json(written))
}

/// `GET /oidc/v1/projects/{project_id}/.well-known/openid-configuration` — a
/// project's issuer; any well-formed id answers alike, so none says which exist.
pub async fn discovery(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
) -> Result<axum::response::Response, TelmoniError> {
    use axum::response::IntoResponse as _;
    let (Some(federation), Ok(project_id)) = (&state.federation, ProjectId::try_new(&project_id))
    else {
        return Err(AuthError::NotFound("no issuer here".into()).into());
    };
    Ok((
        [(axum::http::header::CACHE_CONTROL, "public, max-age=3600")],
        axum::Json(federation.discovery(&project_id)),
    )
        .into_response())
}

/// `GET /oidc/v1/jwks` — the key every project's tokens verify under.
pub async fn jwks(
    State(state): State<Arc<AppState>>,
) -> Result<axum::response::Response, TelmoniError> {
    use axum::response::IntoResponse as _;
    let Some(federation) = &state.federation else {
        return Err(AuthError::NotFound("no issuer here".into()).into());
    };
    let jwks = federation.jwks().await.map_err(|e| {
        tracing::warn!(error = %e, "telemetry: the signing key could not be read");
        TelmoniError::Internal("the signing key could not be read".into())
    })?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "public, max-age=300")],
        axum::Json(jwks),
    )
        .into_response())
}

/// Where a connection's key goes: its endpoint's host, or its region; none
/// where the provider's own host is the only one.
fn endpoint_of(connection: &ModelConnection) -> Option<String> {
    host_or_region(connection.base_url.as_deref(), connection.region.as_deref())
}

fn host_or_region(base_url: Option<&str>, region: Option<&str>) -> Option<String> {
    base_url
        .and_then(|base| reqwest::Url::parse(base).ok())
        .and_then(|url| {
            let port = url
                .port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default();
            url.host_str().map(|host| format!("{host}{port}"))
        })
        .or_else(|| region.map(str::to_owned))
}

/// A connection as a save that changes its key or its sign-in finds it.
#[derive(sqlx::FromRow)]
struct Before {
    secret_hint: String,
    base_url: Option<String>,
    region: Option<String>,
    signs_in: bool,
}

impl Before {
    fn endpoint(&self) -> Option<String> {
        host_or_region(self.base_url.as_deref(), self.region.as_deref())
    }
}

/// What a connection holds and where it goes, in a notice's words.
fn place(signs_in: bool, hint: &str, endpoint: Option<&str>) -> String {
    let at = endpoint.map(|at| format!(" in {at}")).unwrap_or_default();
    if signs_in {
        format!("a sign-in as the project to {hint}{at}, keeping no key")
    } else if hint.is_empty() {
        format!(
            "no key{}",
            endpoint
                .map(|at| format!(", sent to {at}"))
                .unwrap_or_default()
        )
    } else {
        format!(
            "its key …{hint}, sent to {}",
            endpoint.unwrap_or("its provider's own host")
        )
    }
}

/// The waits before each try of telling the owners of a save: at once, then
/// past a notifications outage's usual minutes.
const TELL_WAITS: [std::time::Duration; 3] = [
    std::time::Duration::ZERO,
    std::time::Duration::from_secs(30),
    std::time::Duration::from_secs(300),
];

/// The owners and admins told, apart from the answer and again where it
/// fails, of each key or sign-in saved: the project, where it goes and went.
fn tell_saved(
    state: &Arc<AppState>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    saved: &ModelConnection,
    before: Option<&Before>,
) {
    let endpoint = endpoint_of(saved);
    let now = place(saved.signs_in, &saved.secret_hint, endpoint.as_deref());
    let then = before.map(|before| {
        place(
            before.signs_in,
            &before.secret_hint,
            before.endpoint().as_deref(),
        )
    });
    let provider = saved.provider.to_string();
    let undo = if saved.signs_in {
        "take the project's trust off that role or pool"
    } else {
        "revoke the key at its provider"
    };
    let title = if saved.signs_in {
        "A model connection's sign-in was saved"
    } else {
        "A model connection's key was saved"
    };
    let dedup = format!(
        "model-connections:{}:{}",
        saved.id,
        saved.updated_at.timestamp_micros()
    );
    let metadata = json!({
        "lane": "model_connections", "project_id": project_id, "connection_id": saved.id,
        "provider": saved.provider, "endpoint": endpoint,
    });
    let (state, organization_id, project_id, connection_id) = (
        Arc::clone(state),
        organization_id.clone(),
        project_id.clone(),
        saved.id,
    );
    tokio::spawn(async move {
        let (project, path) = settings_of(&state, &project_id).await;
        let what = match &then {
            None => format!("A {provider} model connection was made in {project}, with {now}."),
            Some(then) => format!(
                "A {provider} model connection of {project} was saved again, with {now}, in \
                 place of {then}."
            ),
        };
        let body = format!(
            "{what} If nobody of yours saved it, delete the connection on the project's Settings \
             page and {undo}; the organization's audit log names who saved it."
        );
        for wait in TELL_WAITS {
            tokio::time::sleep(wait).await;
            let alert = telmoni_shared::seam::OrganizationAlert {
                title,
                body: &body,
                metadata: metadata.clone(),
                dedup_key: Some(&dedup),
            };
            if telmoni_shared::seam::tell_owners(
                state.notifications.as_ref(),
                state.auth.as_ref(),
                &organization_id,
                alert,
                &path,
            )
            .await
            {
                return;
            }
        }
        tracing::error!(project_id = %project_id, connection_id = %connection_id,
            "telemetry: the owners were never told a model connection was saved; its audit row stands");
    });
}

/// The project in a notice's words, and its Settings page's path; one of
/// the organization's projects, and its root, where auth cannot say.
pub(crate) async fn settings_of(state: &AppState, project_id: &ProjectId) -> (String, String) {
    let home = state
        .auth
        .project_homes(std::slice::from_ref(project_id))
        .await
        .ok()
        .and_then(|homes| homes.into_iter().find(|h| h.project_id == *project_id));
    home.map_or_else(
        || ("one of your projects".to_owned(), "/".to_owned()),
        |h| {
            (
                format!("the project {}", h.slug),
                format!("/{}/{}/settings", h.organization_slug, h.slug),
            )
        },
    )
}

/// `DELETE /internal/v1/telemetry/model-connections/{id}` — a connection and
/// its key gone, by an owner or admin, filed in the chain.
pub async fn delete_connection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Project)?
        .project_id
        .clone();
    let auth = &*state.auth;
    chain::write(
        &state.db,
        &acting,
        &project_id,
        "a model connection",
        async |tx| {
            chain::held(auth, &acting, tx, Verb::Update, Resource::Project).await?;
            let deleted: Option<(ModelProvider, Option<String>, Option<String>)> = sqlx::query_as(
                "DELETE FROM telemetry.model_connections WHERE project_id = $1 AND id = $2
             RETURNING provider, base_url, region",
            )
            .bind(&project_id)
            .bind(id)
            .fetch_optional(tx.conn())
            .await?;
            let Some((provider, base_url, region)) = deleted else {
                return Err(AuthError::NotFound(
                    "no model connection with that id in this project".into(),
                )
                .into());
            };
            let endpoint = host_or_region(base_url.as_deref(), region.as_deref());
            emit_audit(
                tx,
                AuditEvent::new(
                    &acting.organization_id,
                    Actor::User(acting.user_id.as_str()),
                    AuditAction::Deleted,
                    TelmoniResourceKind::ModelConnection,
                )
                .in_project(&project_id)
                .on(&id.to_string())
                .details(json!({ "provider": provider, "endpoint": endpoint })),
            )
            .await?;
            Ok(())
        },
    )
    .await?;
    tracing::info!(project_id = %project_id, connection_id = %id, "telemetry: a model connection deleted");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(provider: ModelProvider) -> ConnectionChange {
        ConnectionChange {
            id: None,
            name: " Judge ".into(),
            provider,
            base_url: None,
            region: None,
            models: vec![" gpt-x ".into(), "gpt-x".into()],
            is_default: false,
            secret: Some("sk-1234".into()),
        }
    }

    /// A connection is held to its provider's rules: an endpoint where it
    /// answers at one of the customer's, a region where Bedrock's names one.
    #[test]
    fn a_connection_is_held_to_its_providers_rules() {
        let kept = checked(change(ModelProvider::Openai)).expect("kept");
        assert_eq!(
            (kept.name.as_str(), kept.models.as_slice()),
            ("Judge", &["gpt-x".to_owned()][..])
        );
        assert!(
            checked(ConnectionChange {
                base_url: Some("https://x".into()),
                ..change(ModelProvider::Openai)
            })
            .is_err()
        );
        assert!(
            checked(change(ModelProvider::OpenaiCompatible)).is_err(),
            "an endpoint of its own"
        );
        assert!(
            checked(ConnectionChange {
                base_url: Some("https://gw.example/v1/".into()),
                ..change(ModelProvider::OpenaiCompatible)
            })
            .is_ok()
        );
        assert!(checked(change(ModelProvider::Bedrock)).is_err(), "a region");
        assert!(
            checked(ConnectionChange {
                region: Some("us-east-1".into()),
                ..change(ModelProvider::Bedrock)
            })
            .is_ok()
        );
        assert!(
            checked(ConnectionChange {
                region: Some("us-east-1".into()),
                ..change(ModelProvider::Anthropic)
            })
            .is_err()
        );
        assert!(
            checked(ConnectionChange {
                models: vec![],
                ..change(ModelProvider::Openai)
            })
            .is_err()
        );
    }

    /// A key is shown by its last four characters, a pair by its id's, a
    /// service account by its email.
    #[test]
    fn a_key_is_shown_by_its_last_four() {
        let key = Credentials::ApiKey("sk-abcdWXYZ".to_owned().into());
        assert_eq!(hint(ModelProvider::Openai, &key, "sk-abcdWXYZ"), "WXYZ");
        let pair = Credentials::AccessKeys {
            access_key_id: "AKIA1234".into(),
            secret_access_key: "s".to_owned().into(),
            session_token: None,
        };
        assert_eq!(hint(ModelProvider::Bedrock, &pair, ""), "1234");
    }
}
