//! Per-transaction tenant binding for row-level security (RLS), stated in the
//! transaction's type.
//!
//! **Three keys, three GUCs, one wrapper.** [`organization_scope`] binds
//! `app.organization_id`, [`project_scope`] binds `app.project_id`, and
//! [`person_scope`] binds `app.user_id` — the person, for the rows that are
//! theirs rather than any organization's (their identity, sessions and codes)
//! and for the questions they ask about themselves ("which organizations am I
//! in?"). Each hands back a [`Scoped`] transaction whose type parameter says
//! what is bound, and a query function takes the binding it is written for.
//! A roster read under a project's binding, or a sweep outside its lane, is
//! then a compile error rather than zero rows.
//!
//! `set_config(..., true)` is transaction-local, and an unset or empty GUC
//! matches **nothing**, so a transaction that binds nothing reads and writes
//! zero rows. It is `set_config` rather than `SET LOCAL` because `SET LOCAL`
//! takes no bind parameters, and formatting an id into SQL is an injection
//! vector.
//!
//! **Bindings change only by the transitions on [`Scoped`]**, each of which
//! consumes the transaction and returns it as its new type. The policies are
//! permissive and OR together, so a binding left behind would quietly widen
//! every later read; the type is how a caller is made to say when one is
//! added or cleared. The states are exactly the ones the lanes use, and a new
//! combination is a new policy conversation, which is why [`Binding`] is
//! sealed.
//!
//! There is deliberately no wildcard value. Work that spans tenants goes
//! through [`maintenance_scope`], a distinct role, never a magic string a
//! handler could bind — and each service declares its own [`Lane`] marker in
//! its own crate, so a sibling's lane cannot be named, let alone entered.

use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use sqlx::{PgConnection, PgPool, Postgres, Transaction};

use crate::types::{OrganizationId, ProjectId, UserId};

mod sealed {
    pub trait Sealed {}
}

/// What a [`Scoped`] transaction has bound. Sealed to the states the policies
/// are written for.
pub trait Binding: sealed::Sealed + Send + Sync + 'static {}

/// `app.organization_id` alone: an organization's own rows.
#[derive(Debug)]
pub struct Organization;

/// `app.project_id` alone: one project's rows.
#[derive(Debug)]
pub struct Project;

/// `app.user_id` alone: a person's own rows, and their view of the rosters
/// they are on.
#[derive(Debug)]
pub struct Person;

/// The person and an organization: a person's act recorded on that
/// organization's chain, the owner's export, first-sign-in provisioning, and
/// account deletion walking what they own.
#[derive(Debug)]
pub struct PersonAndOrganization;

/// The project and its owning organization: a project lane reading the
/// organization's roster or flags on the project's behalf.
#[derive(Debug)]
pub struct ProjectAndOrganization;

/// The project and the caller, for the one read that resolves the caller's
/// role on the project. Cleared to [`Project`] before any tenant work.
#[derive(Debug)]
pub struct ProjectAndPerson;

/// All three: the owner's export walking each project, and an ownership
/// acceptance folding the new owner's seats.
#[derive(Debug)]
pub struct PersonOrganizationProject;

/// A service's cross-tenant lane, `SET LOCAL ROLE <service>_maintenance`, with
/// no key bound: what the lane may see is the role's grants and the
/// `maintenance_access` policies, table by table.
pub struct Maintenance<L: Lane>(PhantomData<L>);

macro_rules! bindings {
    ($($binding:ty),* $(,)?) => {
        $(
            impl sealed::Sealed for $binding {}
            impl Binding for $binding {}
        )*
    };
}

bindings!(
    Organization,
    Project,
    Person,
    PersonAndOrganization,
    ProjectAndOrganization,
    ProjectAndPerson,
    PersonOrganizationProject,
);

impl<L: Lane> sealed::Sealed for Maintenance<L> {}
impl<L: Lane> Binding for Maintenance<L> {}

/// Bindings under which `app.organization_id` is set: for a query keyed on
/// the organization that any of them admits.
pub trait HasOrganization: Binding {}
impl HasOrganization for Organization {}
impl HasOrganization for PersonAndOrganization {}
impl HasOrganization for ProjectAndOrganization {}
impl HasOrganization for PersonOrganizationProject {}

/// Bindings under which `app.project_id` is set.
pub trait HasProject: Binding {}
impl HasProject for Project {}
impl HasProject for ProjectAndOrganization {}
impl HasProject for ProjectAndPerson {}
impl HasProject for PersonOrganizationProject {}

/// Bindings under which `app.user_id` is set.
pub trait HasPerson: Binding {}
impl HasPerson for Person {}
impl HasPerson for PersonAndOrganization {}
impl HasPerson for ProjectAndPerson {}
impl HasPerson for PersonOrganizationProject {}

/// The cross-tenant lanes of this repository's modules — what a
/// [`maintenance_scope`] enters.
///
/// ⚠ **One role per module, never one shared role.** A single lane role held
/// privileges in every schema and every module's pool was a member, so a
/// flaw in any one module could `SET ROLE` into it and read its siblings'
/// tables across every tenant. Each role below is granted on its own module's
/// schema and the audit surface, and only its own login role is a member —
/// which holds in one process exactly as it did across several, because each
/// module keeps its own pool. A module outside this repository declares its
/// own role through [`Lane`], under the same rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceLane {
    /// `auth_maintenance`: the auth schema and the audit surface.
    Auth,
    /// `notifications_maintenance`: the notifications schema and the audit surface.
    Notifications,
    /// `agent_maintenance`: the agent schema alone. Its sources are read
    /// through the seams, in their own modules' lanes.
    Agent,
}

impl MaintenanceLane {
    /// The NOLOGIN role this lane runs as.
    #[must_use]
    pub const fn role(self) -> &'static str {
        match self {
            Self::Auth => "auth_maintenance",
            Self::Notifications => "notifications_maintenance",
            Self::Agent => "agent_maintenance",
        }
    }

    /// The statement that enters the lane, transaction-local.
    #[must_use]
    pub const fn set_role(self) -> &'static str {
        match self {
            Self::Auth => "SET LOCAL ROLE auth_maintenance",
            Self::Notifications => "SET LOCAL ROLE notifications_maintenance",
            Self::Agent => "SET LOCAL ROLE agent_maintenance",
        }
    }
}

/// A service's declaration of its lane: one unit type per service, in that
/// service's own crate, implementing this. A sibling cannot name a type it
/// does not depend on, so entering another service's lane is unwritable.
pub trait Lane: Send + Sync + 'static {
    /// The statement that enters the lane, transaction-local:
    /// `SET LOCAL ROLE <service>_maintenance`. This repository's services take
    /// it from [`MaintenanceLane::set_role`].
    const SET_ROLE: &'static str;
}

/// A transaction whose tenancy binding is its type. Only this module makes
/// one, and only the transitions on it change what is bound.
///
/// It derefs to the connection for the helpers that run under any binding —
/// `emit_audit`, the advisory locks, a test's own SQL — and [`Scoped::conn`]
/// is the same thing said explicitly, for a query function's own statements.
pub struct Scoped<'c, S: Binding> {
    tx: Transaction<'c, Postgres>,
    binding: PhantomData<S>,
}

impl<'c, S: Binding> Scoped<'c, S> {
    fn wrap(tx: Transaction<'c, Postgres>) -> Self {
        Self {
            tx,
            binding: PhantomData,
        }
    }

    fn into_binding<T: Binding>(self) -> Scoped<'c, T> {
        Scoped {
            tx: self.tx,
            binding: PhantomData,
        }
    }

    /// The connection, for this transaction's own statements.
    pub fn conn(&mut self) -> &mut PgConnection {
        &mut self.tx
    }

    /// Commit; every binding ends with the transaction.
    pub async fn commit(self) -> sqlx::Result<()> {
        self.tx.commit().await
    }

    /// Roll back; every binding ends with the transaction.
    pub async fn rollback(self) -> sqlx::Result<()> {
        self.tx.rollback().await
    }
}

impl<S: Binding> Deref for Scoped<'_, S> {
    type Target = PgConnection;

    fn deref(&self) -> &PgConnection {
        &self.tx
    }
}

impl<S: Binding> DerefMut for Scoped<'_, S> {
    fn deref_mut(&mut self) -> &mut PgConnection {
        &mut self.tx
    }
}

impl<S: Binding> fmt::Debug for Scoped<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scoped")
            .field("binding", &std::any::type_name::<S>())
            .finish_non_exhaustive()
    }
}

async fn bind_organization_guc(
    conn: &mut PgConnection,
    organization_id: &OrganizationId,
) -> sqlx::Result<()> {
    sqlx::query("SELECT set_config('app.organization_id', $1, true)")
        .bind(organization_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Takes [`ProjectId`], which does not convert from [`OrganizationId`]. That is
/// load-bearing: the two are indistinguishable strings, and binding the wrong
/// one would key a policy against a value from the other level.
async fn bind_project_guc(conn: &mut PgConnection, project_id: &ProjectId) -> sqlx::Result<()> {
    sqlx::query("SELECT set_config('app.project_id', $1, true)")
        .bind(project_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Takes [`UserId`], which does not convert from [`OrganizationId`]: a person
/// is not an organization.
async fn bind_person_guc(conn: &mut PgConnection, user_id: &UserId) -> sqlx::Result<()> {
    sqlx::query("SELECT set_config('app.user_id', $1, true)")
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Empty rather than `RESET`: `RESET` restores the SESSION value, which on a
/// pooled connection is whatever the last transaction left behind.
async fn clear_guc(conn: &mut PgConnection, guc: &'static str) -> sqlx::Result<()> {
    sqlx::query("SELECT set_config($1, '', true)")
        .bind(guc)
        .execute(conn)
        .await?;
    Ok(())
}

/// Begin a transaction bound to one organization.
pub async fn organization_scope<'a>(
    pool: &'a PgPool,
    organization_id: &OrganizationId,
) -> sqlx::Result<Scoped<'a, Organization>> {
    let mut tx = pool.begin().await?;
    bind_organization_guc(&mut tx, organization_id).await?;
    Ok(Scoped::wrap(tx))
}

/// Begin a transaction bound to one project.
pub async fn project_scope<'a>(
    pool: &'a PgPool,
    project_id: &ProjectId,
) -> sqlx::Result<Scoped<'a, Project>> {
    let mut tx = pool.begin().await?;
    bind_project_guc(&mut tx, project_id).await?;
    Ok(Scoped::wrap(tx))
}

/// Begin a transaction bound to one person.
pub async fn person_scope<'a>(
    pool: &'a PgPool,
    user_id: &UserId,
) -> sqlx::Result<Scoped<'a, Person>> {
    let mut tx = pool.begin().await?;
    bind_person_guc(&mut tx, user_id).await?;
    Ok(Scoped::wrap(tx))
}

/// Begin a transaction bound to a project and the caller, both in one
/// statement: one round trip rather than two on the path behind every
/// `resolve` a sibling module makes.
pub async fn project_and_person_scope<'a>(
    pool: &'a PgPool,
    project_id: &ProjectId,
    user_id: &UserId,
) -> sqlx::Result<Scoped<'a, ProjectAndPerson>> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT set_config('app.project_id', $1, true), set_config('app.user_id', $2, true)",
    )
    .bind(project_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    Ok(Scoped::wrap(tx))
}

/// Begin a **cross-tenant** transaction in the service's own lane:
/// `SET LOCAL ROLE <service>_maintenance`.
///
/// The tenancy boundary's one explicit fallback. The role reverts at commit or
/// rollback, so a pooled connection never keeps it — but every new caller
/// widens the boundary, so keep the call-site set small and audited.
pub async fn maintenance_scope<L: Lane>(
    pool: &PgPool,
    _lane: L,
) -> sqlx::Result<Scoped<'_, Maintenance<L>>> {
    let mut tx = pool.begin().await?;
    sqlx::query(L::SET_ROLE).execute(&mut *tx).await?;
    Ok(Scoped::wrap(tx))
}

impl<'c> Scoped<'c, Person> {
    /// Add the organization a person's act is recorded on, once they are
    /// shown to be in it: the chain's `WITH CHECK` names the organization.
    pub async fn bind_organization(
        mut self,
        organization_id: &OrganizationId,
    ) -> sqlx::Result<Scoped<'c, PersonAndOrganization>> {
        bind_organization_guc(self.conn(), organization_id).await?;
        Ok(self.into_binding())
    }
}

impl<'c> Scoped<'c, PersonAndOrganization> {
    /// Add one of the organization's projects, for its rows.
    pub async fn bind_project(
        mut self,
        project_id: &ProjectId,
    ) -> sqlx::Result<Scoped<'c, PersonOrganizationProject>> {
        bind_project_guc(self.conn(), project_id).await?;
        Ok(self.into_binding())
    }

    /// Back to the person alone: for a lane that walks each organization they
    /// own and must not carry one organization's binding into the next.
    pub async fn clear_organization(mut self) -> sqlx::Result<Scoped<'c, Person>> {
        clear_guc(self.conn(), "app.organization_id").await?;
        Ok(self.into_binding())
    }
}

impl<'c> Scoped<'c, PersonOrganizationProject> {
    /// Back to the person and the organization, before the next project.
    pub async fn clear_project(mut self) -> sqlx::Result<Scoped<'c, PersonAndOrganization>> {
        clear_guc(self.conn(), "app.project_id").await?;
        Ok(self.into_binding())
    }
}

impl<'c> Scoped<'c, Organization> {
    /// Add the caller, for a read of their own rows the organization may not
    /// see — whether they are on their way out.
    pub async fn bind_person(
        mut self,
        user_id: &UserId,
    ) -> sqlx::Result<Scoped<'c, PersonAndOrganization>> {
        bind_person_guc(self.conn(), user_id).await?;
        Ok(self.into_binding())
    }
}

impl<'c> Scoped<'c, Project> {
    /// Add the project's owning organization, for its roster and flags on the
    /// project's behalf.
    pub async fn bind_organization(
        mut self,
        organization_id: &OrganizationId,
    ) -> sqlx::Result<Scoped<'c, ProjectAndOrganization>> {
        bind_organization_guc(self.conn(), organization_id).await?;
        Ok(self.into_binding())
    }
}

impl<'c> Scoped<'c, ProjectAndOrganization> {
    /// Back to the project alone, the way the lane was handed it: an
    /// organization id left bound would widen every later read from one
    /// project to all of them.
    pub async fn clear_organization(mut self) -> sqlx::Result<Scoped<'c, Project>> {
        clear_guc(self.conn(), "app.organization_id").await?;
        Ok(self.into_binding())
    }
}

impl<'c> Scoped<'c, ProjectAndPerson> {
    /// Drop the caller once their role is read, so the person does not ride
    /// along on project work.
    pub async fn clear_person(mut self) -> sqlx::Result<Scoped<'c, Project>> {
        clear_guc(self.conn(), "app.user_id").await?;
        Ok(self.into_binding())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::database_url_or_skip;

    fn project(s: &str) -> ProjectId {
        ProjectId::try_new(s).expect("valid test project id")
    }

    fn person(s: &str) -> UserId {
        UserId::try_new(s).expect("valid test user id")
    }

    async fn setting(conn: &mut PgConnection, guc: &str) -> String {
        sqlx::query_scalar("SELECT current_setting($1, true)")
            .bind(guc)
            .fetch_one(conn)
            .await
            .expect("read")
    }

    /// `project_scope` round-trips through `current_setting`: the helper talks
    /// to the GUC name the policies read.
    #[tokio::test]
    async fn project_scope_binds_app_project_id_for_transaction() {
        let Some(url) = database_url_or_skip(module_path!()) else {
            return;
        };
        let pool = match crate::db::create_pool(&url, None).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let mut tx = project_scope(&pool, &project("project_test_abc"))
            .await
            .expect("scope");

        assert_eq!(
            setting(tx.conn(), "app.project_id").await,
            "project_test_abc"
        );

        tx.rollback().await.expect("rollback");
    }

    /// The empty-wildcard convention is gone at the type level: an empty
    /// binding cannot be constructed, and an empty GUC matches nothing anyway.
    #[test]
    fn empty_project_id_is_unrepresentable() {
        assert!(ProjectId::try_new("").is_err());
    }

    /// After commit, the GUC is cleared and the next transaction starts unset —
    /// the invariant that stops one request's binding leaking onto the next
    /// borrower of a pooled connection.
    #[tokio::test]
    async fn a_project_binding_does_not_leak_to_next_transaction() {
        let Some(url) = database_url_or_skip(module_path!()) else {
            return;
        };
        let pool = match crate::db::create_pool(&url, None).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };

        let tx = project_scope(&pool, &project("project_first"))
            .await
            .expect("scope-1");
        tx.commit().await.expect("commit-1");

        let mut tx = pool.begin().await.expect("tx-2");
        let got: Option<String> =
            sqlx::query_scalar("SELECT current_setting('app.project_id', true)")
                .fetch_one(&mut *tx)
                .await
                .expect("read");
        assert_eq!(
            got, None,
            "recycled pooled connection retained the GUC across commit"
        );
        tx.rollback().await.expect("rollback");
    }

    /// `person_scope` binds the GUC the person policies read.
    #[tokio::test]
    async fn person_scope_binds_app_user_id_for_transaction() {
        let Some(url) = database_url_or_skip(module_path!()) else {
            return;
        };
        let pool = crate::db::create_pool(&url, None)
            .await
            .expect("DATABASE_URL is set, so it must connect");
        let mut tx = person_scope(&pool, &person("user_test_abc"))
            .await
            .expect("scope");

        assert_eq!(setting(tx.conn(), "app.user_id").await, "user_test_abc");

        tx.rollback().await.expect("rollback");
    }

    /// Every transition leaves exactly the GUCs its type claims: the export's
    /// walk binds all three and unwinds to the person alone, and nothing
    /// survives that the type does not name.
    #[tokio::test]
    async fn transitions_bind_and_clear_exactly_what_their_types_say() {
        let Some(url) = database_url_or_skip(module_path!()) else {
            return;
        };
        let pool = crate::db::create_pool(&url, None)
            .await
            .expect("DATABASE_URL is set, so it must connect");
        let organization =
            OrganizationId::try_new("org_transitions").expect("valid test organization id");

        let tx = person_scope(&pool, &person("user_transitions"))
            .await
            .expect("scope");
        let tx = tx
            .bind_organization(&organization)
            .await
            .expect("bind organization");
        let mut tx = tx
            .bind_project(&project("project_transitions"))
            .await
            .expect("bind project");
        assert_eq!(setting(tx.conn(), "app.user_id").await, "user_transitions");
        assert_eq!(
            setting(tx.conn(), "app.organization_id").await,
            "org_transitions"
        );
        assert_eq!(
            setting(tx.conn(), "app.project_id").await,
            "project_transitions"
        );

        let tx = tx.clear_project().await.expect("clear project");
        let mut tx = tx.clear_organization().await.expect("clear organization");
        assert_eq!(setting(tx.conn(), "app.user_id").await, "user_transitions");
        assert_eq!(setting(tx.conn(), "app.organization_id").await, "");
        assert_eq!(setting(tx.conn(), "app.project_id").await, "");

        tx.rollback().await.expect("rollback");
    }

    /// The acting-project hop: the caller is bound for one read and gone
    /// before the project work, with the project binding intact.
    #[tokio::test]
    async fn clearing_the_person_keeps_the_project() {
        let Some(url) = database_url_or_skip(module_path!()) else {
            return;
        };
        let pool = crate::db::create_pool(&url, None)
            .await
            .expect("DATABASE_URL is set, so it must connect");

        let tx = project_and_person_scope(&pool, &project("project_hop"), &person("user_hop"))
            .await
            .expect("scope");
        let mut tx = tx.clear_person().await.expect("clear person");
        assert_eq!(setting(tx.conn(), "app.project_id").await, "project_hop");
        assert_eq!(setting(tx.conn(), "app.user_id").await, "");

        tx.rollback().await.expect("rollback");
    }
}
