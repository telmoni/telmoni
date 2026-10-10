//! What this module does for the modules beside it —
//! [`telmoni_shared::seam::Telemetry`], on a handle to the module's state.
//! Auth runs its purges and a project's move through this.

use std::sync::Arc;

use async_trait::async_trait;

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::seam::Telemetry;
use telmoni_shared::{OrganizationId, ProjectId, TelmoniError};

use crate::AppState;
use crate::db::{self, TelemetryLane};

/// The module as its siblings hold it: `Arc::new(TelemetrySeam(state))` is
/// the `Arc<dyn Telemetry>` auth is handed.
pub struct TelemetrySeam(pub Arc<AppState>);

#[async_trait]
impl Telemetry for TelemetrySeam {
    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError> {
        let mut tx = maintenance_scope(&self.0.db, TelemetryLane).await?;
        let purged = db::purge_organization(&mut tx, organization_id).await?;
        tx.commit().await?;
        tracing::info!(
            organization_id = %organization_id,
            purged,
            "telemetry purge: the organization's project settings removed"
        );
        Ok(purged)
    }

    async fn purge_project(&self, project_id: &ProjectId) -> Result<u64, TelmoniError> {
        let mut tx = maintenance_scope(&self.0.db, TelemetryLane).await?;
        let purged = db::purge_project(&mut tx, project_id).await?;
        tx.commit().await?;
        tracing::info!(
            project_id = %project_id,
            purged,
            "telemetry purge: the project's settings removed"
        );
        Ok(purged)
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
