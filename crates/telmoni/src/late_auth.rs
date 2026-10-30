//! The [`Auth`] seam before auth exists: one call forwarded per method.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use axum::http::HeaderMap;

use telmoni_shared::acting::{Acting, ActingKey};
use telmoni_shared::seam::{
    AuditEventsQuery, Auth, DocumentCursor, OwnedOrganization, ProjectHome, ReadableProject,
    SourceDocument,
};
use telmoni_shared::{
    AuthError, FlagSet, OrganizationId, OrganizationStatus, ProjectId, TelmoniError, UserId,
};

/// Auth as notifications, telemetry and the agent hold it before auth is
/// built. Auth holds each of them, so they are handed a slot that is filled a
/// moment later; a call before that answers as an auth that is not there.
#[derive(Default)]
pub(crate) struct LateAuth(OnceLock<Arc<telmoni_auth::AppState>>);

impl LateAuth {
    /// Fill the slot with auth, once built.
    pub(crate) fn fill(&self, auth: Arc<telmoni_auth::AppState>) -> anyhow::Result<()> {
        if self.0.set(auth).is_err() {
            anyhow::bail!("auth was wired twice");
        }
        Ok(())
    }

    fn get(&self) -> Result<&Arc<telmoni_auth::AppState>, TelmoniError> {
        self.0
            .get()
            .ok_or_else(|| AuthError::IdentityUnavailable.into())
    }
}

#[async_trait]
impl Auth for LateAuth {
    async fn resolve(&self, headers: &HeaderMap) -> Result<Acting, TelmoniError> {
        self.get()?.resolve(headers).await
    }

    async fn resolve_again(&self, acting: &Acting) -> Result<Acting, TelmoniError> {
        self.get()?.resolve_again(acting).await
    }

    async fn resolve_api_key(&self, headers: &HeaderMap) -> Result<ActingKey, TelmoniError> {
        self.get()?.resolve_api_key(headers).await
    }

    async fn project_homes(
        &self,
        projects: &[ProjectId],
    ) -> Result<Vec<ProjectHome>, TelmoniError> {
        self.get()?.project_homes(projects).await
    }

    async fn organization_slugs(
        &self,
        organizations: &[OrganizationId],
    ) -> Result<Vec<(OrganizationId, telmoni_shared::slug::Slug)>, TelmoniError> {
        self.get()?.organization_slugs(organizations).await
    }

    async fn readable_projects(
        &self,
        acting: &Acting,
    ) -> Result<Vec<ReadableProject>, TelmoniError> {
        self.get()?.readable_projects(acting).await
    }

    async fn global_flags(&self) -> Result<FlagSet, TelmoniError> {
        self.get()?.global_flags().await
    }

    async fn organization_standing(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<OrganizationStatus>, TelmoniError> {
        self.get()?.organization_standing(organization_id).await
    }

    async fn organizations_owned_by(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<OwnedOrganization>, TelmoniError> {
        self.get()?.organizations_owned_by(user_id).await
    }

    async fn owner_of(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<UserId>, TelmoniError> {
        self.get()?.owner_of(organization_id).await
    }

    async fn owners_of_several(&self) -> Result<Vec<UserId>, TelmoniError> {
        self.get()?.owners_of_several().await
    }

    async fn audit_documents(
        &self,
        after: Option<&DocumentCursor>,
        limit: i64,
    ) -> Result<Vec<SourceDocument>, TelmoniError> {
        self.get()?.audit_documents(after, limit).await
    }

    async fn members(&self, acting: &Acting) -> Result<serde_json::Value, TelmoniError> {
        self.get()?.members(acting).await
    }

    async fn audit_events(
        &self,
        acting: &Acting,
        query: &AuditEventsQuery,
    ) -> Result<serde_json::Value, TelmoniError> {
        self.get()?.audit_events(acting, query).await
    }
}
