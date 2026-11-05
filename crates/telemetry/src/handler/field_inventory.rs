//! The field inventory as the console reads it: `GET
//! /internal/v1/telemetry/field-inventory`, every attribute ingest reads and
//! what becomes of each ([`crate::contract::INVENTORY`]). The same list for
//! every project, read by every role on one, served from the code so the
//! console's copy cannot say anything the docs page and ingest do not.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use serde::Serialize;

use telmoni_shared::TelmoniError;
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};

use crate::AppState;
use crate::contract::{self, InventoryRow, Person};

/// What the lane answers.
#[derive(Debug, Serialize)]
pub struct FieldInventory {
    /// The commit of the GenAI semantic conventions the names are taken from.
    pub conventions_commit: &'static str,
    /// Every row, in the page's order.
    pub rows: Vec<Row>,
}

/// One row, and whether its field can carry a person.
#[derive(Debug, Serialize)]
pub struct Row {
    #[serde(flatten)]
    pub row: &'static InventoryRow,
    pub person: Person,
}

/// `GET /internal/v1/telemetry/field-inventory` — the inventory, to anyone on
/// the project `x-project-id` names.
pub async fn get_field_inventory(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<FieldInventory>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    acting.require_project(Verb::Read, Resource::Project)?;
    Ok(Json(FieldInventory {
        conventions_commit: contract::CONVENTIONS_COMMIT,
        rows: contract::INVENTORY
            .iter()
            .map(|row| Row {
                row,
                person: row.person(),
            })
            .collect(),
    }))
}
