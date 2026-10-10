//! The notices this service raises about its own connectors.

use serde_json::json;
use telmoni_shared::db::tenant_session::{Binding, Maintenance, ProjectAndOrganization, Scoped};
use telmoni_shared::{NotificationKind, OrganizationId, ProjectId};
use uuid::Uuid;

use crate::connector::Provider;
use crate::db::{self, NotificationsLane};

/// Write one project-level notice and queue it to the project's active
/// connections. Returns the delivery ids for the inline first attempt.
pub async fn emit_project_notice<B: Binding>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    kind: NotificationKind,
    title: &str,
    body: &str,
    metadata: &serde_json::Value,
) -> sqlx::Result<Vec<Uuid>> {
    db::insert_feed(
        &mut *tx,
        Some(project_id),
        organization_id,
        &db::NewFeedItem {
            subject_user_id: None,
            kind,
            title,
            body,
            metadata,
            dedup_key: None,
        },
    )
    .await?;
    fan_out(tx, project_id, organization_id, kind, title, body).await
}

/// One delivery row per active connection on the project, in one `INSERT`.
pub async fn fan_out<B: Binding>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    kind: NotificationKind,
    title: &str,
    body: &str,
) -> sqlx::Result<Vec<Uuid>> {
    db::enqueue_deliveries(
        tx,
        organization_id,
        Some(project_id),
        kind,
        title,
        body,
        None,
    )
    .await
}

/// Where a connection points, in a customer's words: `#general in Acme`, a
/// Discord webhook's name, or the host of a signed webhook — the one thing
/// about its URL the platform will say.
#[must_use]
pub fn describe_target(
    provider: Provider,
    workspace_name: Option<&str>,
    channel_name: &str,
) -> String {
    match (provider, workspace_name) {
        (Provider::Slack, Some(workspace)) => {
            format!("#{} in {workspace}", channel_name.trim_start_matches('#'))
        }
        (Provider::Slack, None) => format!("#{}", channel_name.trim_start_matches('#')),
        (Provider::Discord, _) => channel_name.to_owned(),
        (Provider::Webhook, _) => format!("the endpoint at {channel_name}"),
    }
}

/// A channel was connected. Raised at the end of the handshake, so the
/// channel's first message is the news that it is connected.
pub async fn connector_connected(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    provider: Provider,
    workspace_name: Option<&str>,
    channel_name: &str,
) -> sqlx::Result<Vec<Uuid>> {
    let target = describe_target(provider, workspace_name, channel_name);
    emit_project_notice(
        tx,
        project_id,
        organization_id,
        NotificationKind::ConnectorConnected,
        &format!("{} connected", provider.label()),
        &format!("This project's notices will now be posted to {target}."),
        &json!({ "provider": provider.as_str(), "channel": channel_name }),
    )
    .await
}

/// A connection stopped. `reason` is the vendor's own answer, bounded.
pub async fn connector_disconnected(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    retired: &db::RetiredConnection,
    reason: &str,
) -> sqlx::Result<Vec<Uuid>> {
    let provider = retired.provider;
    let target = describe_target(
        provider,
        retired.external_workspace_name.as_deref(),
        &retired.channel_name,
    );
    emit_project_notice(
        tx,
        &ProjectId::from_trusted(retired.project_id.as_str()),
        &OrganizationId::from_trusted(retired.organization_id.as_str()),
        NotificationKind::ConnectorDisconnected,
        &format!("{} disconnected", provider.label()),
        &format!(
            "Notices are no longer reaching {target}. {}. Bring it back from the Connectors page.",
            telmoni_shared::text::truncate_on_char_boundary(reason, 200)
        ),
        &json!({
            "provider": provider.as_str(),
            "channel": retired.channel_name,
            "connection_id": retired.id,
        }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_is_described_in_the_vendors_own_terms() {
        assert_eq!(
            describe_target(Provider::Slack, Some("Acme"), "general"),
            "#general in Acme"
        );
        assert_eq!(
            describe_target(Provider::Slack, Some("Acme"), "#general"),
            "#general in Acme"
        );
        assert_eq!(
            describe_target(Provider::Slack, None, "general"),
            "#general"
        );
        assert_eq!(
            describe_target(Provider::Discord, None, "Telmoni alerts"),
            "Telmoni alerts"
        );
        assert_eq!(
            describe_target(Provider::Webhook, None, "hooks.example.com:8443"),
            "the endpoint at hooks.example.com:8443"
        );
    }
}
