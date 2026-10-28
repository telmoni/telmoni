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

use std::collections::HashMap;

use async_trait::async_trait;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::acting::{Acting, ActingKey};
use crate::slug::Slug;
use crate::{
    AlertCeilings, AuditAction, AuthError, FlagSet, Grants, LaneCeilings, MemberCeiling,
    NotificationKind, OrganizationId, OrganizationStatus, ProjectId, Role, TelmoniError, UserId,
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
    /// (`OrganizationAction::ReadWholeAudit`): its whole chain and
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
    pub id: Uuid,
}

/// One row of a module's own, as text the agent can embed and cite. What
/// the text leaves out is the source's decision: an audit event's address
/// and user agent never reach it.
#[derive(Debug, Clone)]
pub struct SourceDocument {
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

/// How long a writer holding the chain lock waits for auth's second answer:
/// well inside [`crate::audit::CHAIN_LOCK_WAIT`], which every other writer in
/// the organization waits for that lock.
pub const ANSWER_BUDGET: std::time::Duration = std::time::Duration::from_secs(1);

/// The refusal of a write whose project moved to another organization since
/// it read it.
pub const PROJECT_MOVED: &str =
    "the project has moved to another organization; reload and try again";

/// `acting` read again within [`ANSWER_BUDGET`]: the answer that stands to
/// the commit, refused where auth does not give it in time or the project has
/// moved.
pub async fn resolve_again_in_time(
    auth: &dyn Auth,
    acting: &Acting,
) -> Result<Acting, TelmoniError> {
    let Ok(now) = tokio::time::timeout(ANSWER_BUDGET, auth.resolve_again(acting)).await else {
        tracing::warn!(
            organization_id = %acting.organization_id,
            "auth did not answer a change again in time"
        );
        return Err(AuthError::IdentityUnavailable.into());
    };
    let now = now?;
    if now.organization_id != acting.organization_id {
        return Err(AuthError::Conflict(PROJECT_MOVED.into()).into());
    }
    Ok(now)
}

/// Where a project is held, and how the console's paths spell it there:
/// `/{organization_slug}/{slug}`. A module keeps rows by id and links by slug,
/// since the slugs are the address a person reads and an id never is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectHome {
    /// The project asked about.
    pub project_id: ProjectId,
    /// The project's segment in console paths.
    pub slug: Slug,
    /// The organization holding it now.
    pub organization_id: OrganizationId,
    /// The organization's segment, which the project's follows.
    pub organization_slug: Slug,
}

/// A project a person can open, as auth answers a module that shows each
/// person their part of an organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadableProject {
    /// The project.
    pub project_id: ProjectId,
    /// Its segment in console paths, under its organization's.
    pub slug: Slug,
    /// Its name, as the console shows it.
    pub name: String,
    /// The person's role on it, as [`crate::rbac::project_role`] decides it.
    pub role: Role,
}

/// One active organization a person owns, as auth answers a module that
/// decides something for each owner rather than each organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedOrganization {
    /// The organization.
    pub organization_id: OrganizationId,
    /// When it was founded, which orders a person's organizations oldest
    /// first.
    pub created_at: DateTime<Utc>,
}

/// An organization a deployment founds for a person once it has what it
/// asks for first — a payment, say — rather than at the person's asking:
/// what [`Auth::found_organization`] is handed. The deployment mints the id,
/// so a founding tried again after a failure founds the same organization
/// once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizationFounding {
    /// The id the deployment minted for it, which it is founded under.
    pub organization_id: OrganizationId,
    /// Its founder and owner.
    pub founder: UserId,
    /// Its name, as the founder gave it.
    pub name: String,
    /// The URL the founder asked for, if any. One no longer free, or none,
    /// is derived from the name, never refused: the founding is owed.
    pub slug: Option<Slug>,
}

/// An organization founded for a deployment, as its founder is sent to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundedOrganization {
    /// The organization, under the id the deployment minted.
    pub organization_id: OrganizationId,
    /// Its segment in console paths.
    pub slug: Slug,
}

/// A project moved into another organization of its owner's, as
/// [`Auth::move_project`] answers it: where a page showing it follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedProject {
    /// The project, under the id it keeps.
    pub project_id: ProjectId,
    /// Its name where it landed, numbered when the destination held it.
    pub name: String,
    /// Its segment in console paths.
    pub slug: Slug,
    /// The destination's segment in console paths.
    pub organization_slug: Option<Slug>,
}

/// What [`Auth::close_if_empty`] came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closing {
    /// Its deletion begun, as its owner's own deletion begins it.
    Closed,
    /// Kept, the code untouched: a project is in it still.
    Holding,
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
    /// minute and a half into a turn — or one deciding under a lock what it
    /// resolved before taking it, as telemetry's content switch does under
    /// the organization's chain lock. The bearer is not presented again, so
    /// one that merely ran out since does not end the request.
    async fn resolve_again(&self, acting: &Acting) -> Result<Acting, TelmoniError> {
        let _ = acting;
        Err(not_linked("a role read again"))
    }

    /// The API key this request's bearer presents (`authorization`), and
    /// what it opens: its project, the organization holding that project
    /// now, and its scopes. The key is verified here — live, unexpired, its
    /// organization active — and holding it to a scope is the caller's
    /// ([`ActingKey::require_scope`]). For a lane a machine calls with a key
    /// rather than a person with a session: ingest's.
    async fn resolve_api_key(&self, headers: &HeaderMap) -> Result<ActingKey, TelmoniError> {
        let _ = headers;
        Err(not_linked("an API key"))
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
    ) -> Result<Vec<(OrganizationId, Slug)>, TelmoniError> {
        let _ = organizations;
        Err(not_linked("organization slugs"))
    }

    /// The projects of the organization `acting` stands in that the person
    /// can open, with their role on each, as the console lists them.
    async fn readable_projects(
        &self,
        acting: &Acting,
    ) -> Result<Vec<ReadableProject>, TelmoniError> {
        let _ = acting;
        Err(not_linked("the projects a person can open"))
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

    /// The active organizations the person owns, oldest first. One waiting
    /// out its deletion is left out: it holds nothing for anybody any more.
    async fn organizations_owned_by(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<OwnedOrganization>, TelmoniError> {
        let _ = user_id;
        Err(not_linked("the organizations a person owns"))
    }

    /// Who owns the organization now, in any status: one waiting out its
    /// deletion still has its owner. `None` for one that does not exist.
    async fn owner_of(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<UserId>, TelmoniError> {
        let _ = organization_id;
        Err(not_linked("an organization's owner"))
    }

    /// Everybody who owns two or more active organizations, for a module
    /// whose answer for one of them depends on the others.
    async fn owners_of_several(&self) -> Result<Vec<UserId>, TelmoniError> {
        Err(not_linked("the owners of several organizations"))
    }

    /// Whether a name and a URL could found an organization now, refused as
    /// the founder's own asking would be refused — the name cleaning to
    /// nothing, the URL a word the console keeps, new organizations closed
    /// — for a deployment that takes something of the founder first, a
    /// payment, and must refuse before it does rather than after.
    async fn check_founding(&self, name: &str, slug: Option<&Slug>) -> Result<(), TelmoniError> {
        let _ = (name, slug);
        Err(not_linked("a founding's check"))
    }

    /// Found an organization for a deployment that has what it asks for
    /// first ([`OrganizationFounding`]): its row, its owner and its audit
    /// events, as the founder's own asking would make them, and the
    /// deployment's ownership hook told. Idempotent by the id: one already
    /// founded is answered as it stands. Never refused for the deployment's
    /// own say, which it had at [`Auth::check_founding`], nor for new
    /// organizations closed since: the organization is owed. Refused as a
    /// `401` or `403` when the founder's account is gone, or being deleted,
    /// which is for good.
    async fn found_organization(
        &self,
        founding: &OrganizationFounding,
    ) -> Result<FoundedOrganization, TelmoniError> {
        let _ = founding;
        Err(not_linked("founding an organization"))
    }

    /// Move `project_id` into `into`, both held by organizations the person
    /// `acting` owns, as their own move does (`POST
    /// /internal/v1/projects/{id}/move`): its keys, its telemetry and its
    /// seats go with it, and its connectors are purged.
    async fn move_project(
        &self,
        acting: &Acting,
        project_id: &ProjectId,
        into: &OrganizationId,
    ) -> Result<MovedProject, TelmoniError> {
        let _ = (acting, project_id, into);
        Err(not_linked("moving a project"))
    }

    /// Delete the organization `acting` stands in with the code mailed to its
    /// owner, as their own deletion does (`DELETE /internal/v1/organization`),
    /// unless a project is in it: counted under its lock, which a project made
    /// or landing there takes too, so none goes with it.
    async fn close_if_empty(&self, acting: &Acting, code: &str) -> Result<Closing, TelmoniError> {
        let _ = (acting, code);
        Err(not_linked("closing an emptied organization"))
    }

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

/// A word to an organization's owner and admins on its own feed, never a
/// project's: a plan's, or a ceiling's. Its kind is `organization_alert`.
#[derive(Debug, Clone)]
pub struct OrganizationAlert<'a> {
    /// One line, 1–200 characters.
    pub title: &'a str,
    /// The longer text, 1–4000 characters.
    pub body: &'a str,
    /// A JSON object served to the console verbatim, at most 4 KiB.
    pub metadata: serde_json::Value,
    /// The emit-idempotency key, minted by the producer as `<producer>:<id>`.
    pub dedup_key: Option<&'a str>,
}

impl<'a> OrganizationAlert<'a> {
    /// The notice it is written as.
    #[must_use]
    pub fn into_notice(self) -> Notice<'a> {
        Notice {
            kind: NotificationKind::OrganizationAlert,
            subject_user_id: None,
            title: self.title,
            body: self.body,
            metadata: self.metadata,
            dedup_key: self.dedup_key,
        }
    }
}

/// Where an alert's notice goes: the connections its alert links, under a
/// key of the producer's own (`telemetry:rule:<id>`) by which each one's
/// failed deliveries are counted, five in a row muting it for that key alone.
#[derive(Debug, Clone, Copy)]
pub struct Route<'a> {
    /// The producer's key, 1 to 200 characters.
    pub key: &'a str,
    /// The connections the notice may go to: the project's own, the rest
    /// ignored.
    pub connections: &'a [Uuid],
    /// The most deliveries it may make, given by id to the connections that hear it.
    pub most: usize,
}

/// What a notice's write came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emitted {
    /// A new row, queued to its connections.
    Fresh {
        /// The feed row.
        feed_id: Uuid,
    },
    /// The key had been written before: the row the first emit minted, and
    /// nothing queued this time.
    Replayed {
        /// The feed row.
        feed_id: Uuid,
    },
}

impl Emitted {
    /// The feed row, new or first written.
    #[must_use]
    pub fn feed_id(self) -> Uuid {
        match self {
            Self::Fresh { feed_id } | Self::Replayed { feed_id } => feed_id,
        }
    }

    /// Whether the key had been written before.
    #[must_use]
    pub fn replayed(self) -> bool {
        matches!(self, Self::Replayed { .. })
    }
}

/// What a routed notice's write came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routed {
    /// The feed row's write.
    pub emitted: Emitted,
    /// The connections it was queued to: where it went, as its producer
    /// keeps it. None where the key had been written before.
    pub connections: Vec<Uuid>,
}

/// What notifications does for the modules beside it.
#[async_trait]
pub trait Notifications: Send + Sync {
    /// Write a notice to the project's feed and queue it to the project's
    /// connectors.
    async fn emit(
        &self,
        organization_id: &OrganizationId,
        project_id: &ProjectId,
        notice: Notice<'_>,
    ) -> Result<Emitted, TelmoniError>;

    /// Write an alert to the organization's own feed and queue it to the
    /// organization's connectors.
    async fn alert_organization(
        &self,
        organization_id: &OrganizationId,
        alert: OrganizationAlert<'_>,
    ) -> Result<Emitted, TelmoniError>;

    /// Write a notice to the project's feed and queue it to the connections
    /// `route` names that are active, take its kind and are not muted for the
    /// route. A deployment's own notifications that route nothing emit it,
    /// and name no connection it went to.
    async fn emit_routed(
        &self,
        organization_id: &OrganizationId,
        project_id: &ProjectId,
        notice: Notice<'_>,
        route: Route<'_>,
    ) -> Result<Routed, TelmoniError> {
        let _ = route;
        Ok(Routed {
            emitted: self.emit(organization_id, project_id, notice).await?,
            connections: Vec::new(),
        })
    }

    /// Queue a notice to the connections `route` names alone, as
    /// [`Notifications::emit_routed`] picks them, with no feed row; answers
    /// the connections it was queued to.
    async fn remind(
        &self,
        organization_id: &OrganizationId,
        project_id: &ProjectId,
        notice: Notice<'_>,
        route: Route<'_>,
    ) -> Result<Vec<Uuid>, TelmoniError> {
        let _ = (organization_id, project_id, notice, route);
        Ok(Vec::new())
    }

    /// The project's connections muted for `route_key`: those whose last five
    /// deliveries under it failed.
    async fn muted(
        &self,
        project_id: &ProjectId,
        route_key: &str,
    ) -> Result<Vec<Uuid>, TelmoniError> {
        let _ = (project_id, route_key);
        Ok(Vec::new())
    }

    /// The project's muted connections under each of `route_keys`, read at
    /// once: a key with none is left out. One read for a page of alerts,
    /// where [`muted`](Self::muted) is one a route.
    async fn muted_routes(
        &self,
        project_id: &ProjectId,
        route_keys: &[String],
    ) -> Result<HashMap<String, Vec<Uuid>>, TelmoniError> {
        let mut muted = HashMap::with_capacity(route_keys.len());
        for key in route_keys {
            let connections = self.muted(project_id, key).await?;
            if !connections.is_empty() {
                muted.insert(key.clone(), connections);
            }
        }
        Ok(muted)
    }

    /// One of the project's connections heard again under `route_key`, its
    /// failures forgotten. Answers whether it was muted.
    async fn unmute(
        &self,
        project_id: &ProjectId,
        route_key: &str,
        connection_id: Uuid,
    ) -> Result<bool, TelmoniError> {
        let _ = (project_id, route_key, connection_id);
        Ok(false)
    }

    /// Everything held for an organization — its connectors torn down
    /// upstream first, their deliveries, its feeds — before its row goes.
    /// Answers how many feed rows went. Idempotent.
    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError>;

    /// Everything held for one project after it is deleted: its connections
    /// first, then its feed in chunks, so a purge cut short keeps what it did.
    /// Idempotent; one that fails is retried by the module's hourly retention
    /// sweep, so the caller only logs it.
    async fn purge_project(&self, project_id: &ProjectId) -> Result<u64, TelmoniError>;

    /// The same before the project is handed to another organization, in one
    /// transaction, so a move refused leaves it as it was: the caller's budget
    /// bounds it, and a move it does not fit is refused.
    async fn purge_project_for_move(&self, project_id: &ProjectId) -> Result<u64, TelmoniError>;

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

/// What telemetry does for the modules beside it: forget what it holds of an
/// organization or a project when it goes, and follow a project to the
/// organization it moves to. Each call is idempotent; each purge answers how
/// many rows went.
///
/// What it holds is each project's settings, in Postgres, and the project's
/// spans and hourly totals, in ClickHouse. A span carries its project and
/// nothing above it, so a transfer leaves every span where it is; a purge
/// marks every span and every hour's totals of the project deleted, hidden
/// from every read within moments, as the store applies the mark, and
/// rewritten away by the nightly `rotate`, then removes its settings.
#[async_trait]
pub trait Telemetry: Send + Sync {
    /// Everything held for an organization's projects, before its row goes.
    ///
    /// ⚠ Call it holding no lock: the purge holds each project alone, and
    /// waits for the writes holding it in place, so a caller holding a lock
    /// they want deadlocks where Postgres cannot see it.
    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError>;

    /// Everything held for one project, after it is deleted. ⚠ Call it
    /// holding no lock, as [`Telemetry::purge_organization`]. One that fails
    /// is retried by the module's own sweep, so the caller only logs it.
    async fn purge_project(&self, project_id: &ProjectId) -> Result<u64, TelmoniError>;

    /// Whether the organization's machines may be sent more today: its read
    /// API and its exports refused, a 429 until midnight UTC, once its plan's
    /// data out for the day is spent; the CLI's answers are counted, never
    /// asked. Room by default, and wherever the day's count cannot be read.
    async fn admit_data_out(&self, organization_id: &OrganizationId) -> Result<(), TelmoniError> {
        let _ = organization_id;
        Ok(())
    }

    /// `bytes` sent to the organization's machines, or downloaded from one of
    /// its exports, counted against its day. Nothing by default.
    async fn spend_data_out(&self, organization_id: &OrganizationId, bytes: u64) {
        let _ = (organization_id, bytes);
    }

    /// A project handed to another organization: its settings — its content
    /// mode among them — go with it, so the new organization's purge, and
    /// never the old one's, is what removes them.
    ///
    /// ⚠ **The caller holds the project's move lock alone** across the call
    /// and to its own commit ([`crate::db::project_move::lock_for_move`]): an
    /// export making the project's settings row holds the project in place
    /// while it writes, so the move waits for it, and one kept out files the
    /// row under the organization it finds the project in once it is let in.
    /// A move given up on over its budget may still commit after the caller
    /// lets go; its settling takes the lock again.
    async fn move_project(
        &self,
        project_id: &ProjectId,
        organization_id: &OrganizationId,
    ) -> Result<(), TelmoniError>;
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

/// A change in who owns which organization, as auth tells an
/// [`OwnershipHook`] once it has committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipChange {
    /// `owner` founded the organization: one they asked for, or the first
    /// `/me` provisioned for them.
    Founded {
        /// The new organization.
        organization_id: OrganizationId,
        /// Its founder and owner.
        owner: UserId,
    },
    /// The organization changed hands: its admin accepted the owner's offer.
    HandedOver {
        /// The organization.
        organization_id: OrganizationId,
        /// Who owned it until now; `None` where the roster held no owner.
        from: Option<UserId>,
        /// Who owns it now.
        to: UserId,
    },
}

/// A module of the deployment's own whose answer for an organization depends
/// on who owns it — a plan limited to one per person, say — and so has a say
/// in an organization changing hands, and is told when one is founded or
/// changes hands. An organization's deletion reaches it as a [`PurgeHook`]
/// does, since that is where the deployment already hears of it.
///
/// ⚠ **Asked before any lock, told after the commit.** Auth asks
/// [`OwnershipHook::may_take_ownership`] before it takes the locks an offer
/// or an accept writes under, because the hook reads auth back through
/// [`Auth`] on auth's own pool; it tells [`OwnershipHook::ownership_changed`]
/// once the change has committed, within a bound, and logs a failure, which
/// the module's own catching up must cover.
#[async_trait]
pub trait OwnershipHook: Send + Sync {
    /// Whether `recipient` may come to own the organization: asked before the
    /// owner offers it to them and again before they accept it. An `Err` is
    /// the refusal the person reads.
    async fn may_take_ownership(
        &self,
        organization_id: &OrganizationId,
        recipient: &UserId,
    ) -> Result<(), TelmoniError>;

    /// One change, once committed. Idempotent: the module's own catching up
    /// may arrive at the same answer.
    async fn ownership_changed(&self, change: OwnershipChange) -> Result<(), TelmoniError>;

    /// Whether `founder` may found another organization at their asking:
    /// asked before `POST /internal/v1/organizations` founds one, never
    /// before `/me` provisions a person's first. An `Err` is the refusal the person
    /// reads: a deployment that has a second organization paid for first
    /// answers a 402 naming the plan, and founds it itself once it is
    /// ([`Auth::found_organization`]). Yes, by default.
    async fn may_found(&self, founder: &UserId) -> Result<(), TelmoniError> {
        let _ = founder;
        Ok(())
    }
}

/// What an organization may take in, as a deployment that sells plans
/// answers it: a module of its own, handed to the core empty and filled once
/// it exists, as a [`PurgeHook`] is. A binary that sells nothing answers
/// [`NoLimits`]. Its answer is read at each door, before anything new is
/// taken in ([`admit_new`]).
#[async_trait]
pub trait Entitlements: Send + Sync {
    /// The organization's grants now.
    async fn grants(&self, organization_id: &OrganizationId) -> Result<Grants, TelmoniError>;

    /// How many people the organization may hold, those it has invited
    /// among them; `None` is as many as it invites. Asked apart from
    /// [`Entitlements::grants`], which every span asks and so must be
    /// answered from memory: a roster changes rarely, and this answer may
    /// read the deployment's own rows.
    async fn member_ceiling(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<MemberCeiling>, TelmoniError> {
        let _ = organization_id;
        Ok(None)
    }

    /// The machine lanes' ceiling the organization's plan sets — data out a
    /// day; `None` is none. Asked on every answer the read API gives, so its
    /// callers keep the answer a minute ([`lane_ceilings`]), and it may read
    /// the deployment's own rows.
    async fn lane_ceilings(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<LaneCeilings>, TelmoniError> {
        let _ = organization_id;
        Ok(None)
    }

    /// The alerts the organization's plan holds, and their deliveries a day;
    /// `None` is as many as it makes and sends. Asked when an alert is made
    /// and by the loop that evaluates them, and it may read the deployment's
    /// own rows.
    async fn alert_ceilings(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<AlertCeilings>, TelmoniError> {
        let _ = organization_id;
        Ok(None)
    }
}

/// A binary that sells nothing: every organization holds everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLimits;

#[async_trait]
impl Entitlements for NoLimits {
    async fn grants(&self, organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        let _ = organization_id;
        Ok(Grants::unlimited())
    }
}

/// What a door asks before it takes in anything new for an organization —
/// an export, a question, an invitation: refused while its grants refuse it.
/// An answer that cannot be had lets it in, logged, since an outage of ours
/// must never stop an agent reporting, and nothing it would refuse costs
/// more than an outage lasts.
pub async fn admit_new(
    entitlements: &dyn Entitlements,
    organization_id: &OrganizationId,
) -> Result<(), TelmoniError> {
    match entitlements.grants(organization_id).await {
        Ok(grants) => grants.admit_new(),
        Err(e) => {
            tracing::warn!(organization_id = %organization_id, error = %e,
                "entitlements could not be read; let in");
            Ok(())
        }
    }
}

/// A ceiling `read` answers, or none when the answer cannot be had: logged
/// as `what`, and nothing held, as [`admit_new`] lets in what it cannot decide.
async fn or_none<T>(
    organization_id: &OrganizationId,
    what: &str,
    read: impl Future<Output = Result<Option<T>, TelmoniError>>,
) -> Option<T> {
    match read.await {
        Ok(ceiling) => ceiling,
        Err(e) => {
            tracing::warn!(organization_id = %organization_id, error = %e, what,
                "a ceiling could not be read; none is held");
            None
        }
    }
}

/// What the roster's doors ask before a seat is promised or taken: the
/// organization's member ceiling, which the door then holds its count to
/// ([`MemberCeiling::admit`]) once that count is read under its own lock.
/// An answer that cannot be had is no ceiling, logged, as [`admit_new`]
/// lets in what it cannot decide.
pub async fn member_ceiling(
    entitlements: &dyn Entitlements,
    organization_id: &OrganizationId,
) -> Option<MemberCeiling> {
    or_none(
        organization_id,
        "the member ceiling",
        entitlements.member_ceiling(organization_id),
    )
    .await
}

/// What an alert is held to, made or evaluated: the organization's alert
/// ceilings. An answer that cannot be had is no ceiling, logged, as
/// [`admit_new`] lets in what it cannot decide.
pub async fn alert_ceilings(
    entitlements: &dyn Entitlements,
    organization_id: &OrganizationId,
) -> Option<AlertCeilings> {
    or_none(
        organization_id,
        "the alert ceilings",
        entitlements.alert_ceilings(organization_id),
    )
    .await
}

/// What a machine lane asks before it lets an answer or an export through:
/// the organization's lane ceilings. An answer that cannot be had is
/// no ceiling, logged, as [`admit_new`] lets in what it cannot decide.
pub async fn lane_ceilings(
    entitlements: &dyn Entitlements,
    organization_id: &OrganizationId,
) -> Option<LaneCeilings> {
    or_none(
        organization_id,
        "the lane ceilings",
        entitlements.lane_ceilings(organization_id),
    )
    .await
}
