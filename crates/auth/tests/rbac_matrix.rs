//! Telmoni RBAC matrix.
#![expect(
    clippy::indexing_slicing,
    reason = "a panicking helper is a failing test"
)]
#![expect(clippy::panic, reason = "test scaffolding: asserts and fixture setup")]

use telmoni_shared::Role;
use telmoni_shared::rbac::{Resource, Verb, can};

/// A capability cell: may this role perform this verb on this resource?
#[derive(Debug, Clone, Copy)]
struct Cell {
    role: Role,
    verb: Verb,
    resource: Resource,
    allowed: bool,
}

/// The closed RBAC catalogue. Every row is intentional; a missing row is a deny.
const MATRIX: &[Cell] = &[
    cell(Role::Owner, Verb::Read, Resource::Project, true),
    cell(Role::Owner, Verb::Create, Resource::Project, true),
    cell(Role::Owner, Verb::Update, Resource::Project, true),
    cell(Role::Owner, Verb::Delete, Resource::Project, true),
    cell(Role::Owner, Verb::Read, Resource::Member, true),
    cell(Role::Owner, Verb::Create, Resource::Member, true),
    cell(Role::Owner, Verb::Update, Resource::Member, true),
    cell(Role::Owner, Verb::Delete, Resource::Member, true),
    cell(Role::Owner, Verb::Read, Resource::Token, true),
    cell(Role::Owner, Verb::Create, Resource::Token, true),
    cell(Role::Owner, Verb::Update, Resource::Token, true),
    cell(Role::Owner, Verb::Delete, Resource::Token, true),
    cell(Role::Owner, Verb::Read, Resource::Connector, true),
    cell(Role::Owner, Verb::Create, Resource::Connector, true),
    cell(Role::Owner, Verb::Update, Resource::Connector, true),
    cell(Role::Owner, Verb::Delete, Resource::Connector, true),
    cell(Role::Owner, Verb::Read, Resource::Audit, true),
    cell(Role::Admin, Verb::Read, Resource::Project, true),
    cell(Role::Admin, Verb::Create, Resource::Project, true),
    cell(Role::Admin, Verb::Update, Resource::Project, true),
    cell(Role::Admin, Verb::Delete, Resource::Project, false),
    cell(Role::Admin, Verb::Read, Resource::Member, true),
    cell(Role::Admin, Verb::Create, Resource::Member, true),
    cell(Role::Admin, Verb::Update, Resource::Member, true),
    cell(Role::Admin, Verb::Delete, Resource::Member, true),
    cell(Role::Admin, Verb::Read, Resource::Token, true),
    cell(Role::Admin, Verb::Create, Resource::Token, true),
    cell(Role::Admin, Verb::Update, Resource::Token, true),
    cell(Role::Admin, Verb::Delete, Resource::Token, true),
    cell(Role::Admin, Verb::Read, Resource::Connector, true),
    cell(Role::Admin, Verb::Create, Resource::Connector, true),
    cell(Role::Admin, Verb::Update, Resource::Connector, true),
    cell(Role::Admin, Verb::Delete, Resource::Connector, true),
    cell(Role::Admin, Verb::Read, Resource::Audit, true),
    cell(Role::Member, Verb::Read, Resource::Project, true),
    cell(Role::Member, Verb::Create, Resource::Project, false),
    cell(Role::Member, Verb::Update, Resource::Project, false),
    cell(Role::Member, Verb::Delete, Resource::Project, false),
    cell(Role::Member, Verb::Read, Resource::Member, true),
    cell(Role::Member, Verb::Create, Resource::Member, false),
    cell(Role::Member, Verb::Update, Resource::Member, false),
    cell(Role::Member, Verb::Delete, Resource::Member, false),
    cell(Role::Member, Verb::Read, Resource::Token, true),
    cell(Role::Member, Verb::Create, Resource::Token, false),
    cell(Role::Member, Verb::Update, Resource::Token, false),
    cell(Role::Member, Verb::Delete, Resource::Token, false),
    cell(Role::Member, Verb::Read, Resource::Connector, true),
    cell(Role::Member, Verb::Create, Resource::Connector, false),
    cell(Role::Member, Verb::Update, Resource::Connector, false),
    cell(Role::Member, Verb::Delete, Resource::Connector, false),
    cell(Role::Member, Verb::Read, Resource::Audit, false),
];

const fn cell(role: Role, verb: Verb, resource: Resource, allowed: bool) -> Cell {
    Cell {
        role,
        verb,
        resource,
        allowed,
    }
}

/// THE test this file exists for: `rbac::can` agrees with every explicit cell.
#[test]
fn enforcement_matches_matrix() {
    for c in MATRIX {
        assert_eq!(
            can(c.role, c.verb, c.resource),
            c.allowed,
            "can({:?}, {:?}, {:?}) disagrees with the matrix (expected {})",
            c.role,
            c.verb,
            c.resource,
            c.allowed,
        );
    }
}

/// Meaningless combos are denied though the matrix carries no row for them.
#[test]
fn enforcement_denies_meaningless_combos() {
    for &role in ALL_ROLES {
        for &verb in ALL_VERBS {
            for &resource in ALL_RESOURCES {
                if is_meaningless_combo(verb, resource) {
                    assert!(
                        !can(role, verb, resource),
                        "meaningless combo ({role:?}, {verb:?}, {resource:?}) must deny"
                    );
                }
            }
        }
    }
}

/// Every meaningful (role × verb × resource) cell has an explicit row.
#[test]
fn matrix_is_exhaustive_over_role_verb_resource() {
    let mut missing: Vec<(Role, Verb, Resource)> = vec![];

    for &role in ALL_ROLES {
        for &verb in ALL_VERBS {
            for &resource in ALL_RESOURCES {
                if is_meaningless_combo(verb, resource) {
                    continue;
                }
                let present = MATRIX
                    .iter()
                    .any(|c| c.role == role && c.verb == verb && c.resource == resource);
                if !present {
                    missing.push((role, verb, resource));
                }
            }
        }
    }

    assert!(
        missing.is_empty(),
        "RBAC matrix has {} missing cells — every meaningful \
         (role, verb, resource) combo needs an explicit row:\n{:#?}",
        missing.len(),
        missing,
    );
}

/// No cell is specified twice, or a reviewer would have to pick which one wins.
#[test]
fn matrix_has_no_duplicate_rows() {
    for (i, a) in MATRIX.iter().enumerate() {
        for b in MATRIX.iter().skip(i + 1) {
            assert!(
                !(a.role == b.role && a.verb == b.verb && a.resource == b.resource),
                "duplicate cell: {a:?}",
            );
        }
    }
}

/// No two roles may have the same row of verdicts.
#[test]
fn no_two_roles_have_identical_capabilities() {
    let verdicts = |role: Role| -> Vec<bool> {
        ALL_VERBS
            .iter()
            .flat_map(|&v| ALL_RESOURCES.iter().map(move |&r| can(role, v, r)))
            .collect()
    };
    for (i, &a) in ALL_ROLES.iter().enumerate() {
        for &b in ALL_ROLES.iter().skip(i + 1) {
            assert_ne!(
                verdicts(a),
                verdicts(b),
                "{a:?} and {b:?} grant exactly the same set — one of them is \
                 a duplicate wearing a different name",
            );
        }
    }
}

/// Member ⊂ Admin ⊂ Owner, over every explicit cell of the matrix.
#[test]
fn the_roles_nest() {
    for &verb in ALL_VERBS {
        for &resource in ALL_RESOURCES {
            if is_meaningless_combo(verb, resource) {
                continue;
            }
            if lookup(Role::Member, verb, resource).allowed {
                assert_allows(Role::Admin, verb, resource);
            }
            if lookup(Role::Admin, verb, resource).allowed {
                assert_allows(Role::Owner, verb, resource);
            }
        }
    }
}

#[test]
fn an_admin_cannot_delete_project() {
    assert_denies(Role::Admin, Verb::Delete, Resource::Project);
}

/// The Admin can do everything on the project level except delete it.
#[test]
fn an_admin_can_manage_everything_except_delete_project() {
    assert_allows(Role::Admin, Verb::Read, Resource::Project);
    assert_allows(Role::Admin, Verb::Create, Resource::Project);
    assert_allows(Role::Admin, Verb::Update, Resource::Project);
    assert_denies(Role::Admin, Verb::Delete, Resource::Project);

    assert_allows(Role::Admin, Verb::Read, Resource::Member);
    assert_allows(Role::Admin, Verb::Create, Resource::Member);
    assert_allows(Role::Admin, Verb::Update, Resource::Member);
    assert_allows(Role::Admin, Verb::Delete, Resource::Member);

    assert_allows(Role::Admin, Verb::Read, Resource::Token);
    assert_allows(Role::Admin, Verb::Create, Resource::Token);
    assert_allows(Role::Admin, Verb::Update, Resource::Token);
    assert_allows(Role::Admin, Verb::Delete, Resource::Token);

    assert_allows(Role::Admin, Verb::Read, Resource::Connector);
    assert_allows(Role::Admin, Verb::Create, Resource::Connector);
    assert_allows(Role::Admin, Verb::Update, Resource::Connector);
    assert_allows(Role::Admin, Verb::Delete, Resource::Connector);

    assert_allows(Role::Admin, Verb::Read, Resource::Audit);
}

/// Owner and Admin read the audit log, the Member does not, and no role writes it.
#[test]
fn the_owner_and_admins_read_the_audit_log_and_none_writes_it() {
    let readers: Vec<Role> = ALL_ROLES
        .iter()
        .copied()
        .filter(|&r| can(r, Verb::Read, Resource::Audit))
        .collect();
    assert_eq!(readers, vec![Role::Owner, Role::Admin]);
    for &role in ALL_ROLES {
        for verb in [Verb::Create, Verb::Update, Verb::Delete] {
            assert!(
                !can(role, verb, Resource::Audit),
                "{role:?} must not {verb:?} the audit chain"
            );
        }
    }
}

#[test]
fn member_cannot_mutate_anything() {
    for &resource in ALL_RESOURCES {
        for verb in [Verb::Create, Verb::Update, Verb::Delete] {
            if is_meaningless_combo(verb, resource) {
                continue;
            }
            assert_denies(Role::Member, verb, resource);
        }
    }
}

#[test]
fn owner_can_do_everything() {
    for &verb in ALL_VERBS {
        for &resource in ALL_RESOURCES {
            if is_meaningless_combo(verb, resource) {
                continue;
            }
            assert_allows(Role::Owner, verb, resource);
        }
    }
}

const ALL_ROLES: &[Role] = &Role::all();

const ALL_VERBS: &[Verb] = &[Verb::Read, Verb::Create, Verb::Update, Verb::Delete];

const ALL_RESOURCES: &[Resource] = &[
    Resource::Project,
    Resource::Member,
    Resource::Token,
    Resource::Connector,
    Resource::Audit,
];

/// The arrays above must list every variant of their enum, exactly once.
const fn role_ordinal(r: Role) -> usize {
    match r {
        Role::Owner => 0,
        Role::Admin => 1,
        Role::Member => 2,
    }
}
const ROLE_COUNT: usize = 3;

const fn verb_ordinal(v: Verb) -> usize {
    match v {
        Verb::Read => 0,
        Verb::Create => 1,
        Verb::Update => 2,
        Verb::Delete => 3,
    }
}
const VERB_COUNT: usize = 4;

const fn resource_ordinal(r: Resource) -> usize {
    match r {
        Resource::Project => 0,
        Resource::Member => 1,
        Resource::Token => 2,
        Resource::Connector => 3,
        Resource::Audit => 4,
    }
}
const RESOURCE_COUNT: usize = 5;

/// Assert one array covers `N` slots exactly once.
fn assert_covers<T: Copy + std::fmt::Debug>(
    array: &[T],
    count: usize,
    ordinal: fn(T) -> usize,
    name: &str,
) {
    assert_eq!(
        array.len(),
        count,
        "{name} has {} entries but its enum has {count} variants — a missing one \
         is not a smaller test, it is a variant whose every cell goes unchecked \
         while the suite reports a full pass",
        array.len()
    );
    let mut seen = vec![false; count];
    for &item in array {
        let i = ordinal(item);
        assert!(!seen[i], "{name} lists {item:?} twice");
        seen[i] = true;
    }
    let holes: Vec<usize> = seen
        .iter()
        .enumerate()
        .filter(|&(_, &b)| !b)
        .map(|(i, _)| i)
        .collect();
    assert!(
        holes.is_empty(),
        "{name} omits the variant(s) at ordinal {holes:?} — see the ordinal fn \
         above for which"
    );
}

#[test]
fn every_array_lists_every_variant_exactly_once() {
    assert_covers(ALL_ROLES, ROLE_COUNT, role_ordinal, "ALL_ROLES");
    assert_covers(ALL_VERBS, VERB_COUNT, verb_ordinal, "ALL_VERBS");
    assert_covers(
        ALL_RESOURCES,
        RESOURCE_COUNT,
        resource_ordinal,
        "ALL_RESOURCES",
    );
}

fn is_meaningless_combo(verb: Verb, resource: Resource) -> bool {
    matches!(
        (verb, resource),
        (Verb::Create | Verb::Update | Verb::Delete, Resource::Audit)
    )
}

#[track_caller]
fn assert_allows(role: Role, verb: Verb, resource: Resource) {
    let cell = lookup(role, verb, resource);
    assert!(
        cell.allowed,
        "expected {role:?} to be ALLOWED for {verb:?} on {resource:?}, matrix says deny",
    );
}

#[track_caller]
fn assert_denies(role: Role, verb: Verb, resource: Resource) {
    let cell = lookup(role, verb, resource);
    assert!(
        !cell.allowed,
        "expected {role:?} to be DENIED for {verb:?} on {resource:?}, matrix says allow",
    );
}

#[track_caller]
fn lookup(role: Role, verb: Verb, resource: Resource) -> &'static Cell {
    MATRIX
        .iter()
        .find(|c| c.role == role && c.verb == verb && c.resource == resource)
        .unwrap_or_else(|| {
            panic!("no cell in MATRIX for ({role:?}, {verb:?}, {resource:?})");
        })
}
