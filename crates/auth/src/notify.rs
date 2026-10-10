//! The notices this module raises, handed to notifications in process.

use serde_json::json;

use telmoni_shared::seam::Notice;
use telmoni_shared::{NotificationKind, Role};

use crate::AppState;

/// Tell the PROJECT that `joiner` accepted their invitation. Returns whether
/// notifications took the notice. The organization travels too, because it
/// is what the deletion cascade purges on; and the joiner's id, as the
/// subject, because the notice names them, and their account's erasure has
/// notifications rewrite every notice that does (`redact_person`).
pub async fn emit_member_joined(
    state: &AppState,
    project: &telmoni_shared::ProjectId,
    organization: &telmoni_shared::OrganizationId,
    joiner_id: &telmoni_shared::UserId,
    joiner: &str,
    role: Role,
) -> bool {
    let Some(notifications) = state.siblings.notifications.as_ref() else {
        tracing::info!(project = %project, "member-joined notification skipped (no notifications module)");
        return false;
    };
    let title = format!("{joiner} joined the project");
    let body = format!("{joiner} accepted the invitation and is now {role} on the project.");
    let notice = Notice {
        kind: NotificationKind::MemberAdded,
        subject_user_id: Some(joiner_id),
        title: &title,
        body: &body,
        metadata: json!({ "project_id": project, "role": role }),
        dedup_key: None,
    };
    match notifications
        .emit(organization, Some(project), notice)
        .await
    {
        Ok(_) => {
            tracing::info!(project = %project, "member-joined notification emitted");
            true
        }
        Err(e) => {
            tracing::warn!(project = %project, error = %e, "member-joined notification failed to emit");
            false
        }
    }
}

/// Tell the PROJECT that `leaver` left the project. Returns whether
/// notifications took the notice.
pub async fn emit_member_left(
    state: &AppState,
    project: &telmoni_shared::ProjectId,
    organization: &telmoni_shared::OrganizationId,
    leaver_id: &telmoni_shared::UserId,
    leaver: &str,
) -> bool {
    let Some(notifications) = state.siblings.notifications.as_ref() else {
        tracing::info!(project = %project, "member-left notification skipped (no notifications module)");
        return false;
    };
    let title = format!("{leaver} left the project");
    let body = format!("{leaver} left the project.");
    let notice = Notice {
        kind: NotificationKind::MemberLeft,
        subject_user_id: Some(leaver_id),
        title: &title,
        body: &body,
        metadata: json!({ "project_id": project }),
        dedup_key: None,
    };
    match notifications
        .emit(organization, Some(project), notice)
        .await
    {
        Ok(_) => {
            tracing::info!(project = %project, "member-left notification emitted");
            true
        }
        Err(e) => {
            tracing::warn!(project = %project, error = %e, "member-left notification failed to emit");
            false
        }
    }
}
