//! What this module does for the modules beside it —
//! [`telmoni_shared::seam::Agent`]: auth calls it when a person, an
//! organization or a project goes, and for a person's export.

use std::sync::Arc;

use async_trait::async_trait;
use telmoni_shared::db::tenant_session::{maintenance_scope, person_scope};
use telmoni_shared::seam::Agent;
use telmoni_shared::{OrganizationId, ProjectId, TelmoniError, UserId};

use crate::AppState;
use crate::db::{self, AgentLane};

/// The module as auth holds it.
pub struct AgentSeam(pub Arc<AppState>);

#[async_trait]
impl Agent for AgentSeam {
    async fn erase_person(&self, user_id: &UserId) -> Result<u64, TelmoniError> {
        let mut tx = maintenance_scope(&self.0.db, AgentLane).await?;
        let gone = db::erase_person(&mut tx, user_id).await?;
        tx.commit().await?;
        tracing::info!(user_id = %user_id, gone, "agent erasure: the person's conversations and passages removed");
        Ok(gone)
    }

    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError> {
        let mut tx = maintenance_scope(&self.0.db, AgentLane).await?;
        let gone = db::purge_organization(&mut tx, organization_id).await?;
        tx.commit().await?;
        tracing::info!(organization_id = %organization_id, gone, "agent purge: the organization's rows removed");
        Ok(gone)
    }

    async fn purge_project(&self, project_id: &ProjectId) -> Result<u64, TelmoniError> {
        let mut tx = maintenance_scope(&self.0.db, AgentLane).await?;
        let gone = db::purge_project(&mut tx, project_id).await?;
        tx.commit().await?;
        tracing::info!(project_id = %project_id, gone, "agent purge: the project's rows removed");
        Ok(gone)
    }

    async fn export_person(
        &self,
        user_id: &UserId,
        organization_id: &OrganizationId,
    ) -> Result<serde_json::Value, TelmoniError> {
        let tx = person_scope(&self.0.db, user_id).await?;
        let mut tx = tx.bind_organization(organization_id).await?;
        let conversations = db::export(&mut tx, user_id, organization_id).await?;
        tx.commit().await?;
        Ok(conversations)
    }
}
