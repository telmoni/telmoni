//! What this module does for the modules beside it —
//! [`telmoni_shared::seam::Notifications`], on a handle to the module's
//! state. Auth raises its notices and runs its purges through this; a
//! deployment's own module raises its `organization_alert`s the same way.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam::{
    ActivitySource, Audience, DocumentCursor, Emitted, Notice, Notifications, SourceDocument,
};
use telmoni_shared::{AuthError, OrganizationId, ProjectId, TelmoniError, UserId};

use crate::AppState;
use crate::db::{self, NotificationsLane};
use crate::handler;

/// The module as its siblings hold it: `Arc::new(Notifier(state))` is the
/// `Arc<dyn Notifications>` auth is handed.
pub struct Notifier(pub Arc<AppState>);

#[async_trait]
impl Notifications for Notifier {
    async fn emit(
        &self,
        organization_id: &OrganizationId,
        project_id: Option<&ProjectId>,
        notice: Notice<'_>,
    ) -> Result<Emitted, TelmoniError> {
        handler::emit_notice(&self.0, organization_id, project_id, notice).await
    }

    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError> {
        handler::purge_organization(&self.0, organization_id).await
    }

    async fn purge_project(&self, project_id: &ProjectId) -> Result<u64, TelmoniError> {
        handler::purge_project(&self.0, project_id).await
    }

    async fn redact_person(&self, user_id: &UserId) -> Result<u64, TelmoniError> {
        handler::redact_person(&self.0, user_id).await
    }

    async fn activity_documents(
        &self,
        source: ActivitySource,
        after: Option<&DocumentCursor>,
        limit: i64,
    ) -> Result<Vec<SourceDocument>, TelmoniError> {
        let after = match after {
            Some(cursor) => Some((
                cursor.at,
                Uuid::parse_str(&cursor.id)
                    .map_err(|e| TelmoniError::internal("an activity cursor is not a row id", e))?,
            )),
            None => None,
        };
        let mut tx = maintenance_scope(&self.0.db, NotificationsLane).await?;
        let documents = match source {
            ActivitySource::Feed => db::feed_after(&mut tx, after, limit)
                .await?
                .into_iter()
                .map(feed_document)
                .collect(),
            ActivitySource::Delivery => db::deliveries_after(&mut tx, after, limit)
                .await?
                .into_iter()
                .map(delivery_document)
                .collect(),
        };
        tx.commit().await?;
        Ok(documents)
    }

    async fn connectors(&self, acting: &Acting) -> Result<serde_json::Value, TelmoniError> {
        let project = acting.require_project(Verb::Read, Resource::Connector)?;
        let mut tx = project_scope(&self.0.db, &project.project_id).await?;
        let connections = db::list_connections(&mut tx, &project.project_id).await?;
        tx.commit().await?;
        Ok(json!({ "connections": connections }))
    }

    async fn connector_deliveries(
        &self,
        acting: &Acting,
        connector_id: Uuid,
        limit: i64,
    ) -> Result<serde_json::Value, TelmoniError> {
        let project = acting.require_project(Verb::Read, Resource::Connector)?;
        let mut tx = project_scope(&self.0.db, &project.project_id).await?;
        if !db::connection_exists(&mut tx, &project.project_id, connector_id).await? {
            tx.commit().await?;
            return Err(
                AuthError::NotFound(format!("connection not found: {connector_id}")).into(),
            );
        }
        let deliveries = db::delivery_log(
            &mut tx,
            &project.project_id,
            connector_id,
            None,
            limit.clamp(1, DELIVERIES_TOOL_MAX),
        )
        .await?;
        tx.commit().await?;
        Ok(json!({ "connector_id": connector_id, "deliveries": deliveries }))
    }
}

/// The most deliveries one agent tool call reads.
const DELIVERIES_TOOL_MAX: i64 = 20;

/// A notice as the agent indexes it. A project's is read by everyone on the
/// project; the organization's own feed is its owner's and admins', as the
/// feed lane gives it (`require_organization_admin`).
fn feed_document(item: db::IndexedFeedItem) -> SourceDocument {
    let (audience, url) = match &item.project_id {
        Some(project) => (Audience::Everyone, format!("/{project}")),
        None => (Audience::OrganizationAdmin, "/organization".to_owned()),
    };
    SourceDocument {
        source_id: item.id.to_string(),
        body: format!(
            "Notice ({kind}) at {at}: {title}\n{body}",
            kind = item.kind,
            at = item.created_at.to_rfc3339(),
            title = item.title,
            body = item.body,
        ),
        title: item.title,
        organization_id: item.organization_id,
        project_id: item.project_id,
        subject_user_id: item.subject_user_id,
        audience,
        url,
        created_at: item.created_at,
        cursor: DocumentCursor {
            at: item.created_at,
            id: item.id.to_string(),
        },
    }
}

/// A delivery as the agent indexes it: where it went, how it stands and the
/// last error, so "why didn't Slack get it" finds the attempt.
fn delivery_document(d: db::IndexedDelivery) -> SourceDocument {
    let error = d
        .last_error
        .as_deref()
        .map_or_else(|| "none".to_owned(), ToOwned::to_owned);
    SourceDocument {
        source_id: d.id.to_string(),
        title: format!(
            "Delivery to {} #{}: {}",
            d.provider, d.channel_name, d.status
        ),
        body: format!(
            "The {kind} notice \"{subject}\" to the {provider} connector #{channel} \
             (connector id {connector}, delivery id {id}) is {status} after {attempts} \
             attempt(s). Last error: {error}. Queued at {queued}; last changed at {changed}.",
            kind = d.kind,
            subject = d.subject,
            provider = d.provider,
            channel = d.channel_name,
            connector = d.connection_id,
            id = d.id,
            status = d.status,
            attempts = d.attempts,
            queued = d.created_at.to_rfc3339(),
            changed = d.updated_at.to_rfc3339(),
        ),
        url: format!("/{}/connectors", d.project_id),
        organization_id: d.organization_id,
        project_id: Some(d.project_id),
        subject_user_id: d.subject_user_id,
        audience: Audience::Everyone,
        created_at: d.updated_at,
        cursor: DocumentCursor {
            at: d.updated_at,
            id: d.id.to_string(),
        },
    }
}
