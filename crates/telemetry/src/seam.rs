//! What this module does for the modules beside it —
//! [`telmoni_shared::seam::Telemetry`], on a handle to the module's state.
//! Auth runs its purges and a project's move through this.
//!
//! A purge takes a project's spans and totals with its settings and saved
//! views: the rows marked deleted in ClickHouse first, before the settings
//! row goes, so a purge cut short leaves the row the sweep finds it by
//! ([`crate::sweep`]); then the views and the row removed. An organization's
//! projects are the ones its settings rows name, since neither a span nor a
//! view names an organization.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use telmoni_shared::audit::requested_by;
use telmoni_shared::db::tenant_session::{maintenance_scope, organization_scope};
use telmoni_shared::seam::{Receipt, Telemetry};
use telmoni_shared::{
    AuditAction, OrganizationId, ProjectId, TelemetryError, TelmoniError, TelmoniResourceKind,
};

use crate::AppState;
use crate::db::{self, TelemetryLane};
use crate::store;

/// How long a project's receipt stays claimed: past any purge's retry.
const RECEIPT_CLAIMED_SECS: u64 = 24 * 60 * 60;

/// The module as its siblings hold it: `Arc::new(TelemetrySeam(state))` is
/// the `Arc<dyn Telemetry>` auth is handed.
pub struct TelemetrySeam(pub Arc<AppState>);

/// `project`'s spans and totals marked deleted in ClickHouse, or why not: a
/// store that did not answer is the retried kind, a shape of ours it refused
/// is not.
async fn mark_deleted(state: &AppState, project: &ProjectId) -> Result<(), TelmoniError> {
    state.store.delete_project(project).await.map_err(|error| {
        // ⚠ The error's text can carry ClickHouse's own message: its kind
        // alone is logged, beside the project.
        let fault = store::Fault::of(&error);
        if fault == store::Fault::Ours {
            tracing::error!(project_id = %project, kind = fault.as_str(), "telemetry purge: ClickHouse refused the project's delete");
            return TelmoniError::Internal("telemetry purge: ClickHouse refused the delete".into());
        }
        tracing::warn!(project_id = %project, kind = fault.as_str(), "telemetry purge: ClickHouse did not take the project's delete");
        TelemetryError::StoreUnavailable("the span store did not answer; the purge is retried".into())
            .into()
    })
}

/// One project's purge, as auth calls it after the delete and the sweep
/// calls it again: its spans and totals, then the rest, then the receipt to
/// whoever `organization_id`'s chain names as asking. Safe to repeat;
/// answers how many rows went from Postgres.
pub(crate) async fn purge_project(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
) -> Result<u64, TelmoniError> {
    mark_deleted(state, project_id).await?;
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let purged = db::purge_project(&mut tx, project_id).await?;
    tx.commit().await?;
    tracing::info!(
        project_id = %project_id,
        purged,
        "telemetry purge: the project's rows marked deleted and its settings and views removed"
    );
    mail_project_receipt(state, organization_id, project_id).await;
    Ok(purged)
}

/// The project's receipt, mailed apart from the purge, so a slow mail is
/// neither held to the purge's budget nor cut short with it: to the person
/// whose deletion the organization's chain filed, under the name it filed.
/// Claimed in Redis first, so the delete's purge and the sweep's, racing,
/// mail it once; without Redis it is mailed all the same.
async fn mail_project_receipt(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
) {
    let (db, auth) = (state.db.clone(), Arc::clone(&state.auth));
    let (organization_id, project_id) = (organization_id.clone(), project_id.clone());
    let connection = state.redis.connection().await;
    tokio::spawn(async move {
        if let Some(mut connection) = connection {
            let claimed: Result<Option<String>, redis::RedisError> = redis::cmd("SET")
                .arg(format!("telemetry:receipt:project:{project_id}"))
                .arg(1)
                .arg("NX")
                .arg("EX")
                .arg(RECEIPT_CLAIMED_SECS)
                .query_async(&mut connection)
                .await;
            if matches!(claimed, Ok(None)) {
                return;
            }
        }
        let asked = async {
            let mut tx = organization_scope(&db, &organization_id).await?;
            let asked = requested_by(
                tx.conn(),
                &organization_id,
                AuditAction::Deleted,
                TelmoniResourceKind::Project,
                project_id.as_str(),
                &json!({}),
            )
            .await?;
            tx.commit().await?;
            Ok::<_, TelmoniError>(asked)
        };
        match asked.await {
            Ok(Some((to, filed))) => {
                let name = filed
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(project_id.as_str());
                auth.mail_receipt(&to, Receipt::Project { name }).await;
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(
                project_id = %project_id,
                error = %e,
                "telemetry purge: who asked for the project's deletion could not be read; no receipt is mailed"
            ),
        }
    });
}

#[async_trait]
impl Telemetry for TelemetrySeam {
    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError> {
        let mut tx = maintenance_scope(&self.0.db, TelemetryLane).await?;
        let projects = db::projects_of(&mut tx, organization_id).await?;
        tx.commit().await?;
        for project in &projects {
            mark_deleted(&self.0, project).await?;
        }
        let mut tx = maintenance_scope(&self.0.db, TelemetryLane).await?;
        let purged = db::purge_organization(&mut tx, organization_id).await?;
        tx.commit().await?;
        tracing::info!(
            organization_id = %organization_id,
            projects = projects.len(),
            purged,
            "telemetry purge: the organization's projects' rows marked deleted and their settings and views removed"
        );
        Ok(purged)
    }

    async fn purge_project(
        &self,
        organization_id: &OrganizationId,
        project_id: &ProjectId,
    ) -> Result<u64, TelmoniError> {
        purge_project(&self.0, organization_id, project_id).await
    }

    async fn admit_data_out(&self, organization_id: &OrganizationId) -> Result<(), TelmoniError> {
        crate::ceilings::admit_data_out(&self.0, organization_id).await
    }

    async fn spend_data_out(&self, organization_id: &OrganizationId, bytes: u64) {
        crate::ceilings::spend_data_out(&self.0, organization_id, bytes).await;
    }

    async fn move_project(
        &self,
        project_id: &ProjectId,
        organization_id: &OrganizationId,
    ) -> Result<(), TelmoniError> {
        let mut tx = maintenance_scope(&self.0.db, TelemetryLane).await?;
        let moved = db::move_project(&mut tx, project_id, organization_id).await?;
        tx.commit().await?;
        tracing::info!(
            project_id = %project_id,
            organization_id = %organization_id,
            moved,
            "telemetry: the project's settings moved with it"
        );
        Ok(())
    }
}
