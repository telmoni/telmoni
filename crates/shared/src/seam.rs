//! The seams between the modules one binary links.
//!
//! Auth, notifications and whatever a deployment adds beside them run in one
//! process, each on its own pool and database role. They still do not reach
//! into each other's tables: what one module asks of another goes through the
//! traits here, and the binary hands each module the implementations it
//! links. A test hands it a double instead.
//!
//! In process there is no hop to secure and nothing to cache: an answer is
//! the current row.

use async_trait::async_trait;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::acting::Acting;
use crate::{
    AuditAction, FlagSet, NotificationKind, OrganizationId, OrganizationStatus, ProjectId,
    TelmoniError, UserId,
};

/// Who may read a document the agent indexes, beyond the tenant its keys
/// name — the role the source's own lane asks for, which row-level security
/// cannot know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    /// Every role on the project (or, with no project, anyone the scope admits).
    Everyone,
    /// A role that may read the audit log: a project's own chain.
    Audit,
    /// The organization's owner and its admins, as the console reads it
    /// (`OrganizationRole::can_view_rolled_up_audit`): its whole chain and
    /// its own feed.
    OrganizationAdmin,
}

/// Where the indexer stopped in a source: the last row taken, in the
/// source's own order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentCursor {
    /// The row's time in the order the source pages by.
    pub at: DateTime<Utc>,
    /// Its id, which breaks a tie between rows of the same instant.
    pub id: String,
}

/// One row of a module's own, as text the agent can embed and cite. What
/// the text leaves out is the source's decision: an audit event's address
/// and user agent never reach it.
#[derive(Debug, Clone)]
pub struct SourceDocument {
    /// The row's id in its source, stable across re-reads.
    pub source_id: String,
    /// The organization it belongs to.
    pub organization_id: OrganizationId,
    /// The project it happened inside, if any.
    pub project_id: Option<ProjectId>,
    /// The person the text names, so their erasure can drop it.
    pub subject_user_id: Option<UserId>,
    /// Who may read it within that tenant.
    pub audience: Audience,
    /// One line.
    pub title: String,
    /// The text.
    pub body: String,
    /// The console path that shows it.
    pub url: String,
    /// When it happened.
    pub created_at: DateTime<Utc>,
    /// Where the next page starts.
    pub cursor: DocumentCursor,
}

/// Notifications' two indexed sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivitySource {
    /// The in-app feed.
    Feed,
    /// Connector deliveries, with their outcome.
    Delivery,
}

/// The audit tool's filters. The window and the page are the caller's; the
/// scope is the acting person's, decided by auth.
#[derive(Debug, Clone, Default)]
pub struct AuditEventsQuery {
    /// One actor's events.
    pub actor: Option<String>,
    /// One action's events.
    pub action: Option<AuditAction>,
    /// Events at or after this instant.
    pub from: Option<DateTime<Utc>>,
    /// Events before this instant.
    pub to: Option<DateTime<Utc>>,
    /// At most this many, newest first.
    pub limit: i64,
}

/// What a module does not answer, from a double or a module built without
/// it: the agent's reads refuse rather than answer empty.
fn not_linked(what: &str) -> TelmoniError {
    TelmoniError::Internal(format!("{what} is not answered by this module"))
}

/// Where a project is held, and how the console's paths spell it there:
/// `/{organization_slug}/{slug}`. A module keeps rows by id and links by slug,
/// since the slugs are the address a person reads and an id never is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectHome {
    /// The project asked about.
    pub project_id: ProjectId,
    /// The project's segment in console paths.
    pub slug: String,
    /// The organization holding it now.
    pub organization_id: OrganizationId,
    /// The organization's segment, which the project's follows.
    pub organization_slug: String,
}

/// What auth answers the modules beside it.
#[async_trait]
pub trait Auth: Send + Sync {
    /// Who is behind this request's bearer, acting on the organization and
    /// project its headers name (`authorization`, `x-organization-id`,
    /// `x-project-id`). The bearer is verified here; the caller asserts
    /// nothing about who is asking.
    async fn resolve(&self, headers: &HeaderMap) -> Result<Acting, TelmoniError>;

    /// The person `acting` names, on the same project, read again from the
    /// tables: their session still live, their role on it now, and the
    /// organization that holds it now. For a module still working on a
    /// request it resolved a while ago — the agent's tool calls, up to a
    /// minute and a half into a turn. The bearer is not presented again, so
    /// one that merely ran out since does not end the request.
    async fn resolve_again(&self, acting: &Acting) -> Result<Acting, TelmoniError> {
        let _ = acting;
        Err(not_linked("a role read again"))
    }

    /// Where each of `projects` is held now — its organization, and the slugs
    /// the console's paths spell both by — for every one that still exists;
    /// one that does not is left out. For a module keying rows on a project
    /// and its organization, finding the ones a transfer or a delete left
    /// behind, and for one spelling a link to a project's page. An error
    /// answers nothing, never "none of them exist".
    async fn project_homes(
        &self,
        projects: &[ProjectId],
    ) -> Result<Vec<ProjectHome>, TelmoniError> {
        let _ = projects;
        Err(not_linked("project homes"))
    }

    /// The slug each of `organizations` goes by, for every one that exists in
    /// any status; one that does not is left out. For a module spelling a
    /// link to an organization's own page.
    async fn organization_slugs(
        &self,
        organizations: &[OrganizationId],
    ) -> Result<Vec<(OrganizationId, String)>, TelmoniError> {
        let _ = organizations;
        Err(not_linked("organization slugs"))
    }

    /// The global feature-flag set. **An unreadable set is an error, never
    /// all-on**: a kill switch that fails open during the outage it was
    /// thrown for is no kill switch.
    async fn global_flags(&self) -> Result<FlagSet, TelmoniError>;

    /// Where an organization stands, for a module deciding whether something
    /// that arrived for it may land: `None` for one that does not exist.
    async fn organization_standing(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<OrganizationStatus>, TelmoniError>;

    /// The next page of audit events, oldest first after `after`, for the
    /// agent's index, read in auth's own lane. Nothing by default.
    async fn audit_documents(
        &self,
        after: Option<&DocumentCursor>,
        limit: i64,
    ) -> Result<Vec<SourceDocument>, TelmoniError> {
        let _ = (after, limit);
        Ok(Vec::new())
    }

    /// The acting project's roster, as the members page shows it to this
    /// person, re-checked here.
    async fn members(&self, acting: &Acting) -> Result<serde_json::Value, TelmoniError> {
        let _ = acting;
        Err(not_linked("members"))
    }

    /// Audit events this person may read: the project's to a role that reads
    /// the audit log, the whole organization's to its owner and admins.
    async fn audit_events(
        &self,
        acting: &Acting,
        query: &AuditEventsQuery,
    ) -> Result<serde_json::Value, TelmoniError> {
        let _ = (acting, query);
        Err(not_linked("audit events"))
    }
}

/// One notice, as a module hands it to notifications.
#[derive(Debug, Clone)]
pub struct Notice<'a> {
    /// The wire-stable kind.
    pub kind: NotificationKind,
    /// The person the title and body NAME, when they name one — the joiner
    /// of a `member_added` — so that their account's erasure can have the
    /// notice rewritten. `None` for a notice that names nobody.
    pub subject_user_id: Option<&'a UserId>,
    /// One line, 1–200 characters.
    pub title: &'a str,
    /// The longer text, 1–4000 characters.
    pub body: &'a str,
    /// A JSON object served to the console verbatim, at most 4 KiB.
    pub metadata: serde_json::Value,
    /// The emit-idempotency key, minted by the producer as `<producer>:<id>`.
    /// A replay answers the first emit's row and enqueues nothing.
    pub dedup_key: Option<&'a str>,
}

/// What a notice's write came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emitted {
    /// The feed row: a new one, or the one the key was first written under.
    pub feed_id: Uuid,
    /// The key had been written before, and nothing was queued this time.
    pub deduplicated: bool,
}

/// What notifications does for the modules beside it.
#[async_trait]
pub trait Notifications: Send + Sync {
    /// Write a notice to the feed and queue it to the connectors: the
    /// project's when one is named, else the organization's.
    async fn emit(
        &self,
        organization_id: &OrganizationId,
        project_id: Option<&ProjectId>,
        notice: Notice<'_>,
    ) -> Result<Emitted, TelmoniError>;

    /// Everything held for an organization — its connectors torn down
    /// upstream first, their deliveries, its feeds — before its row goes.
    /// Answers how many feed rows went. Idempotent.
    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError>;

    /// Everything held for one project: before it is handed to another
    /// organization, and after it is deleted. Idempotent.
    async fn purge_project(&self, project_id: &ProjectId) -> Result<u64, TelmoniError>;

    /// Every notice that named the person, rewritten to name a former
    /// member, on every feed and every delivery still holding the text.
    /// Answers how many rows were rewritten. Idempotent, and an error stops
    /// the erasure it is part of.
    async fn redact_person(&self, user_id: &UserId) -> Result<u64, TelmoniError>;

    /// The next page of one source, in its own order after `after`, for the
    /// agent's index, read in notifications' own lane. Nothing by default.
    async fn activity_documents(
        &self,
        source: ActivitySource,
        after: Option<&DocumentCursor>,
        limit: i64,
    ) -> Result<Vec<SourceDocument>, TelmoniError> {
        let _ = (source, after, limit);
        Ok(Vec::new())
    }

    /// The acting project's connectors, as the connectors page lists them.
    async fn connectors(&self, acting: &Acting) -> Result<serde_json::Value, TelmoniError> {
        let _ = acting;
        Err(not_linked("connectors"))
    }

    /// One connector's latest deliveries with their sends, as its delivery
    /// log shows them.
    async fn connector_deliveries(
        &self,
        acting: &Acting,
        connector_id: Uuid,
        limit: i64,
    ) -> Result<serde_json::Value, TelmoniError> {
        let _ = (acting, connector_id, limit);
        Err(not_linked("connector deliveries"))
    }
}

/// What the agent does for the modules beside it: forget what it holds of a
/// person or an organization when they go. Each purge is idempotent and
/// answers how many rows went. A project that moved or was deleted needs no
/// call: the agent asks [`Auth::project_homes`] itself, hourly, and removes
/// what it holds of the project under an organization that no longer has it.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Every conversation of the person's, and every passage that names them.
    ///
    /// The model quotes their `address` and `name` from the member list into
    /// other people's answers, which carry no id to find them by, so those
    /// are scrubbed by the text itself: the address, theirs alone, wherever
    /// it is; the name, which is not, in the `organizations` they are in.
    ///
    /// An answer still being written in those organizations when this runs
    /// is withheld when it lands rather than saved. Idempotent, and auth
    /// calls it twice: before the person's memberships go, while it can still
    /// name the organizations, and after, once no lane can read them.
    async fn erase_person(
        &self,
        user_id: &UserId,
        address: &str,
        name: Option<&str>,
        organizations: &[OrganizationId],
    ) -> Result<u64, TelmoniError>;

    /// Everything held for the organization.
    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError>;
}

/// A module of the deployment's own that holds something of an
/// organization's outside auth's tables — a subscription, say — and must
/// drop it before the organization goes.
///
/// Auth calls it when a deletion is confirmed and again before the row
/// goes, and keeps the row until it answers `Ok`. Idempotent, then.
#[async_trait]
pub trait PurgeHook: Send + Sync {
    /// Drop what is held for the organization.
    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<(), TelmoniError>;
}
