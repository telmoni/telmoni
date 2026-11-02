//! A project's model connections, the customer's own keys its evaluators
//! judge with: owners and admins connect, change and delete one, each change
//! filed in the chain; every member reads them, never a key. A key is sealed
//! under its own data key, wrapped by `MODEL_CONNECTION_KEK`, and opened only
//! by a save's test and the evaluation loop.

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
use telmoni_shared::model::{self, Ask, Connection, Credentials, Role};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, ModelProvider, ProjectId, Redacted, TelmoniError, TelmoniResourceKind,
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
    /// The key's last four characters, as an API key's are shown.
    pub secret_hint: String,
    /// When its provider last refused its key, and its words: its evaluators
    /// wait until a save proves it again.
    pub refused_at: Option<DateTime<Utc>>,
    pub refused_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The project's connections, its default first.
#[derive(Debug, Serialize)]
pub struct ModelConnections {
    pub connections: Vec<ModelConnection>,
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
    pub secret: Option<String>,
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
        ModelProvider::AzureOpenai | ModelProvider::OpenaiCompatible
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
        (Some(region), ModelProvider::Bedrock | ModelProvider::VertexAi) => {
            Some(model::region(region).map_err(TelmoniError::BadRequest)?)
        }
        (None, ModelProvider::Bedrock) => return refuse("a Bedrock connection names its region"),
        (None, _) => None,
        (Some(_), _) => return refuse("only Bedrock and Vertex AI connections name a region"),
    };
    if change.secret.as_ref().is_some_and(|s| s.len() > SECRET_MAX) {
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
        Credentials::AccessKeys { access_key_id, .. } => tail(access_key_id),
        Credentials::ApiKey(_)
            if secret.trim().is_empty() && provider == ModelProvider::OpenaiCompatible =>
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

/// One small judgement of `connection`'s first model: the save's proof that
/// the key, the endpoint and the model answer.
async fn try_connection(
    state: &AppState,
    connection: &Connection,
    model: &str,
) -> Result<(), TelmoniError> {
    let schema = test_schema();
    let messages = [(Role::User, "Answer {\"ok\": true}.".to_owned())];
    let ask = Ask {
        model,
        system: "You check that a connection answers. Answer with the JSON asked for.",
        messages: &messages,
        schema: &schema,
        temperature: None,
        max_tokens: 64,
    };
    model::judge(&state.egress, connection, &ask)
        .await
        .map(|_| ())
        .map_err(|refused| {
            TelmoniError::BadRequest(format!(
                "the connection was not saved: {}",
                refused.sentence()
            ))
        })
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
    region: Option<String>,
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
    let opened = kek
        .open_row(
            &state.http,
            &state.vault,
            id,
            &row.wrapped_dek,
            row.key_version,
        )
        .await
        .map_err(|e| {
            TelmoniError::Internal(format!(
                "a model connection's key could not be unwrapped: {e}"
            ))
        })?;
    let secret = opened.open(SealedRef {
        ciphertext: &row.secret_ciphertext,
        nonce: &row.secret_nonce,
    })?;
    let credentials = Credentials::read(row.provider, &secret).map_err(TelmoniError::BadRequest)?;
    Ok(Connection {
        provider: row.provider,
        base_url: row.base_url,
        region: row.region,
        credentials,
    })
}

/// Every connection of `project_id`, its default first.
async fn list(
    state: &AppState,
    project_id: &ProjectId,
) -> Result<Vec<ModelConnection>, TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = sqlx::query_as(
        "SELECT id, name, provider, base_url, region, models, is_default, secret_hint, refused_at, refused_reason, created_at, updated_at
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
    Ok(Json(ModelConnections {
        connections: list(&state, &project_id).await?,
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
    let id = change.id.unwrap_or_else(Uuid::new_v4);
    let fresh = change.id.is_none();
    let (connection, sealed) = match change.secret.as_deref() {
        Some(secret) => {
            let secret: Redacted = secret.to_owned().into();
            let credentials =
                Credentials::read(change.provider, &secret).map_err(TelmoniError::BadRequest)?;
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
            let field = state.vault.seal(&dek, &secret, id.as_bytes())?;
            let connection = Connection {
                provider: change.provider,
                base_url: change.base_url.clone(),
                region: change.region.clone(),
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
            let kept = open(&state, &project_id, id).await?;
            if kept.provider != change.provider {
                return Err(TelmoniError::BadRequest(
                    "a connection's provider changes with a key of its own".into(),
                ));
            }
            let connection = Connection {
                base_url: change.base_url.clone(),
                region: change.region.clone(),
                ..kept
            };
            (connection, None)
        }
    };
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
        let row: Option<ModelConnection> = match &sealed {
            Some((hint, field, wrapped)) => sqlx::query_as(
                "INSERT INTO telemetry.model_connections
                     (id, project_id, name, provider, base_url, region, models, is_default, secret_hint,
                      secret_ciphertext, secret_nonce, wrapped_dek, key_version, created_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                 ON CONFLICT (id) DO UPDATE SET
                     name = excluded.name, provider = excluded.provider, base_url = excluded.base_url,
                     region = excluded.region, models = excluded.models, is_default = excluded.is_default,
                     secret_hint = excluded.secret_hint, secret_ciphertext = excluded.secret_ciphertext,
                     secret_nonce = excluded.secret_nonce, wrapped_dek = excluded.wrapped_dek,
                     key_version = excluded.key_version, refused_at = NULL, refused_reason = NULL,
                     updated_at = now()
                 WHERE telemetry.model_connections.project_id = excluded.project_id
                 RETURNING id, name, provider, base_url, region, models, is_default, secret_hint, refused_at, refused_reason, created_at, updated_at",
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
            .fetch_optional(tx.conn())
            .await?,
            None => sqlx::query_as(
                "UPDATE telemetry.model_connections
                    SET name = $3, base_url = $4, region = $5, models = $6, is_default = $7,
                        refused_at = NULL, refused_reason = NULL, updated_at = now()
                  WHERE project_id = $1 AND id = $2
              RETURNING id, name, provider, base_url, region, models, is_default, secret_hint, refused_at, refused_reason, created_at, updated_at",
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
            "key_changed": sealed.is_some(),
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
        Ok(row)
    })
    .await
    .map_err(|e| match e {
        TelmoniError::Database(sqlx::Error::Database(db)) if db.constraint() == Some("model_connections_name_key") => {
            TelmoniError::BadRequest("another connection of this project goes by that name".into())
        }
        other => other,
    })?;
    tracing::info!(project_id = %project_id, connection_id = %id, provider = %written.provider, "telemetry: a model connection saved");
    Ok(Json(written))
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
    chain::write(&state.db, &acting, &project_id, "a model connection", async |tx| {
        chain::held(auth, &acting, tx, Verb::Update, Resource::Project).await?;
        let provider: Option<ModelProvider> = sqlx::query_scalar(
            "DELETE FROM telemetry.model_connections WHERE project_id = $1 AND id = $2 RETURNING provider",
        )
        .bind(&project_id)
        .bind(id)
        .fetch_optional(tx.conn())
        .await?;
        let Some(provider) = provider else {
            return Err(AuthError::NotFound("no model connection with that id in this project".into()).into());
        };
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
            .details(json!({ "provider": provider })),
        )
        .await?;
        Ok(())
    })
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
        };
        assert_eq!(hint(ModelProvider::Bedrock, &pair, ""), "1234");
    }
}
