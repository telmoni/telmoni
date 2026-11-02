//! A project's own model prices, which spend reads before the table's: `GET
//! /internal/v1/telemetry/prices`, which every role on the project reads, and
//! `PUT`, its owners' and admins', which replaces the list whole and records
//! the change. Each price is US dollars per million tokens on the wire, a
//! token's in the row, as the table keeps it.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{HasProject, Scoped, project_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{AuditAction, ProjectId, TelmoniError, TelmoniResourceKind};

use crate::AppState;
use crate::contract::{MODEL_MAX, PROVIDER_MAX};
use crate::handler::chain;

/// The most prices a project sets.
pub const PRICES_MAX: usize = 100;

/// The most names one price answers as.
pub const MODELS_MAX: usize = 20;

/// A token's price in the row, per million on the wire.
const PER_MILLION: f64 = 1_000_000.0;

/// A row's price per million, to twelve figures, so a price saved reads back as typed.
fn per_million(per_token: f64) -> f64 {
    let price = per_token * PER_MILLION;
    format!("{price:.11e}").parse().unwrap_or(price)
}

/// The most a price is, per million tokens: a typo past it is refused.
const PRICE_MAX: f64 = 10_000.0;

/// One price, as the Settings page reads and writes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPrice {
    /// The provider, as the GenAI conventions spell it (`gen_ai.provider.name`).
    pub provider: String,
    /// The model, by every name it answers with (`gen_ai.response.model`).
    pub models: Vec<String>,
    /// The day it takes effect, from midnight UTC; none for always.
    #[serde(default)]
    pub effective_from: Option<NaiveDate>,
    /// US dollars per million input tokens no cache served.
    pub input: f64,
    /// Per million output tokens, reasoning among them.
    pub output: f64,
    /// Per million input tokens read from the provider's cache.
    pub cache_read: f64,
    /// Per million input tokens written to it.
    pub cache_write: f64,
}

/// What both lanes answer, and what `PUT` takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPrices {
    pub prices: Vec<ProjectPrice>,
}

/// A price as its row keeps it: a token's four prices.
type PriceRow = (String, Vec<String>, Option<NaiveDate>, f64, f64, f64, f64);

/// The project's prices, in the order set.
async fn read(
    tx: &mut Scoped<'_, impl HasProject>,
    project_id: &ProjectId,
) -> Result<Vec<ProjectPrice>, TelmoniError> {
    let rows: Vec<PriceRow> = sqlx::query_as(
        "SELECT provider, models, effective_from, input, output, cache_read, cache_write
           FROM telemetry.project_prices
          WHERE project_id = $1 ORDER BY position",
    )
    .bind(project_id)
    .fetch_all(tx.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(provider, models, effective_from, input, output, cache_read, cache_write)| {
                ProjectPrice {
                    provider,
                    models,
                    effective_from,
                    input: per_million(input),
                    output: per_million(output),
                    cache_read: per_million(cache_read),
                    cache_write: per_million(cache_write),
                }
            },
        )
        .collect())
}

/// `GET /internal/v1/telemetry/prices` — the project's own prices.
pub async fn get_prices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ProjectPrices>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let prices = read(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(Json(ProjectPrices { prices }))
}

/// Each price held to its rules: a provider and its names under the
/// contract's caps, no name twice for one provider and day, and four prices
/// from 0 up to [`PRICE_MAX`].
fn checked(prices: &[ProjectPrice]) -> Result<(), TelmoniError> {
    let refuse = |why: String| Err(TelmoniError::BadRequest(why));
    if prices.len() > PRICES_MAX {
        return refuse(format!("a project sets up to {PRICES_MAX} prices"));
    }
    let mut seen = std::collections::HashSet::new();
    for price in prices {
        let provider = price.provider.trim();
        if provider.is_empty() || provider.chars().count() > PROVIDER_MAX {
            return refuse(format!("a provider is 1 to {PROVIDER_MAX} characters"));
        }
        if price.models.is_empty() || price.models.len() > MODELS_MAX {
            return refuse(format!("a price names 1 to {MODELS_MAX} models"));
        }
        for model in &price.models {
            if model.trim().is_empty() || model.chars().count() > MODEL_MAX {
                return refuse(format!("a model is 1 to {MODEL_MAX} characters"));
            }
            if !seen.insert((provider.to_owned(), model.clone(), price.effective_from)) {
                return refuse(format!(
                    "{model} is priced twice from the same day for {provider}"
                ));
            }
        }
        for amount in [
            price.input,
            price.output,
            price.cache_read,
            price.cache_write,
        ] {
            if !amount.is_finite() || !(0.0..=PRICE_MAX).contains(&amount) {
                return refuse(format!(
                    "a price is 0 to {PRICE_MAX} US dollars per million tokens"
                ));
            }
        }
    }
    Ok(())
}

/// `PUT /internal/v1/telemetry/prices` — the project's prices replaced whole,
/// under the organization's chain lock, the role checked again there, and a
/// change recorded with how many there were and are.
pub async fn put_prices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(change): Json<ProjectPrices>,
) -> Result<Json<ProjectPrices>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Project)?
        .project_id
        .clone();
    let prices: Vec<ProjectPrice> = change
        .prices
        .into_iter()
        .map(|price| ProjectPrice {
            provider: price.provider.trim().to_owned(),
            models: price.models.iter().map(|m| m.trim().to_owned()).collect(),
            ..price
        })
        .collect();
    checked(&prices)?;
    let auth = &*state.auth;
    let written = prices.clone();
    chain::write(&state.db, &acting, &project_id, "prices", async |tx| {
        chain::held(auth, &acting, tx, Verb::Update, Resource::Project).await?;
        let before = read(tx, &project_id).await?;
        if before == written {
            return Ok(());
        }
        sqlx::query("DELETE FROM telemetry.project_prices WHERE project_id = $1")
            .bind(&project_id)
            .execute(tx.conn())
            .await?;
        for (position, price) in written.iter().enumerate() {
            sqlx::query(
                "INSERT INTO telemetry.project_prices
                     (project_id, position, provider, models, effective_from,
                      input, output, cache_read, cache_write)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(&project_id)
            .bind(i32::try_from(position).unwrap_or(i32::MAX))
            .bind(&price.provider)
            .bind(&price.models)
            .bind(price.effective_from)
            .bind(price.input / PER_MILLION)
            .bind(price.output / PER_MILLION)
            .bind(price.cache_read / PER_MILLION)
            .bind(price.cache_write / PER_MILLION)
            .execute(tx.conn())
            .await?;
        }
        emit_audit(
            tx,
            AuditEvent::new(
                &acting.organization_id,
                Actor::User(acting.user_id.as_str()),
                AuditAction::Updated,
                TelmoniResourceKind::Project,
            )
            .in_project(&project_id)
            .on(project_id.as_str())
            .details(json!({ "prices": { "from": before.len(), "to": written.len() } })),
        )
        .await?;
        Ok(())
    })
    .await?;
    tracing::info!(project_id = %project_id, prices = prices.len(), "telemetry: a project's prices set");
    Ok(Json(ProjectPrices { prices }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price(models: &[&str], input: f64) -> ProjectPrice {
        ProjectPrice {
            provider: "openai".into(),
            models: models.iter().map(|m| (*m).to_owned()).collect(),
            effective_from: None,
            input,
            output: 4.0 * input,
            cache_read: input / 10.0,
            cache_write: input,
        }
    }

    /// A price list is held to its caps, one name priced twice a day and no amount among them.
    #[test]
    fn a_price_list_is_held_to_its_rules() {
        assert!(checked(&[price(&["ft:gpt-5:acme"], 3.0)]).is_ok());
        assert!(checked(&[price(&[], 3.0)]).is_err());
        assert!(checked(&[price(&["a"], -1.0)]).is_err());
        assert!(checked(&[price(&["a"], f64::NAN)]).is_err());
        assert!(checked(&[price(&["a"], PRICE_MAX * 2.0)]).is_err());
        assert!(checked(&[price(&["a"], 1.0), price(&["a"], 2.0)]).is_err());
        let later = ProjectPrice {
            effective_from: Some("2026-12-01".parse().expect("a day")),
            ..price(&["a"], 2.0)
        };
        assert!(
            checked(&[price(&["a"], 1.0), later]).is_ok(),
            "a new price from a day"
        );
        assert!(checked(&vec![price(&["a"], 1.0); PRICES_MAX + 1]).is_err());
    }

    /// A price per million reads back from its row as it was written.
    #[test]
    fn a_price_reads_back_as_written() {
        for typed in [0.061, 0.15, 2.5, 3.0, 0.0, 1e-3, 75.0] {
            assert_eq!(per_million(typed / PER_MILLION), typed);
        }
    }
}
