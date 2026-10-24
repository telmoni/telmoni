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

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::seam::Telemetry;
use telmoni_shared::{OrganizationId, ProjectId, TelemetryError, TelmoniError};

use crate::AppState;
use crate::db::{self, TelemetryLane};
use crate::store;

/// The module as its siblings hold it: `Arc::new(TelemetrySeam(state))` is
/// the `Arc<dyn Telemetry>` auth is handed.
pub struct TelemetrySeam(pub Arc<AppState>);

/// `project`'s spans and totals marked deleted in ClickHouse, or why not: a
/// store that did not answer is the retried kind, a shape of ours it refused
/// is not.
async fn mark_deleted(state: &AppState, project: &ProjectId) -> Result<(), TelmoniError> {
    state.store.delete_project(project).await.map_err(|error| {
        // ⚠ The error's text can carry ClickHouse's own message: its kind
        // alone is logged.
        let kind = store::error_kind(&error);
        if kind == "ours" {
            tracing::error!(project_id = %project, kind, "telemetry purge: ClickHouse refused the project's delete");
            return TelmoniError::Internal("telemetry purge: ClickHouse refused the delete".into());
        }
        tracing::warn!(project_id = %project, kind, "telemetry purge: ClickHouse did not take the project's delete");
        TelemetryError::StoreUnavailable("the span store did not answer; the purge is retried".into())
            .into()
    })
}

/// One project's purge, as auth calls it after the delete and the sweep
/// calls it again: its spans and totals, then the rest. Safe to repeat;
/// answers how many rows went from Postgres.
pub(crate) async fn purge_project(
    state: &AppState,
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
    Ok(purged)
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

    async fn purge_project(&self, project_id: &ProjectId) -> Result<u64, TelmoniError> {
        purge_project(&self.0, project_id).await
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
