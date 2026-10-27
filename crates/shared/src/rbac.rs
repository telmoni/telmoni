//! RBAC enforcement — [`can`], the one permission matrix.
//!
//! **Auth decides the role; a service enforces it.** A request names the
//! organization it acts on, and the service asks this function before it binds
//! that scope. `crates/auth/tests/rbac_matrix.rs` pins every cell, so a flipped
//! verdict here fails a named test there.

use crate::error::{AuthzError, TelmoniError};
use crate::types::{OrganizationRole, Role};

/// What the caller is trying to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// Read a single resource or an organization-scoped list.
    Read,
    /// Create a new resource on the organization.
    Create,
    /// Mutate an existing resource (rename, rotate, change-state).
    Update,
    /// Permanently remove a resource.
    Delete,
}

// The words a refusal names them by, in the matrix's order.
crate::wire_enum!(Verb {
    Read => "read",
    Create => "create",
    Update => "update",
    Delete => "delete",
});

/// What the caller is trying to do it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    /// A project: its settings and lifecycle.
    Project,
    /// The organization's member list: every role reads it, owner and admin write.
    Member,
    /// `tm_` API tokens — the keys a project's admins mint, to read the
    /// public API and to send spans.
    Token,
    /// A project's Slack or webhook connection.
    Connector,
    /// The organization's append-only audit chain. Owners and admins read it,
    /// members do not; the resource exists to deny the write verbs structurally.
    Audit,
    /// A project's traces: the runs its agents recorded, which every role on
    /// the project reads. Ingest alone writes a span, and only the nightly
    /// purge or the project's own deletion removes one, so the write verbs
    /// are denied structurally.
    Trace,
    /// A project's saved views of its trace list: a filter, a sort and the
    /// columns shown, under a name. Every role on the project applies them;
    /// its owners and admins save them, as they change everything else the
    /// project's members share.
    View,
    /// A project's alerts: a metric over its spans against thresholds. Any
    /// member of the project makes, edits, pauses and deletes them, as
    /// Langfuse's members make alerts: an alert reads what every member may
    /// read already, and tells the connectors the project's admins set up.
    Alert,
    // The product built on this platform adds its resources here WITH their
    // handlers, never before, or the matrix grants a capability no endpoint
    // exposes.
}

// The words a refusal names them by, in the matrix's order.
crate::wire_enum!(Resource {
    Project => "project",
    Member => "member",
    Token => "token",
    Connector => "connector",
    Audit => "audit",
    Trace => "trace",
    View => "view",
    Alert => "alert",
});

/// The verdict: may `role` perform `verb` on `resource`?
#[must_use]
pub fn can(role: Role, verb: Verb, resource: Resource) -> bool {
    use Resource as R;
    use Verb as V;

    // Combinations no customer endpoint exposes — deny outright. The chain is
    // written by `emit_audit` and hash-linked. A role that could edit it would
    // make it a record of what somebody was willing to leave behind. A trace
    // is written by ingest and removed by the purge: a record, not a thing a
    // person edits.
    if matches!(
        (verb, resource),
        (V::Create | V::Update | V::Delete, R::Audit | R::Trace)
    ) {
        return false;
    }

    match role {
        Role::Owner => true,

        // Project admins can do everything on the project level except delete
        // the project (and structurally impossible audit mutations).
        Role::Admin => !(resource == R::Project && verb == V::Delete),

        // Reads the work, mutates nothing but the project's alerts. The chain
        // is the exception: what everyone did is the owner's record of the
        // collaboration, not the work a member was invited to see.
        Role::Member => resource == R::Alert || (resource != R::Audit && verb == V::Read),
    }
}

/// The least privileged role [`can`] admits for `(verb, resource)`, or `None`
/// when no role may.
///
/// ⚠️ **A refusal has to name the thing that would actually lift it.** Denials
/// once always named Owner, so a member refused the audit log was told to
/// become the owner — a role no project grants; only an ownership transfer
/// makes somebody the owner. Derived from [`can`] by search, so the two cannot
/// disagree.
#[must_use]
pub fn minimum_role(verb: Verb, resource: Resource) -> Option<Role> {
    Role::all()
        .into_iter()
        .rev()
        .find(|&role| can(role, verb, resource))
}

/// [`can`]'s verdict as the refusal every lane answers: the least role that
/// would do, or that no role may.
pub fn authorize(role: Role, verb: Verb, resource: Resource) -> Result<(), TelmoniError> {
    if can(role, verb, resource) {
        return Ok(());
    }
    Err(match minimum_role(verb, resource) {
        Some(required) => AuthzError::InsufficientRole {
            required,
            actual: role,
        },
        None => AuthzError::Forbidden(format!("no role may {verb} {resource}")),
    }
    .into())
}

/// A person's role on one project, from their organization role and their
/// seat on the project, if any; `None` refuses them the project.
///
/// The organization role is a floor a seat cannot lower: the owner owns every
/// project, and an admin is an admin on every one, whatever seat they also
/// hold. Below that, the seat alone decides. Nothing flows the other way: no
/// seat grants anything on the organization.
#[must_use]
pub fn project_role(organization: Option<OrganizationRole>, seat: Option<Role>) -> Option<Role> {
    match organization {
        Some(OrganizationRole::Owner) => Some(Role::Owner),
        Some(OrganizationRole::Admin) => Some(Role::Admin),
        Some(OrganizationRole::Member) | None => seat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERBS: [Verb; 4] = Verb::all();
    const RESOURCES: [Resource; 8] = Resource::all();

    /// A refusal reads as a sentence: lower case, as `Display` gives it.
    #[test]
    fn display_is_the_lower_case_word() {
        assert_eq!(Verb::Create.to_string(), "create");
        assert_eq!(Resource::Connector.to_string(), "connector");
    }

    #[test]
    fn deny_by_default_on_meaningless_combos() {
        assert!(!can(Role::Owner, Verb::Create, Resource::Audit));
        assert!(!can(Role::Owner, Verb::Delete, Resource::Audit));
        assert!(!can(Role::Owner, Verb::Create, Resource::Trace));
        assert!(!can(Role::Owner, Verb::Update, Resource::Trace));
        assert!(!can(Role::Owner, Verb::Delete, Resource::Trace));
    }

    #[test]
    fn member_is_read_only_but_for_the_alerts() {
        assert!(can(Role::Member, Verb::Read, Resource::Member));
        assert!(can(Role::Member, Verb::Read, Resource::Project));
        assert!(can(Role::Member, Verb::Read, Resource::Trace));
        assert!(!can(Role::Member, Verb::Read, Resource::Audit));
        for resource in RESOURCES {
            for verb in [Verb::Create, Verb::Update, Verb::Delete] {
                assert_eq!(
                    can(Role::Member, verb, resource),
                    resource == Resource::Alert,
                    "Member {verb:?} {resource:?}"
                );
            }
        }
        assert_eq!(
            minimum_role(Verb::Delete, Resource::Alert),
            Some(Role::Member)
        );
    }

    /// What separates an Admin from the Owner: the Admin can do everything
    /// on the project except delete it.
    #[test]
    fn an_admin_can_do_everything_except_delete_project() {
        assert!(can(Role::Admin, Verb::Read, Resource::Project));
        assert!(can(Role::Admin, Verb::Create, Resource::Project));
        assert!(can(Role::Admin, Verb::Update, Resource::Project));
        assert!(!can(Role::Admin, Verb::Delete, Resource::Project));

        assert!(can(Role::Admin, Verb::Read, Resource::Member));
        assert!(can(Role::Admin, Verb::Create, Resource::Member));
        assert!(can(Role::Admin, Verb::Update, Resource::Member));
        assert!(can(Role::Admin, Verb::Delete, Resource::Member));

        assert!(can(Role::Admin, Verb::Read, Resource::Token));
        assert!(can(Role::Admin, Verb::Create, Resource::Token));
        assert!(can(Role::Admin, Verb::Update, Resource::Token));
        assert!(can(Role::Admin, Verb::Delete, Resource::Token));

        assert!(can(Role::Admin, Verb::Read, Resource::Connector));
        assert!(can(Role::Admin, Verb::Create, Resource::Connector));
        assert!(can(Role::Admin, Verb::Update, Resource::Connector));
        assert!(can(Role::Admin, Verb::Delete, Resource::Connector));

        assert!(can(Role::Admin, Verb::Read, Resource::Audit));
        assert!(!can(Role::Admin, Verb::Create, Resource::Audit));
        assert!(!can(Role::Admin, Verb::Update, Resource::Audit));
        assert!(!can(Role::Admin, Verb::Delete, Resource::Audit));

        assert!(can(Role::Admin, Verb::Read, Resource::Trace));
        assert!(!can(Role::Admin, Verb::Create, Resource::Trace));
        assert!(!can(Role::Admin, Verb::Update, Resource::Trace));
        assert!(!can(Role::Admin, Verb::Delete, Resource::Trace));

        assert!(can(Role::Admin, Verb::Read, Resource::View));
        assert!(can(Role::Admin, Verb::Create, Resource::View));
        assert!(can(Role::Admin, Verb::Update, Resource::View));
        assert!(can(Role::Admin, Verb::Delete, Resource::View));
    }

    /// The answer a refusal gives has to be reachable and has to be true.
    #[test]
    fn the_minimum_role_is_the_least_one_that_may() {
        for verb in VERBS {
            for resource in RESOURCES {
                let minimum = minimum_role(verb, resource);
                match minimum {
                    Some(role) => {
                        assert!(
                            can(role, verb, resource),
                            "{role} is named for {verb:?} {resource:?} and may not do it"
                        );
                        let below = Role::all()
                            .into_iter()
                            .skip_while(|&r| r != role)
                            .skip(1)
                            .collect::<Vec<_>>();
                        for lesser in below {
                            assert!(
                                !can(lesser, verb, resource),
                                "{lesser} may {verb:?} {resource:?} below the named {role}"
                            );
                        }
                    }
                    None => {
                        for role in Role::all() {
                            assert!(
                                !can(role, verb, resource),
                                "{verb:?} {resource:?} names no role but {role} may do it"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The cell the literal got wrong, called out by name: an Admin reads the
    /// chain, so a Member refused it must be told Admin, not Owner.
    #[test]
    fn reading_the_audit_log_asks_for_an_admin_not_the_owner() {
        assert_eq!(minimum_role(Verb::Read, Resource::Audit), Some(Role::Admin));
        assert_eq!(
            minimum_role(Verb::Create, Resource::Token),
            Some(Role::Admin)
        );
        assert_eq!(
            minimum_role(Verb::Delete, Resource::Member),
            Some(Role::Admin)
        );
        assert_eq!(
            minimum_role(Verb::Delete, Resource::Project),
            Some(Role::Owner)
        );
        assert_eq!(minimum_role(Verb::Delete, Resource::Audit), None);
        assert_eq!(
            minimum_role(Verb::Read, Resource::Trace),
            Some(Role::Member)
        );
        assert_eq!(minimum_role(Verb::Delete, Resource::Trace), None);
        assert_eq!(minimum_role(Verb::Read, Resource::View), Some(Role::Member));
        assert_eq!(
            minimum_role(Verb::Create, Resource::View),
            Some(Role::Admin)
        );
    }

    #[test]
    fn an_organization_role_is_a_floor_a_seat_cannot_lower() {
        let seats = [None, Some(Role::Member), Some(Role::Admin)];
        for seat in seats {
            assert_eq!(
                project_role(Some(OrganizationRole::Owner), seat),
                Some(Role::Owner)
            );
            assert_eq!(
                project_role(Some(OrganizationRole::Admin), seat),
                Some(Role::Admin)
            );
            assert_eq!(project_role(Some(OrganizationRole::Member), seat), seat);
            assert_eq!(project_role(None, seat), seat);
        }
    }

    /// Member ⊂ Admin ⊂ Owner, over every cell. The BFF treats an unknown role
    /// as Member, which is only safe while this holds.
    #[test]
    fn the_roles_nest() {
        for verb in VERBS {
            for resource in RESOURCES {
                if can(Role::Member, verb, resource) {
                    assert!(
                        can(Role::Admin, verb, resource),
                        "Member may {verb:?} {resource:?} but Admin may not"
                    );
                }
                if can(Role::Admin, verb, resource) {
                    assert!(
                        can(Role::Owner, verb, resource),
                        "Admin may {verb:?} {resource:?} but Owner may not"
                    );
                }
            }
        }
    }
}
