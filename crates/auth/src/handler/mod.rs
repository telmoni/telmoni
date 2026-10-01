pub mod account;
pub mod audit;
pub mod deletion;
pub mod export;
pub mod invite;
pub mod me;
pub mod member;
pub mod organization;
pub mod organization_members;
pub mod ownership;
pub mod password;
pub mod project_transfer;
pub mod projects;
pub mod session;
pub mod sessions;
pub mod tokens;
pub mod v1;

use telmoni_shared::db::tenant_session::{
    Binding, Organization, Person, Project, ProjectAndOrganization, Scoped,
};

use crate::db::organization_members::RosterRead;

/// Validate a project id arriving as a plain string. A malformed id is the
/// caller's 400, never a row keyed on garbage.
pub(crate) fn parse_project_id(
    s: &str,
) -> Result<telmoni_shared::ProjectId, telmoni_shared::TelmoniError> {
    telmoni_shared::ProjectId::try_new(s).map_err(|e| {
        telmoni_shared::AuthError::BadRequest(format!("invalid project id: {e}")).into()
    })
}

/// Validate an organization id at the HTTP boundary.
pub(crate) fn parse_organization_id(
    s: &str,
) -> Result<telmoni_shared::OrganizationId, telmoni_shared::TelmoniError> {
    telmoni_shared::OrganizationId::try_new(s).map_err(|e| {
        telmoni_shared::AuthError::BadRequest(format!("invalid organization id: {e}")).into()
    })
}

/// Validate a user id at the HTTP boundary. It becomes `added_by`,
/// `created_by` and the audit `actor_id`, so an unbounded or control-character
/// value once reached those columns and every log line naming the actor.
pub(crate) fn parse_user_id(
    s: &str,
) -> Result<telmoni_shared::UserId, telmoni_shared::TelmoniError> {
    telmoni_shared::UserId::try_new(s)
        .map_err(|e| telmoni_shared::AuthError::BadRequest(format!("invalid user id: {e}")).into())
}

/// The organization a request ACTS ON, from the console's `x-organization-id`
/// header. A claim about WHAT, never WHO: the person comes from the verified
/// bearer (`person_token::Principal`), and every lane checks their membership
/// of this organization before touching a row.
pub(crate) fn organization_of(
    headers: &axum::http::HeaderMap,
) -> Result<telmoni_shared::OrganizationId, telmoni_shared::TelmoniError> {
    let raw = headers
        .get("x-organization-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            telmoni_shared::AuthError::BadRequest("missing x-organization-id header".to_string())
        })?;
    parse_organization_id(raw)
}

/// An open transaction bound to the ORGANIZATION being acted on, with the
/// caller's organization role resolved inside it.
pub(crate) struct ActingOrganization<'a> {
    pub tx: Scoped<'a, Organization>,
    /// The caller's row in the organization's roster — `Owner` for exactly one
    /// person.
    pub role: telmoni_shared::OrganizationRole,
}

/// Open an [`ActingOrganization`] transaction: bind the organization being
/// acted on, then resolve the caller's role on it from its roster. The owner
/// is a row like everyone else.
pub(crate) async fn acting_organization<'a>(
    state: &'a crate::AppState,
    organization_id: &telmoni_shared::OrganizationId,
    user_id: &telmoni_shared::UserId,
) -> Result<ActingOrganization<'a>, telmoni_shared::TelmoniError> {
    use telmoni_shared::db::tenant_session::organization_scope;

    let mut tx = organization_scope(&state.db, organization_id).await?;
    let role = organization_role_or_forbidden(&mut tx, organization_id, user_id).await?;
    Ok(ActingOrganization { tx, role })
}

/// The one sentence every lane answers for an organization whose deletion has
/// been requested.
pub(crate) const BEING_DELETED: &str = "this organization is being deleted";

/// The caller's role on an organization, or a 403, on a transaction somebody
/// else opened. ⚠ The ONE spelling of this refusal: it stands between a forged
/// `x-organization-id` and another tenant's rows, and eight hand-written copies
/// were seven chances for one to drift.
///
/// ⚠ **And the one place an organization being deleted is refused.** From the
/// `pending_deletion` mark until its row goes, nothing acts in it: its keys
/// are revoked and `/me` stops listing it, but a request already holding an
/// organization id — a stale tab, a connector handshake that started before
/// the mark — would otherwise write rows the purge has already run past. The
/// membership is read first, so a stranger learns nothing about its state.
pub(crate) async fn organization_role_or_forbidden<B: RosterRead>(
    tx: &mut Scoped<'_, B>,
    organization_id: &telmoni_shared::OrganizationId,
    user_id: &telmoni_shared::UserId,
) -> Result<telmoni_shared::OrganizationRole, telmoni_shared::TelmoniError> {
    let role = crate::db::organization_members::role_on_organization(tx, organization_id, user_id)
        .await?
        .ok_or_else(|| -> telmoni_shared::TelmoniError {
            telmoni_shared::AuthzError::Forbidden(
                "you are not a member of this organization".to_string(),
            )
            .into()
        })?;
    refuse_unless_active(tx, organization_id).await?;
    Ok(role)
}

/// A 403 naming [`BEING_DELETED`] unless the organization is `active`. Reads
/// the row under whatever `tx` has bound: the organization's own, the
/// caller's person scope (`member_read`, for an organization they are in) or a
/// project's (`project_read`) — and an organization none of them may see reads
/// as not active, so the answer is a refusal, never a leak.
pub(crate) async fn refuse_unless_active<B: Binding>(
    tx: &mut Scoped<'_, B>,
    organization_id: &telmoni_shared::OrganizationId,
) -> Result<(), telmoni_shared::TelmoniError> {
    match crate::db::organizations::status(tx, organization_id).await? {
        Some(telmoni_shared::OrganizationStatus::Active) => Ok(()),
        _ => Err(telmoni_shared::AuthzError::Forbidden(BEING_DELETED.to_string()).into()),
    }
}

/// The organization a person's own act is recorded on, from
/// `x-organization-id` — or none, for somebody in no organization at all
/// (sign-ups closed after they left their last one), whose sessions, address
/// and consent are still theirs to manage. [`acting_person_in`] decides
/// whether an absent header is allowed; a malformed one is still a 400.
pub(crate) fn recording_organization_of(
    headers: &axum::http::HeaderMap,
) -> Result<Option<telmoni_shared::OrganizationId>, telmoni_shared::TelmoniError> {
    match headers
        .get("x-organization-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
    {
        Some(raw) => parse_organization_id(raw).map(Some),
        None => Ok(None),
    }
}

/// A transaction for something a person does to THEMSELVES — their sessions,
/// their address, their analytics consent — bound to the person, once they
/// are shown to be a member of the organization the act will be recorded on,
/// or to be in no organization at all.
///
/// ⚠ **The membership check is the whole point.** These acts are audited on
/// the chain of the organization the console names (`x-organization-id`), and
/// `emit_audit` binds whatever organization it is handed. Without this check
/// anybody could write rows onto any organization's chain by naming it.
///
/// ⚠ **No header is allowed only to somebody in no organization.** They have
/// no chain to write on, so the act goes to the log alone
/// ([`log_act_outside_every_organization`]). Anybody in one must name it, or
/// dropping the header would be a way to act unrecorded.
///
/// ⚠ **And the person's lock, which account deletion holds throughout.** These
/// lanes write the person's own rows and then an organization's audit chain;
/// deletion writes the chains of what they own and then their rows. Unordered,
/// the two deadlock — and an email change that lost would fail after the
/// identity provider had already moved the address.
pub(crate) async fn acting_person_in<'a>(
    state: &'a crate::AppState,
    user_id: &telmoni_shared::UserId,
    organization_id: Option<&telmoni_shared::OrganizationId>,
) -> Result<Scoped<'a, Person>, telmoni_shared::TelmoniError> {
    let mut tx = telmoni_shared::db::tenant_session::person_scope(&state.db, user_id).await?;
    match organization_id {
        Some(organization_id) => {
            organization_role_or_forbidden(&mut tx, organization_id, user_id).await?;
            crate::db::locks::lock_person(&mut tx, user_id).await?;
        }
        None => {
            crate::db::locks::lock_person(&mut tx, user_id).await?;
            if crate::db::organization_members::belongs_anywhere(&mut tx, user_id).await? {
                return Err(telmoni_shared::AuthError::BadRequest(
                    "missing x-organization-id header: name the organization this is \
                     recorded on"
                        .to_string(),
                )
                .into());
            }
        }
    }
    Ok(tx)
}

/// The record of a person's act on themselves when [`acting_person_in`] let it
/// through with no organization: no organization's audit log is theirs to
/// write, so the structured log is the only record.
pub(crate) fn log_act_outside_every_organization(
    user_id: &telmoni_shared::UserId,
    act: &str,
    metadata: &serde_json::Value,
) {
    tracing::info!(
        user_id = %user_id,
        act,
        metadata = %metadata,
        "person act by somebody in no organization; recorded in the log alone"
    );
}

/// Gate an [`ActingProject`] transaction against the RBAC matrix, or fail with
/// the role held and the least one that would suffice — read OUT OF the matrix,
/// read directly from the matrix.
pub(crate) fn authorize(
    role: telmoni_shared::Role,
    verb: telmoni_shared::rbac::Verb,
    resource: telmoni_shared::rbac::Resource,
) -> Result<(), telmoni_shared::TelmoniError> {
    if telmoni_shared::rbac::can(role, verb, resource) {
        return Ok(());
    }
    let Some(required) = telmoni_shared::rbac::minimum_role(verb, resource) else {
        return Err(telmoni_shared::AuthzError::Forbidden(format!(
            "no role may {} {}",
            format!("{verb:?}").to_lowercase(),
            format!("{resource:?}").to_lowercase(),
        ))
        .into());
    };
    Err(telmoni_shared::AuthzError::InsufficientRole {
        required,
        actual: role,
    }
    .into())
}

/// The project a request acts on, from `x-project-id` and nothing else.
pub(crate) fn project_of(
    headers: &axum::http::HeaderMap,
) -> Result<telmoni_shared::types::ProjectId, telmoni_shared::TelmoniError> {
    let raw = headers
        .get("x-project-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            telmoni_shared::AuthError::BadRequest("missing x-project-id header".to_string())
        })?;
    telmoni_shared::types::ProjectId::try_new(raw).map_err(|e| {
        telmoni_shared::error::AuthError::BadRequest(format!("invalid x-project-id: {e}")).into()
    })
}

pub(crate) struct ActingProject<'a, B: Binding = Project> {
    pub tx: Scoped<'a, B>,
    pub role: telmoni_shared::Role,
    /// The organization that owns this project, for a handler that must write
    /// `auth.projects` itself without a second query.
    pub organization: telmoni_shared::OrganizationId,
    /// The caller's row in that organization's roster, when they hold one —
    /// what a sibling module's `resolve` is answered as the organization role.
    pub organization_role: Option<telmoni_shared::OrganizationRole>,
}

impl<'a> ActingProject<'a, Project> {
    /// Bind the OWNING organization for the statements that follow.
    pub(crate) async fn enter_owner_scope(
        self,
    ) -> Result<ActingProject<'a, ProjectAndOrganization>, telmoni_shared::TelmoniError> {
        let tx = self.tx.bind_organization(&self.organization).await?;
        Ok(ActingProject {
            tx,
            role: self.role,
            organization: self.organization,
            organization_role: self.organization_role,
        })
    }
}

impl<'a> ActingProject<'a, ProjectAndOrganization> {
    /// Back to the project-only scope, the way `acting_project` handed it over.
    /// Always paired with `enter_owner_scope`: an organization id left bound
    /// would widen every later read from one project to all of them.
    pub(crate) async fn leave_owner_scope(
        self,
    ) -> Result<ActingProject<'a, Project>, telmoni_shared::TelmoniError> {
        let tx = self.tx.clear_organization().await?;
        Ok(ActingProject {
            tx,
            role: self.role,
            organization: self.organization,
            organization_role: self.organization_role,
        })
    }
}

/// Open a transaction bound to the project being acted on, with the caller's
/// role on it, as [`telmoni_shared::rbac::project_role`] decides it.
pub(crate) async fn acting_project<'a>(
    state: &'a crate::AppState,
    project_id: &telmoni_shared::types::ProjectId,
    user_id: &telmoni_shared::UserId,
) -> Result<ActingProject<'a>, telmoni_shared::TelmoniError> {
    use telmoni_shared::OrganizationStatus;
    use telmoni_shared::db::tenant_session::project_and_person_scope;

    // ⚠ **THE PERSON IS BOUND FOR ONE READ.** Bound to the CALLER
    // (`app.user_id`), `member_read` admits only their own roster row — which
    // is also how the owner is recognised — rather than opening the
    // organization's roster. Cleared again before the transaction is handed
    // back, so the person does not ride along on project work; the project
    // binding stays. Both bindings and the read are one statement each: this
    // runs behind every `resolve` a sibling module makes.
    let mut tx = project_and_person_scope(&state.db, project_id, user_id).await?;
    let context = crate::db::projects::acting_context(&mut tx, project_id, user_id).await?;
    let tx = tx.clear_person().await?;

    let Some(context) = context else {
        tx.rollback().await?;
        return Err(telmoni_shared::AuthError::BadRequest("project not found".into()).into());
    };

    let Some(role) = telmoni_shared::rbac::project_role(context.organization_role, context.seat)
    else {
        tx.commit().await?;
        return Err(telmoni_shared::AuthzError::Forbidden(
            "you are not a member of this project".to_string(),
        )
        .into());
    };
    // The project's organization, once the caller is known to be in it: a
    // project in an organization being deleted is as closed as the
    // organization (see `organization_role_or_forbidden`).
    if context.status != OrganizationStatus::Active {
        return Err(telmoni_shared::AuthzError::Forbidden(BEING_DELETED.to_string()).into());
    }
    Ok(ActingProject {
        tx,
        role,
        organization: context.organization_id,
        organization_role: context.organization_role,
    })
}

/// The organization a project belongs to — the audit chain's root for anything
/// done inside that project. A project the scope cannot see is an error, as a
/// row that is not there.
pub(crate) async fn get_project_organization<B: Binding>(
    tx: &mut Scoped<'_, B>,
    project_id: &telmoni_shared::types::ProjectId,
) -> Result<telmoni_shared::OrganizationId, telmoni_shared::TelmoniError> {
    Ok(crate::db::projects::organization_of(tx, project_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?)
}
