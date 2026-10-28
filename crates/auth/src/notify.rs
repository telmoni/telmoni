//! The notices this module raises, handed to notifications in process.

use serde_json::json;

use telmoni_shared::seam::Notice;
use telmoni_shared::{NotificationKind, Role};

use crate::AppState;

/// How far a title names a person: a display name's own bound, so an address
/// of up to 254 characters still leaves the title under notifications' 200.
const NAMED_MAX_CHARS: usize = 120;

/// The person as a title names them, cut to [`NAMED_MAX_CHARS`].
fn named(who: &str) -> String {
    if who.chars().count() <= NAMED_MAX_CHARS {
        return who.to_owned();
    }
    let cut: String = who.chars().take(NAMED_MAX_CHARS - 1).collect();
    format!("{cut}…")
}

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
    let title = format!("{} joined the project", named(joiner));
    let body = format!("{joiner} accepted the invitation and is now {role} on the project.");
    let notice = Notice {
        kind: NotificationKind::MemberAdded,
        subject_user_id: Some(joiner_id),
        title: &title,
        body: &body,
        metadata: json!({ "project_id": project, "role": role }),
        dedup_key: None,
    };
    match notifications.emit(organization, project, notice).await {
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

/// Tell the PROJECT that `member` was given a seat on it from the
/// organization's roster: the accept's notice, for a seat nobody accepted.
/// It names the member alone, whom `subject_user_id` lets an erasure unname.
pub async fn emit_member_seated(
    state: &AppState,
    project: &telmoni_shared::ProjectId,
    organization: &telmoni_shared::OrganizationId,
    member_id: &telmoni_shared::UserId,
    member: &str,
    role: Role,
) -> bool {
    let Some(notifications) = state.siblings.notifications.as_ref() else {
        tracing::info!(project = %project, "member-seated notification skipped (no notifications module)");
        return false;
    };
    let title = format!("{} was added to the project", named(member));
    let body =
        format!("{member} is now {role} on the project, added from the organization's members.");
    let notice = Notice {
        kind: NotificationKind::MemberAdded,
        subject_user_id: Some(member_id),
        title: &title,
        body: &body,
        metadata: json!({ "project_id": project, "role": role, "given": true }),
        dedup_key: None,
    };
    match notifications.emit(organization, project, notice).await {
        Ok(_) => {
            tracing::info!(project = %project, "member-seated notification emitted");
            true
        }
        Err(e) => {
            tracing::warn!(project = %project, error = %e, "member-seated notification failed to emit");
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
    let title = format!("{} left the project", named(leaver));
    let body = format!("{leaver} left the project.");
    let notice = Notice {
        kind: NotificationKind::MemberLeft,
        subject_user_id: Some(leaver_id),
        title: &title,
        body: &body,
        metadata: json!({ "project_id": project }),
        dedup_key: None,
    };
    match notifications.emit(organization, project, notice).await {
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

#[cfg(test)]
mod tests {
    use super::{NAMED_MAX_CHARS, named};

    /// The longest address a person can hold still leaves every title these
    /// notices write under the 200 characters notifications takes.
    #[test]
    fn a_title_names_the_longest_address_within_its_bound() {
        let address = format!("{}@example.test", "a".repeat(241));
        assert_eq!(address.chars().count(), 254);
        let name = named(&address);
        assert_eq!(name.chars().count(), NAMED_MAX_CHARS);
        assert!(name.ends_with('…'));
        for title in [
            format!("{name} joined the project"),
            format!("{name} was added to the project"),
            format!("{name} left the project"),
        ] {
            assert!(title.chars().count() <= 200, "{title}");
        }
        assert_eq!(named("Ada Lovelace"), "Ada Lovelace");
    }
}
