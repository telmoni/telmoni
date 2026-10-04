//! The notifications data layer: the in-app feed, the connections, the
//! delivery queue and the handshake states.
//!
//! **No function here returns a webhook URL or a bot token in the clear.**
//! Sealed columns come out as `Sealed` pairs; opening them is the vault's job,
//! at the moment of use.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::FromRow;
use telmoni_shared::db::tenant_session::{
    Binding, HasProject, Lane, Maintenance, MaintenanceLane, Organization, Project,
    ProjectAndOrganization, Scoped,
};
use telmoni_shared::envelope::Sealed;
use telmoni_shared::{OrganizationId, ProjectId, UserId};
use uuid::Uuid;

use crate::connector::Provider;

/// This service's cross-tenant lane: the one declaration, so a sibling's
/// lane is unnameable here. See `tenant_session::Lane`.
#[derive(Debug, Clone, Copy)]
pub struct NotificationsLane;
impl Lane for NotificationsLane {
    const SET_ROLE: &'static str = MaintenanceLane::Notifications.set_role();
}

/// What the emit lane binds: the project for a project notice, the
/// organization for one that names no project.
pub trait EmitBinding: Binding {}
impl EmitBinding for Project {}
impl EmitBinding for Organization {}

/// A feed row as served to the BFF's bell-dropdown.
#[derive(Debug, Serialize, FromRow)]
pub struct FeedItem {
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub metadata: Value,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// One notice's content, as an emit carries it to the feed.
#[derive(Debug, Clone, Copy)]
pub struct NewFeedItem<'a> {
    /// The person the text names, when it names one, for the redaction their
    /// erasure asks for.
    pub subject_user_id: Option<&'a UserId>,
    pub kind: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub metadata: &'a Value,
    /// The emit-idempotency key; `None` opts out.
    pub dedup_key: Option<&'a str>,
}

/// Insert one feed row. `None` means a REPLAY — the `dedup_key` already exists
/// — and the caller must skip the fan-out the first emit already enqueued.
pub async fn insert_feed<B: Binding>(
    tx: &mut Scoped<'_, B>,
    project_id: Option<&ProjectId>,
    organization_id: &OrganizationId,
    item: &NewFeedItem<'_>,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO notifications.feed
             (id, project_id, organization_id, subject_user_id, kind, title, body,
              metadata, dedup_key, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (organization_id, dedup_key, project_id) WHERE dedup_key IS NOT NULL DO NOTHING
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(organization_id)
    .bind(item.subject_user_id)
    .bind(item.kind)
    .bind(item.title)
    .bind(item.body)
    .bind(item.metadata)
    .bind(item.dedup_key)
    .bind(telmoni_shared::derive_shard_key(organization_id))
    .fetch_optional(tx.conn())
    .await
}

/// The text a `member_added` notice is left with once the person it named is
/// erased. The role survives, from `metadata`; the name does not.
const REDACTED_MEMBER_ADDED_TITLE: &str = "A former member joined the project";
const REDACTED_MEMBER_ADDED_BODY_BEFORE_ROLE: &str =
    "A former member accepted the invitation and is now ";
const REDACTED_MEMBER_ADDED_BODY_AFTER_ROLE: &str = " on the project.";

/// The text a `member_left` notice is left with once the person it named is
/// erased.
const REDACTED_MEMBER_LEFT_TITLE: &str = "A former member left the project";
const REDACTED_MEMBER_LEFT_BODY: &str = "A former member left the project.";

/// What a failed send's recorded answer becomes when the notice it carried
/// named a person who has since been erased.
const REDACTED_ERROR: &str =
    "the endpoint's answer was removed when the person it named was erased";

/// Rewrite every notice that names one person to name "a former member"
/// instead — on every feed, and on every delivery row still holding the
/// text — and forget who it named. The lane, because the rows span every
/// organization the person was ever announced to.
///
/// ⚠ **Errors on a kind it has no words for.** A row that still names the
/// person after every rewrite is a notice this function does not know how
/// to redact; leaving it would be the erasure silently not kept, so the
/// error stops the erasure, and the sweep retries until somebody adds the
/// words. Today `member_added` and `member_left` name a person.
pub async fn redact_person(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    user_id: &UserId,
) -> sqlx::Result<u64> {
    let feed = sqlx::query(
        "UPDATE notifications.feed
            SET title = $2,
                body = $3 || COALESCE(metadata->>'role', 'a member') || $4,
                subject_user_id = NULL
          WHERE subject_user_id = $1 AND kind = 'member_added'",
    )
    .bind(user_id)
    .bind(REDACTED_MEMBER_ADDED_TITLE)
    .bind(REDACTED_MEMBER_ADDED_BODY_BEFORE_ROLE)
    .bind(REDACTED_MEMBER_ADDED_BODY_AFTER_ROLE)
    .execute(tx.conn())
    .await?
    .rows_affected();
    let feed_left = sqlx::query(
        "UPDATE notifications.feed
            SET title = $2,
                body = $3,
                subject_user_id = NULL
          WHERE subject_user_id = $1 AND kind = 'member_left'",
    )
    .bind(user_id)
    .bind(REDACTED_MEMBER_LEFT_TITLE)
    .bind(REDACTED_MEMBER_LEFT_BODY)
    .execute(tx.conn())
    .await?
    .rows_affected();
    // A receiver may echo the notice back in its refusal, and the log keeps
    // that answer, so the error of every send of a named delivery goes too —
    // replaced, not emptied, since a failed send always says why. First, while
    // the deliveries still name the person.
    sqlx::query(
        "UPDATE notifications.delivery_attempts
            SET error = $2
          WHERE delivery_id IN (
              SELECT id FROM notifications.deliveries
               WHERE subject_user_id = $1 AND kind IN ('member_added', 'member_left')
          )
            AND outcome = 'failed'",
    )
    .bind(user_id)
    .bind(REDACTED_ERROR)
    .execute(tx.conn())
    .await?;
    // Delivered rows too: they keep their text for a resend from the delivery
    // log, which must never carry the name again. A queued one is sent as
    // rewritten. The role is not on this row, so the text goes without it.
    let deliveries = sqlx::query(
        "UPDATE notifications.deliveries
            SET subject = $2,
                body = CASE WHEN body = '' THEN '' ELSE $3 || 'a member' || $4 END,
                last_error = CASE WHEN last_error IS NULL THEN NULL ELSE $5 END,
                subject_user_id = NULL,
                updated_at = now()
          WHERE subject_user_id = $1 AND kind = 'member_added'",
    )
    .bind(user_id)
    .bind(REDACTED_MEMBER_ADDED_TITLE)
    .bind(REDACTED_MEMBER_ADDED_BODY_BEFORE_ROLE)
    .bind(REDACTED_MEMBER_ADDED_BODY_AFTER_ROLE)
    .bind(REDACTED_ERROR)
    .execute(tx.conn())
    .await?
    .rows_affected();
    let deliveries_left = sqlx::query(
        "UPDATE notifications.deliveries
            SET subject = $2,
                body = CASE WHEN body = '' THEN '' ELSE $3 END,
                last_error = CASE WHEN last_error IS NULL THEN NULL ELSE $4 END,
                subject_user_id = NULL,
                updated_at = now()
          WHERE subject_user_id = $1 AND kind = 'member_left'",
    )
    .bind(user_id)
    .bind(REDACTED_MEMBER_LEFT_TITLE)
    .bind(REDACTED_MEMBER_LEFT_BODY)
    .bind(REDACTED_ERROR)
    .execute(tx.conn())
    .await?
    .rows_affected();
    let unredacted: Option<String> = sqlx::query_scalar(
        "SELECT kind FROM notifications.feed WHERE subject_user_id = $1
          UNION ALL
         SELECT kind FROM notifications.deliveries WHERE subject_user_id = $1
          LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await?;
    if let Some(kind) = unredacted {
        return Err(sqlx::Error::Protocol(format!(
            "a `{kind}` notice names a person and `redact_person` has no words for it"
        )));
    }
    Ok(feed + feed_left + deliveries + deliveries_left)
}

/// The replayed emit's answer: the id the first emit minted for this key.
pub async fn feed_id_for_dedup_key<B: EmitBinding>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    project_id: Option<&ProjectId>,
    dedup_key: &str,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM notifications.feed
          WHERE organization_id = $1 AND dedup_key = $2 AND project_id IS NOT DISTINCT FROM $3",
    )
    .bind(organization_id)
    .bind(dedup_key)
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await
}

/// A project's feed, newest first.
pub async fn list_feed(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    limit: i64,
) -> sqlx::Result<Vec<FeedItem>> {
    sqlx::query_as::<_, FeedItem>(
        "SELECT id, kind, title, body, metadata, read_at, created_at
           FROM notifications.feed
          WHERE project_id = $1
          ORDER BY created_at DESC, id DESC
          LIMIT $2",
    )
    .bind(project_id)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// The total unread count, counted rather than taken from a capped page,
/// which would undercount a backlog.
pub async fn count_unread(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications.feed
          WHERE project_id = $1 AND read_at IS NULL",
    )
    .bind(project_id)
    .fetch_one(tx.conn())
    .await
}

/// Mark every unread row read, with the same predicate as the list and count.
pub async fn mark_all_read(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "UPDATE notifications.feed SET read_at = now()
          WHERE project_id = $1 AND read_at IS NULL",
    )
    .bind(project_id)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}

/// The organization's feed, newest first: the rows that name no project.
pub async fn list_organization_feed(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
    limit: i64,
) -> sqlx::Result<Vec<FeedItem>> {
    sqlx::query_as::<_, FeedItem>(
        "SELECT id, kind, title, body, metadata, read_at, created_at
           FROM notifications.feed
          WHERE project_id IS NULL AND organization_id = $1
          ORDER BY created_at DESC, id DESC
          LIMIT $2",
    )
    .bind(organization_id)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// The organization's unread count, counted for the same reason.
pub async fn count_organization_unread(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications.feed
          WHERE project_id IS NULL AND organization_id = $1 AND read_at IS NULL",
    )
    .bind(organization_id)
    .fetch_one(tx.conn())
    .await
}

/// Mark the organization's unread rows read, with the list's predicate.
pub async fn mark_organization_read(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "UPDATE notifications.feed SET read_at = now()
          WHERE project_id IS NULL AND organization_id = $1 AND read_at IS NULL",
    )
    .bind(organization_id)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}

/// Retention sweep: drop feed rows past their window.
pub async fn sweep_retention(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    feed_retention_days: i32,
    chunk: i64,
) -> sqlx::Result<u64> {
    let removed = sqlx::query(
        "DELETE FROM notifications.feed
          WHERE id IN (
              SELECT id FROM notifications.feed
               WHERE created_at < now() - make_interval(days => $1)
               LIMIT $2
          )",
    )
    .bind(feed_retention_days)
    .bind(chunk)
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(removed)
}

/// Whether this process may sweep: one advisory lock per sweep, held to the
/// transaction. Every replica runs the retention loop, and a second sweep
/// queued behind the first's row locks until its `lock_timeout`, failing the
/// round; a replica that finds the lock held skips its tick instead.
pub async fn try_retention_lock(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtextextended('notifications:retention', 0))",
    )
    .fetch_one(tx.conn())
    .await
}

/// Hard-delete every notifications row for one ORGANIZATION — this service's
/// step of the deletion cascade. One lane for every table: when this was two,
/// only one was ever called and the other's rows outlived the erasure.
pub async fn purge_organization(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    organization_id: &OrganizationId,
) -> sqlx::Result<u64> {
    let mut purged = 0u64;
    for sql in [
        "DELETE FROM notifications.connections WHERE organization_id = $1",
        "DELETE FROM notifications.oauth_states WHERE organization_id = $1",
        "DELETE FROM notifications.feed WHERE organization_id = $1",
    ] {
        purged += sqlx::query(sql)
            .bind(organization_id)
            .execute(tx.conn())
            .await?
            .rows_affected();
    }
    Ok(purged)
}

/// Hard-delete every notifications row for one PROJECT: what a project
/// handed to another organization leaves behind, and what a deleted one
/// does. The same three tables as the organization's purge; deliveries and
/// their attempts cascade from the connections.
pub async fn purge_project(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    project_id: &ProjectId,
) -> sqlx::Result<u64> {
    let mut purged = 0u64;
    for sql in [
        "DELETE FROM notifications.connections WHERE project_id = $1",
        "DELETE FROM notifications.oauth_states WHERE project_id = $1",
        "DELETE FROM notifications.feed WHERE project_id = $1",
    ] {
        purged += sqlx::query(sql)
            .bind(project_id)
            .execute(tx.conn())
            .await?
            .rows_affected();
    }
    Ok(purged)
}

/// A connection as served to the BFF — **never the target or the token.**
#[derive(Debug, Serialize, FromRow)]
pub struct ConnectionSummary {
    pub id: Uuid,
    pub provider: String,
    pub external_workspace_id: String,
    pub external_workspace_name: Option<String>,
    pub channel_id: String,
    pub channel_name: String,
    pub status: String,
    pub last_error: Option<String>,
    pub last_delivery_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// The notice kinds a webhook receives; `None` is every kind.
    pub event_kinds: Option<Vec<String>>,
    /// Until when a rotated secret still signs; `None` outside an overlap.
    pub previous_secret_expires_at: Option<DateTime<Utc>>,
}

const SUMMARY_COLUMNS: &str = "id, provider, external_workspace_id, external_workspace_name, \
     channel_id, channel_name, status, last_error, last_delivery_at, created_at, event_kinds, \
     CASE WHEN prior_token_expires_at > now() THEN prior_token_expires_at END \
     AS previous_secret_expires_at";

/// A project's connections, newest first, every status.
pub async fn list_connections(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<ConnectionSummary>> {
    sqlx::query_as::<_, ConnectionSummary>(&format!(
        "SELECT {SUMMARY_COLUMNS} FROM notifications.connections
          WHERE project_id = $1
          ORDER BY created_at DESC, id DESC"
    ))
    .bind(project_id)
    .fetch_all(tx.conn())
    .await
}

/// A connection with its sealed credentials, for the moment of use. `Debug`
/// prints ciphertext lengths and nothing else.
#[derive(FromRow)]
pub struct SealedConnection {
    pub id: Uuid,
    pub project_id: String,
    pub organization_id: String,
    pub provider: String,
    pub status: String,
    pub external_workspace_id: String,
    pub external_workspace_name: Option<String>,
    pub channel_name: String,
    pub target_ciphertext: Vec<u8>,
    pub target_nonce: Vec<u8>,
    pub key_version: i16,
    /// The row's data key, wrapped by the KEK `key_version` names.
    pub wrapped_dek: Vec<u8>,
    pub token_ciphertext: Option<Vec<u8>>,
    pub token_nonce: Option<Vec<u8>>,
    pub prior_token_ciphertext: Option<Vec<u8>>,
    pub prior_token_nonce: Option<Vec<u8>>,
    pub prior_token_expires_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for SealedConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedConnection")
            .field("id", &self.id)
            .field("project_id", &self.project_id)
            .field("provider", &self.provider)
            .field("status", &self.status)
            .field("external_workspace_id", &self.external_workspace_id)
            .field("channel_name", &self.channel_name)
            .field("target", &self.target_ciphertext.len())
            .field("wrapped_dek", &self.wrapped_dek.len())
            .field("token", &self.token_ciphertext.as_ref().map(Vec::len))
            .field("prior_token_expires_at", &self.prior_token_expires_at)
            .finish()
    }
}

impl SealedConnection {
    /// The sealed target: a vendor's webhook URL or the customer's endpoint.
    #[must_use]
    pub fn target(&self) -> Sealed {
        Sealed {
            ciphertext: self.target_ciphertext.clone(),
            nonce: self.target_nonce.clone(),
        }
    }

    /// The sealed token: Slack's bot token or the webhook's signing secret;
    /// `None` for Discord.
    #[must_use]
    pub fn token(&self) -> Option<Sealed> {
        match (&self.token_ciphertext, &self.token_nonce) {
            (Some(ciphertext), Some(nonce)) => Some(Sealed {
                ciphertext: ciphertext.clone(),
                nonce: nonce.clone(),
            }),
            _ => None,
        }
    }

    /// A webhook's rotated secret while it still signs, as of `now`. Past its
    /// expiry it is treated as gone even before the sweep clears it, so the
    /// overlap ends when the owner was told it would.
    #[must_use]
    pub fn prior_token(&self, now: DateTime<Utc>) -> Option<Sealed> {
        match (
            &self.prior_token_ciphertext,
            &self.prior_token_nonce,
            self.prior_token_expires_at,
        ) {
            (Some(ciphertext), Some(nonce), Some(until)) if until > now => Some(Sealed {
                ciphertext: ciphertext.clone(),
                nonce: nonce.clone(),
            }),
            _ => None,
        }
    }
}

const SEALED_COLUMNS: &str = "id, project_id, organization_id, provider, status, \
     external_workspace_id, external_workspace_name, channel_name, \
     target_ciphertext, target_nonce, key_version, wrapped_dek, \
     token_ciphertext, token_nonce, \
     prior_token_ciphertext, prior_token_nonce, prior_token_expires_at";

/// One connection by id, sealed, in the project named: the predicate is what
/// keeps tenants apart where everything connects as one database user and
/// row-level security has nothing to say.
pub async fn sealed_connection<B: Binding>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<SealedConnection>> {
    sqlx::query_as::<_, SealedConnection>(&format!(
        "SELECT {SEALED_COLUMNS} FROM notifications.connections WHERE project_id = $1 AND id = $2"
    ))
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await
}

/// The connections a leased BATCH points at, sealed, read in one round trip.
pub async fn sealed_connections(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    ids: &[Uuid],
) -> sqlx::Result<Vec<SealedConnection>> {
    sqlx::query_as::<_, SealedConnection>(&format!(
        "SELECT {SEALED_COLUMNS} FROM notifications.connections WHERE id = ANY($1)"
    ))
    .bind(ids)
    .fetch_all(tx.conn())
    .await
}

/// Every connection an organization holds, sealed, so the purge can tear each
/// down upstream before the rows go.
pub async fn sealed_connections_for_organization(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    organization_id: &OrganizationId,
) -> sqlx::Result<Vec<SealedConnection>> {
    sqlx::query_as::<_, SealedConnection>(&format!(
        "SELECT {SEALED_COLUMNS} FROM notifications.connections
          WHERE organization_id = $1
          ORDER BY id"
    ))
    .bind(organization_id)
    .fetch_all(tx.conn())
    .await
}

/// Every connection a project holds, sealed, so the project purge can tear
/// each down upstream before the rows go.
pub async fn sealed_connections_for_project(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<SealedConnection>> {
    sqlx::query_as::<_, SealedConnection>(&format!(
        "SELECT {SEALED_COLUMNS} FROM notifications.connections
          WHERE project_id = $1
          ORDER BY id"
    ))
    .bind(project_id)
    .fetch_all(tx.conn())
    .await
}

/// The row a reconnect would land on, locked. Read before sealing rather than
/// `ON CONFLICT`, because the ciphertext binds the ROW's id, which must be
/// known first; a double press loses to the unique index with a 409.
pub async fn existing_connection_id<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    provider: Provider,
    external_workspace_id: &str,
    channel_id: &str,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM notifications.connections
          WHERE project_id = $1 AND provider = $2
            AND external_workspace_id = $3 AND channel_id = $4
          FOR UPDATE",
    )
    .bind(project_id)
    .bind(provider.as_str())
    .bind(external_workspace_id)
    .bind(channel_id)
    .fetch_optional(tx.conn())
    .await
}

/// One install, sealed and ready to store.
#[derive(Debug)]
pub struct NewConnection<'a> {
    /// The row's id — chosen before sealing, because it is the AAD.
    pub id: Uuid,
    pub project_id: &'a ProjectId,
    pub organization_id: &'a OrganizationId,
    pub provider: Provider,
    pub external_workspace_id: &'a str,
    pub external_workspace_name: Option<&'a str>,
    pub channel_id: &'a str,
    pub channel_name: &'a str,
    pub target: &'a Sealed,
    pub key_version: i16,
    /// The data key the fields are sealed under, wrapped with this row's id as
    /// associated data.
    pub wrapped_dek: &'a [u8],
    pub token: Option<&'a Sealed>,
    pub scopes: &'a str,
    /// A webhook's chosen notice kinds; `None` is every kind, and always
    /// `None` for a vendor channel.
    pub event_kinds: Option<&'a [String]>,
    pub installed_by: &'a UserId,
}

/// A first install.
pub async fn insert_connection<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    c: &NewConnection<'_>,
) -> sqlx::Result<ConnectionSummary> {
    sqlx::query_as::<_, ConnectionSummary>(&format!(
        "INSERT INTO notifications.connections
             (id, project_id, organization_id, provider, external_workspace_id,
              external_workspace_name, channel_id, channel_name,
              target_ciphertext, target_nonce, key_version, wrapped_dek,
              token_ciphertext, token_nonce, scopes, installed_by, shard_key, event_kinds)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                 $18)
         RETURNING {SUMMARY_COLUMNS}"
    ))
    .bind(c.id)
    .bind(c.project_id)
    .bind(c.organization_id)
    .bind(c.provider.as_str())
    .bind(c.external_workspace_id)
    .bind(c.external_workspace_name)
    .bind(c.channel_id)
    .bind(c.channel_name)
    .bind(&c.target.ciphertext)
    .bind(&c.target.nonce)
    .bind(c.key_version)
    .bind(c.wrapped_dek)
    .bind(c.token.map(|t| t.ciphertext.as_slice()))
    .bind(c.token.map(|t| t.nonce.as_slice()))
    .bind(c.scopes)
    .bind(c.installed_by)
    .bind(telmoni_shared::derive_shard_key(c.organization_id))
    .bind(c.event_kinds)
    .fetch_one(tx.conn())
    .await
}

/// Change which notice kinds a webhook receives; `None` is every kind. Only a
/// webhook chooses: a vendor channel is refused by the `provider` guard here
/// and by `connections_event_kinds_webhook_check` behind it.
pub async fn set_event_kinds(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    id: Uuid,
    event_kinds: Option<&[String]>,
) -> sqlx::Result<Option<ConnectionSummary>> {
    sqlx::query_as::<_, ConnectionSummary>(&format!(
        "UPDATE notifications.connections
            SET event_kinds = $4
          WHERE id = $1 AND project_id = $2 AND provider = $3
      RETURNING {SUMMARY_COLUMNS}"
    ))
    .bind(id)
    .bind(project_id)
    .bind(Provider::Webhook.as_str())
    .bind(event_kinds)
    .fetch_optional(tx.conn())
    .await
}

/// A reconnect: the same channel, a fresh grant. The row comes back `active`.
pub async fn reconnect_connection(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    c: &NewConnection<'_>,
) -> sqlx::Result<ConnectionSummary> {
    sqlx::query_as::<_, ConnectionSummary>(&format!(
        "UPDATE notifications.connections
            SET external_workspace_name = $2, channel_name = $3,
                target_ciphertext = $4, target_nonce = $5, key_version = $6,
                wrapped_dek = $7, token_ciphertext = $8, token_nonce = $9,
                scopes = $10, installed_by = $11,
                status = 'active', last_error = NULL, revoked_at = NULL,
                consecutive_failures = 0
          WHERE id = $1
      RETURNING {SUMMARY_COLUMNS}"
    ))
    .bind(c.id)
    .bind(c.external_workspace_name)
    .bind(c.channel_name)
    .bind(&c.target.ciphertext)
    .bind(&c.target.nonce)
    .bind(c.key_version)
    .bind(c.wrapped_dek)
    .bind(c.token.map(|t| t.ciphertext.as_slice()))
    .bind(c.token.map(|t| t.nonce.as_slice()))
    .bind(c.scopes)
    .bind(c.installed_by)
    .fetch_one(tx.conn())
    .await
}

/// Remove one connection; its queue cascades. `None` when there is no such row.
pub async fn delete_connection(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(
        "DELETE FROM notifications.connections WHERE project_id = $1 AND id = $2 RETURNING id",
    )
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await
}

/// How many connections, platform-wide, still point at one vendor workspace.
pub async fn count_workspace_connections(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    provider: Provider,
    external_workspace_id: &str,
) -> sqlx::Result<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications.connections
          WHERE provider = $1 AND external_workspace_id = $2",
    )
    .bind(provider.as_str())
    .bind(external_workspace_id)
    .fetch_one(tx.conn())
    .await
}

/// Like [`count_workspace_connections`], but outside one organization: does
/// anybody else still use this workspace?
pub async fn count_workspace_connections_outside(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    provider: Provider,
    external_workspace_id: &str,
    organization_id: &OrganizationId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications.connections
          WHERE provider = $1 AND external_workspace_id = $2 AND organization_id <> $3",
    )
    .bind(provider.as_str())
    .bind(external_workspace_id)
    .bind(organization_id)
    .fetch_one(tx.conn())
    .await
}

/// Like [`count_workspace_connections`], but outside one project: does any
/// other project, in this organization or another, still use this workspace?
pub async fn count_workspace_connections_outside_project(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    provider: Provider,
    external_workspace_id: &str,
    project_id: &ProjectId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications.connections
          WHERE provider = $1 AND external_workspace_id = $2 AND project_id <> $3",
    )
    .bind(provider.as_str())
    .bind(external_workspace_id)
    .bind(project_id)
    .fetch_one(tx.conn())
    .await
}

/// What a connection looked like when it was retired, enough to notify and
/// audit.
#[derive(Debug, FromRow)]
pub struct RetiredConnection {
    pub id: Uuid,
    pub project_id: String,
    pub organization_id: String,
    pub provider: String,
    pub external_workspace_id: String,
    pub external_workspace_name: Option<String>,
    pub channel_name: String,
}

const RETIRED_COLUMNS: &str = "id, project_id, organization_id, provider, external_workspace_id, \
     external_workspace_name, channel_name";

/// Move one ACTIVE connection to `revoked` or `errored`. `None` means a
/// concurrent attempt got there first, so no second notice is written.
pub async fn retire_connection(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    id: Uuid,
    status: &str,
    reason: &str,
) -> sqlx::Result<Option<RetiredConnection>> {
    sqlx::query_as::<_, RetiredConnection>(&format!(
        "UPDATE notifications.connections
            SET status = $2, last_error = $3,
                revoked_at = CASE WHEN $2 = 'revoked' THEN now() ELSE revoked_at END
          WHERE id = $1 AND status = 'active'
      RETURNING {RETIRED_COLUMNS}"
    ))
    .bind(id)
    .bind(status)
    .bind(telmoni_shared::text::truncate_on_char_boundary(
        reason,
        LAST_ERROR_MAX,
    ))
    .fetch_optional(tx.conn())
    .await
}

/// Revoke every connection into one vendor workspace, across every tenant —
/// the answer to `app_uninstalled`. Idempotent, as Slack's redeliveries need.
pub async fn revoke_workspace(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    provider: Provider,
    external_workspace_id: &str,
    reason: &str,
) -> sqlx::Result<Vec<RetiredConnection>> {
    sqlx::query_as::<_, RetiredConnection>(&format!(
        "UPDATE notifications.connections
            SET status = 'revoked', last_error = $3, revoked_at = now()
          WHERE provider = $1 AND external_workspace_id = $2 AND status <> 'revoked'
      RETURNING {RETIRED_COLUMNS}"
    ))
    .bind(provider.as_str())
    .bind(external_workspace_id)
    .bind(telmoni_shared::text::truncate_on_char_boundary(
        reason,
        LAST_ERROR_MAX,
    ))
    .fetch_all(tx.conn())
    .await
}

/// Stamp a successful send and reset the breaker: a send that landed ends a
/// run of failures.
pub async fn touch_connection(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    id: Uuid,
) -> sqlx::Result<()> {
    touch_connections(tx, &[id]).await
}

/// The same, for every connection a batch delivered to, in one round trip.
pub async fn touch_connections(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    ids: &[Uuid],
) -> sqlx::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE notifications.connections
            SET last_delivery_at = now(), consecutive_failures = 0
          WHERE id = ANY($1)",
    )
    .bind(ids)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Add a batch's failures to each connection's breaker and return the new
/// counts, for the caller to compare with the connector's limit.
pub async fn record_consecutive_failures(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    tallies: &[(Uuid, i32)],
) -> sqlx::Result<Vec<(Uuid, i32)>> {
    if tallies.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = tallies.iter().map(|&(id, _)| id).collect();
    let counts: Vec<i32> = tallies.iter().map(|&(_, n)| n).collect();
    let rows: Vec<(Uuid, i32)> = sqlx::query_as(
        "UPDATE notifications.connections c
            SET consecutive_failures = c.consecutive_failures + f.n
           FROM unnest($1::uuid[], $2::int4[]) AS f(id, n)
          WHERE c.id = f.id AND c.status = 'active'
      RETURNING c.id, c.consecutive_failures",
    )
    .bind(&ids)
    .bind(&counts)
    .fetch_all(tx.conn())
    .await?;
    Ok(rows)
}

/// A webhook's new signing secret, sealed under the row's existing data key;
/// the row comes back `active`, since a rotation is this lane's Reconnect.
/// Conditional on the secret the caller read, so of two racing rotations only
/// one answers a secret that will verify.
///
/// With `keep_prior_hours` above zero the replaced secret keeps signing that
/// long beside the new one; at zero it stops now. A rotation inside an overlap
/// keeps only the secret it replaced, so at most two ever sign.
pub async fn reseal_webhook_secret(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    id: Uuid,
    replacing: &Sealed,
    token: &Sealed,
    keep_prior_hours: i32,
) -> sqlx::Result<Option<ConnectionSummary>> {
    sqlx::query_as::<_, ConnectionSummary>(&format!(
        "UPDATE notifications.connections
            SET token_ciphertext = $5, token_nonce = $6,
                prior_token_ciphertext = CASE WHEN $8 > 0 THEN $4 END,
                prior_token_nonce = CASE WHEN $8 > 0 THEN $7::bytea END,
                prior_token_expires_at = CASE WHEN $8 > 0
                    THEN now() + make_interval(hours => $8) END,
                status = 'active', last_error = NULL, revoked_at = NULL,
                consecutive_failures = 0
          WHERE id = $1 AND project_id = $2 AND provider = $3 AND token_ciphertext = $4
      RETURNING {SUMMARY_COLUMNS}"
    ))
    .bind(id)
    .bind(project_id)
    .bind(Provider::Webhook.as_str())
    .bind(&replacing.ciphertext)
    .bind(&token.ciphertext)
    .bind(&token.nonce)
    .bind(&replacing.nonce)
    .bind(keep_prior_hours)
    .fetch_optional(tx.conn())
    .await
}

/// Most of a far end's failure we keep on the row.
const LAST_ERROR_MAX: usize = 500;

/// One queued send's content; the connection decides where.
#[derive(Debug, Clone, Copy)]
pub struct NewDelivery<'a> {
    pub connection_id: Uuid,
    pub project_id: &'a str,
    pub organization_id: &'a OrganizationId,
    pub kind: &'a str,
    pub subject: &'a str,
    pub body: &'a str,
    /// The person the text names, as on the feed row it came from.
    pub subject_user_id: Option<&'a str>,
}

/// Enqueue one delivery to a connection the caller names — the one-row form a
/// fixture uses; a notice's fan-out is [`enqueue_deliveries`].
pub async fn enqueue_delivery<B: Binding>(
    tx: &mut Scoped<'_, B>,
    d: &NewDelivery<'_>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO notifications.deliveries
            (id, connection_id, project_id, organization_id, kind, subject, body,
             subject_user_id, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(d.connection_id)
    .bind(d.project_id)
    .bind(d.organization_id)
    .bind(d.kind)
    .bind(d.subject)
    .bind(d.body)
    .bind(d.subject_user_id)
    .bind(telmoni_shared::derive_shard_key(d.organization_id))
    .fetch_one(tx.conn())
    .await
}

/// Queue one delivery per active connection a notice reaches — the project's,
/// or every project's in the organization when `project_id` is `None`, less
/// any webhook that did not choose this kind — in one `INSERT`, answering the
/// ids for the inline first attempt. The targets
/// are read first because each row's id is minted here, time-ordered, and
/// Postgres 17 cannot mint one per row. Each row copies its connection's
/// tenant keys, so the composite foreign key and every policy's `WITH CHECK`
/// hold by construction under any binding. Known wart: the organization-wide
/// fan-out does not dedupe on channel.
pub async fn enqueue_deliveries<B: Binding>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    project_id: Option<&ProjectId>,
    kind: &str,
    subject: &str,
    body: &str,
    subject_user_id: Option<&UserId>,
) -> sqlx::Result<Vec<Uuid>> {
    let targets: Vec<(Uuid, ProjectId)> = sqlx::query_as(
        "SELECT id, project_id
           FROM notifications.connections
          WHERE organization_id = $1
            AND ($2::text IS NULL OR project_id = $2)
            AND status = 'active'
            AND (event_kinds IS NULL OR $3 = ANY (event_kinds))
          ORDER BY id",
    )
    .bind(organization_id)
    .bind(project_id)
    .bind(kind)
    .fetch_all(tx.conn())
    .await?;
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = targets.iter().map(|_| Uuid::now_v7()).collect();
    let connection_ids: Vec<Uuid> = targets.iter().map(|(id, _)| *id).collect();
    let project_ids: Vec<ProjectId> = targets.into_iter().map(|(_, p)| p).collect();
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO notifications.deliveries
            (id, connection_id, project_id, organization_id, kind, subject, body,
             subject_user_id, shard_key)
         SELECT t.id, t.connection_id, t.project_id, $4::text, $5::text, $6::text,
                $7::text, $8::text, $9::uuid
           FROM unnest($1::uuid[], $2::uuid[], $3::text[]) AS t(id, connection_id, project_id)
         RETURNING id",
    )
    .bind(&ids)
    .bind(&connection_ids)
    .bind(&project_ids)
    .bind(organization_id)
    .bind(kind)
    .bind(subject)
    .bind(body)
    .bind(subject_user_id)
    .bind(telmoni_shared::derive_shard_key(organization_id))
    .fetch_all(tx.conn())
    .await
}

/// A leased delivery: everything but the URL, which is opened at the send.
#[derive(Debug, FromRow)]
pub struct LeasedDelivery {
    pub id: Uuid,
    pub connection_id: Uuid,
    pub kind: String,
    pub subject: String,
    pub body: String,
    pub attempts: i32,
}

const LEASED_COLUMNS: &str = "d.id, d.connection_id, d.kind, d.subject, d.body, d.attempts";

/// Lease up to `limit` eligible deliveries, `SKIP LOCKED`.
///
/// ⚠ **No `ORDER BY`**: the planner would sort every eligible row before the
/// limit (measured: a 3MB sort on a 40,000-row backlog, growing with it). The
/// cost is physical-order draining, so sustained overload starves some rows.
/// **No `EXISTS` on the connection being active**: it drove the plan through
/// the connection's whole history; `settle` re-checks after the lease instead.
pub async fn lease_deliveries(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    limit: i64,
    lease_secs: i64,
) -> sqlx::Result<Vec<LeasedDelivery>> {
    sqlx::query_as::<_, LeasedDelivery>(&format!(
        "WITH candidate AS (
            SELECT d.id FROM notifications.deliveries d
             WHERE d.status = 'pending'
               AND d.next_attempt_at <= now()
               AND (d.lease_until IS NULL OR d.lease_until < now())
             LIMIT $1
             FOR UPDATE SKIP LOCKED
         )
         UPDATE notifications.deliveries d
            SET attempts = d.attempts + 1,
                lease_until = now() + make_interval(secs => $2::bigint),
                updated_at = now()
           FROM candidate c
          WHERE d.id = c.id
      RETURNING {LEASED_COLUMNS}"
    ))
    .bind(limit)
    .bind(lease_secs)
    .fetch_all(tx.conn())
    .await
}

/// Lease ONE named row for the inline first attempt; `None` means the loop has
/// it. It keeps the `active` check, which costs nothing on a primary-key read.
pub async fn lease_delivery(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    id: Uuid,
    lease_secs: i64,
) -> sqlx::Result<Option<LeasedDelivery>> {
    sqlx::query_as::<_, LeasedDelivery>(&format!(
        "UPDATE notifications.deliveries d
            SET attempts = d.attempts + 1,
                lease_until = now() + make_interval(secs => $2::bigint),
                updated_at = now()
          WHERE d.id = $1
            AND d.status = 'pending'
            AND (d.lease_until IS NULL OR d.lease_until < now())
            AND EXISTS (
                SELECT 1 FROM notifications.connections c
                 WHERE c.id = d.connection_id AND c.status = 'active'
            )
      RETURNING {LEASED_COLUMNS}"
    ))
    .bind(id)
    .bind(lease_secs)
    .fetch_optional(tx.conn())
    .await
}

/// Terminal success for a batch, in one statement. A transaction per row held
/// a five-connection pool two hundred times a batch. The body stays until the
/// retention sweep, so a resend from the delivery log carries the same text.
pub async fn mark_delivered(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    ids: &[Uuid],
) -> sqlx::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE notifications.deliveries
            SET status = 'delivered', lease_until = NULL, last_error = NULL, updated_at = now()
          WHERE id = ANY($1)",
    )
    .bind(ids)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// A batch of failed attempts: back to `pending` behind a backoff while
/// attempts remain, else terminal `failed`. `max_attempts` of 0 is terminal now.
#[derive(Debug, Clone)]
pub struct FailedAttempt {
    pub id: Uuid,
    pub error: String,
    pub max_attempts: i32,
    pub backoff_secs: i64,
}

/// Record every failed attempt in a batch, one statement.
pub async fn fail_deliveries(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    failures: &[FailedAttempt],
) -> sqlx::Result<()> {
    if failures.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = failures.iter().map(|f| f.id).collect();
    let errors: Vec<String> = failures
        .iter()
        .map(|f| {
            telmoni_shared::text::truncate_on_char_boundary(&f.error, LAST_ERROR_MAX).to_owned()
        })
        .collect();
    let budgets: Vec<i32> = failures.iter().map(|f| f.max_attempts).collect();
    let waits: Vec<i64> = failures.iter().map(|f| f.backoff_secs).collect();
    sqlx::query(
        "UPDATE notifications.deliveries d
            SET status = CASE WHEN d.attempts >= f.max_attempts THEN 'failed' ELSE 'pending' END,
                next_attempt_at = CASE WHEN d.attempts >= f.max_attempts
                    THEN d.next_attempt_at
                    ELSE now() + make_interval(secs => f.backoff_secs::double precision) END,
                lease_until = NULL,
                last_error = f.error,
                updated_at = now()
           FROM unnest($1::uuid[], $2::text[], $3::int4[], $4::int8[])
                AS f(id, error, max_attempts, backoff_secs)
          WHERE d.id = f.id AND d.status = 'pending'",
    )
    .bind(&ids)
    .bind(&errors)
    .bind(&budgets)
    .bind(&waits)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Hand leased deliveries back untouched: the attempt is returned and the gate
/// left where it was. For the failure that is nobody's — the key could not be
/// reached — and the reason a KMS outage cannot burn a row's budget.
pub async fn unlease_deliveries(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    ids: &[Uuid],
    error: &str,
) -> sqlx::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE notifications.deliveries
            SET attempts = GREATEST(attempts - 1, 0),
                lease_until = NULL,
                last_error = $2,
                updated_at = now()
          WHERE id = ANY($1) AND status = 'pending'",
    )
    .bind(ids)
    .bind(telmoni_shared::text::truncate_on_char_boundary(
        error,
        LAST_ERROR_MAX,
    ))
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Fail every queued delivery bound for a just-retired or removed connection.
pub async fn fail_pending_for_connection(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    connection_id: Uuid,
    reason: &str,
) -> sqlx::Result<u64> {
    fail_pending_for_connections(tx, &[connection_id], reason).await
}

/// The same, for every connection a workspace revocation retired, in one
/// round trip.
pub async fn fail_pending_for_connections(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    connection_ids: &[Uuid],
    reason: &str,
) -> sqlx::Result<u64> {
    if connection_ids.is_empty() {
        return Ok(0);
    }
    let done = sqlx::query(
        "UPDATE notifications.deliveries
            SET status = 'failed', lease_until = NULL, last_error = $2, updated_at = now()
          WHERE connection_id = ANY($1) AND status = 'pending'",
    )
    .bind(connection_ids)
    .bind(telmoni_shared::text::truncate_on_char_boundary(
        reason,
        LAST_ERROR_MAX,
    ))
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}

/// Who started an attempt: the queue, or an owner or admin resending from the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptTrigger {
    Scheduled,
    Manual,
}

impl AttemptTrigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Manual => "manual",
        }
    }
}

/// One send, as the delivery log keeps it. `error` is `None` exactly when it
/// was delivered.
#[derive(Debug, Clone)]
pub struct NewAttempt {
    pub delivery_id: Uuid,
    pub trigger: AttemptTrigger,
    pub status_code: Option<u16>,
    pub duration_ms: i32,
    pub error: Option<String>,
}

/// Record a batch's sends in one statement. The tenant keys and shard are the
/// delivery row's, so the composite foreign key holds by construction; a
/// delivery already gone — its connection disconnected mid-send — records
/// nothing.
///
/// ⚠ **`FOR KEY SHARE` on the deliveries joined.** The join reads the
/// statement's snapshot but the foreign key checks the latest rows, so a
/// delivery deleted between the two failed the whole INSERT — and with it the
/// batch's bookkeeping transaction, every tenant's rows in it re-sent when
/// their leases lapsed. The lock waits out a concurrent delete and then drops
/// the gone row, so the parent is always there.
pub async fn record_attempts(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    attempts: &[NewAttempt],
) -> sqlx::Result<()> {
    if attempts.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = attempts.iter().map(|_| Uuid::now_v7()).collect();
    let deliveries: Vec<Uuid> = attempts.iter().map(|a| a.delivery_id).collect();
    let triggers: Vec<&str> = attempts.iter().map(|a| a.trigger.as_str()).collect();
    let outcomes: Vec<&str> = attempts
        .iter()
        .map(|a| {
            if a.error.is_none() {
                "delivered"
            } else {
                "failed"
            }
        })
        .collect();
    let codes: Vec<Option<i32>> = attempts
        .iter()
        .map(|a| a.status_code.map(i32::from))
        .collect();
    let durations: Vec<i32> = attempts.iter().map(|a| a.duration_ms.max(0)).collect();
    let errors: Vec<Option<String>> = attempts
        .iter()
        .map(|a| {
            a.error.as_deref().map(|e| {
                telmoni_shared::text::truncate_on_char_boundary(e, LAST_ERROR_MAX).to_owned()
            })
        })
        .collect();
    sqlx::query(
        "INSERT INTO notifications.delivery_attempts
            (id, delivery_id, project_id, organization_id, trigger, outcome, status_code,
             duration_ms, error, shard_key)
         SELECT a.id, d.id, d.project_id, d.organization_id, a.trigger, a.outcome,
                a.status_code, a.duration_ms, a.error, d.shard_key
           FROM unnest($1::uuid[], $2::uuid[], $3::text[], $4::text[], $5::int4[],
                       $6::int4[], $7::text[])
                AS a(id, delivery_id, trigger, outcome, status_code, duration_ms, error)
           JOIN (SELECT id, project_id, organization_id, shard_key
                   FROM notifications.deliveries
                  WHERE id = ANY ($2)
                    FOR KEY SHARE) d ON d.id = a.delivery_id",
    )
    .bind(&ids)
    .bind(&deliveries)
    .bind(&triggers)
    .bind(&outcomes)
    .bind(&codes)
    .bind(&durations)
    .bind(&errors)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Whether the project holds this connection, without reading its sealed
/// fields: the log needs to know the row is there, not what it guards.
pub async fn connection_exists(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM notifications.connections
             WHERE project_id = $1 AND id = $2
         )",
    )
    .bind(project_id)
    .bind(id)
    .fetch_one(tx.conn())
    .await
}

/// One delivery as the log lists it, with its sends oldest first.
#[derive(Debug, Serialize, FromRow)]
pub struct DeliveryLogEntry {
    pub id: Uuid,
    pub kind: String,
    pub subject: String,
    pub body: String,
    pub status: String,
    pub next_attempt_at: DateTime<Utc>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[sqlx(skip)]
    pub attempts: Vec<AttemptLogEntry>,
}

/// One send, as the log lists it.
#[derive(Debug, Serialize, FromRow)]
pub struct AttemptLogEntry {
    pub id: Uuid,
    #[serde(skip)]
    pub delivery_id: Uuid,
    pub trigger: String,
    pub outcome: String,
    pub status_code: Option<i32>,
    pub duration_ms: i32,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A page of one connection's deliveries, newest first, each with its sends.
/// `before` is the cursor: the oldest id of the page already shown. Ids are
/// v7, so id order is time order and one index serves the walk. The first
/// page's bound is the largest id rather than an `IS NULL` branch: under a
/// generic plan the branch stops being an index bound, and every page then
/// walked every newer entry.
pub async fn delivery_log(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    connection_id: Uuid,
    before: Option<Uuid>,
    limit: i64,
) -> sqlx::Result<Vec<DeliveryLogEntry>> {
    let mut page = sqlx::query_as::<_, DeliveryLogEntry>(
        "SELECT id, kind, subject, body, status, next_attempt_at, last_error, created_at,
                updated_at
           FROM notifications.deliveries
          WHERE project_id = $1 AND connection_id = $2
            AND id < COALESCE($3, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid)
          ORDER BY id DESC
          LIMIT $4",
    )
    .bind(project_id)
    .bind(connection_id)
    .bind(before)
    .bind(limit)
    .fetch_all(tx.conn())
    .await?;
    if page.is_empty() {
        return Ok(page);
    }
    let ids: Vec<Uuid> = page.iter().map(|d| d.id).collect();
    let attempts = sqlx::query_as::<_, AttemptLogEntry>(
        "SELECT id, delivery_id, trigger, outcome, status_code, duration_ms, error, created_at
           FROM notifications.delivery_attempts
          WHERE project_id = $1 AND delivery_id = ANY ($2)
          ORDER BY delivery_id, id",
    )
    .bind(project_id)
    .bind(&ids)
    .fetch_all(tx.conn())
    .await?;
    let mut by_delivery: std::collections::HashMap<Uuid, Vec<AttemptLogEntry>> =
        std::collections::HashMap::with_capacity(page.len());
    for attempt in attempts {
        by_delivery
            .entry(attempt.delivery_id)
            .or_default()
            .push(attempt);
    }
    for delivery in &mut page {
        delivery.attempts = by_delivery.remove(&delivery.id).unwrap_or_default();
    }
    Ok(page)
}

/// What stops a manual resend, when it is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unclaimed {
    /// No such delivery on this connection in this project.
    Missing,
    /// An attempt holds the row right now, the queue's or another resend's.
    Busy,
}

/// Take one delivery for a manual resend, whatever its status, by leasing it:
/// the loop leases only unleased pending rows, so the two never send the same
/// row at once. The attempt count is untouched — a resend is not the queue's
/// retry and spends none of its budget.
pub async fn claim_for_redelivery(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    connection_id: Uuid,
    delivery_id: Uuid,
    lease_secs: i64,
) -> sqlx::Result<Result<Claimed, Unclaimed>> {
    let claimed = sqlx::query_as::<_, Claimed>(
        "UPDATE notifications.deliveries d
            SET lease_until = now() + make_interval(secs => $4::bigint)
          WHERE d.id = $3 AND d.project_id = $1 AND d.connection_id = $2
            AND (d.lease_until IS NULL OR d.lease_until < now())
      RETURNING d.id, d.connection_id, d.kind, d.subject, d.body, d.attempts, d.lease_until",
    )
    .bind(project_id)
    .bind(connection_id)
    .bind(delivery_id)
    .bind(lease_secs)
    .fetch_optional(tx.conn())
    .await?;
    if let Some(claimed) = claimed {
        return Ok(Ok(claimed));
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM notifications.deliveries
             WHERE id = $3 AND project_id = $1 AND connection_id = $2
         )",
    )
    .bind(project_id)
    .bind(connection_id)
    .bind(delivery_id)
    .fetch_one(tx.conn())
    .await?;
    Ok(Err(if exists {
        Unclaimed::Busy
    } else {
        Unclaimed::Missing
    }))
}

/// A delivery taken for a manual resend, and the lease that proves it is ours.
#[derive(Debug, FromRow)]
pub struct Claimed {
    #[sqlx(flatten)]
    pub delivery: LeasedDelivery,
    pub lease_until: DateTime<Utc>,
}

/// Settle a manual resend and release its claim. Delivered ends the row as
/// delivered, whatever it was. A failure leaves the status alone — a pending
/// row keeps its place in the retry schedule, a failed or delivered one stays
/// as it was — and records the answer.
///
/// ⚠ **Only while the lease is still ours.** A claim that outlived its lease
/// may have been taken by the loop, and settling it then would clear the
/// loop's lease and let a third send start. `false` means it was lost: the
/// attempt happened and is logged, but the row is someone else's now.
///
/// `updated_at` moves only when the status does: it is the retention clock,
/// and a resend that changed nothing must not buy the row another 30 days.
/// Keyed on the project the request holds, as well as the id, though the lane
/// would reach any row: a wrong id then matches nothing.
pub async fn finish_redelivery(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    project_id: &ProjectId,
    claim: &Claimed,
    error: Option<&str>,
) -> sqlx::Result<bool> {
    let settled = sqlx::query(
        "UPDATE notifications.deliveries
            SET status = CASE WHEN $4::text IS NULL THEN 'delivered' ELSE status END,
                updated_at = CASE WHEN $4::text IS NULL AND status <> 'delivered'
                    THEN now() ELSE updated_at END,
                last_error = $4,
                lease_until = NULL
          WHERE id = $1 AND project_id = $2 AND lease_until = $3",
    )
    .bind(claim.delivery.id)
    .bind(project_id)
    .bind(claim.lease_until)
    .bind(error.map(|e| telmoni_shared::text::truncate_on_char_boundary(e, LAST_ERROR_MAX)))
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(settled == 1)
}

/// Release a manual claim that sent nothing, while it is still ours.
pub async fn release_redelivery(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    project_id: &ProjectId,
    claim: &Claimed,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE notifications.deliveries
            SET lease_until = NULL
          WHERE id = $1 AND project_id = $2 AND lease_until = $3",
    )
    .bind(claim.delivery.id)
    .bind(project_id)
    .bind(claim.lease_until)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// How many times a delivery has been resent by hand. Read before a claim, so
/// the cap refuses before anything is sent.
pub async fn manual_resends(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    delivery_id: Uuid,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM notifications.delivery_attempts
          WHERE project_id = $1 AND delivery_id = $2 AND trigger = 'manual'",
    )
    .bind(project_id)
    .bind(delivery_id)
    .fetch_one(tx.conn())
    .await
}

/// Forget every rotated webhook secret whose overlap has ended. Nothing signs
/// with one past its expiry anyway ([`SealedConnection::prior_token`]); this
/// is so the sealed secret does not outlive the promise made about it.
pub async fn clear_expired_prior_tokens(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
) -> sqlx::Result<u64> {
    let cleared = sqlx::query(
        "UPDATE notifications.connections
            SET prior_token_ciphertext = NULL, prior_token_nonce = NULL,
                prior_token_expires_at = NULL
          WHERE prior_token_expires_at <= now()",
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(cleared)
}

/// Retention: terminal deliveries older than `days` and expired handshake
/// states. ⚠ At most `chunk` rows per table per call: every role carries a 5s
/// `statement_timeout`, and an unbounded delete over a month would exceed it.
/// Each delivery takes its attempts by cascade: at most the queue's five and
/// ten resends each, so a chunk is bounded at a fixed multiple. A row a
/// resend holds is left for the next sweep, so the send it is making is
/// logged against a row that still exists.
pub async fn sweep_connector_rows(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    delivery_retention_days: i32,
    chunk: i64,
) -> sqlx::Result<u64> {
    let mut removed = sqlx::query(
        "DELETE FROM notifications.deliveries
          WHERE id IN (
              SELECT id FROM notifications.deliveries
               WHERE status IN ('delivered', 'failed')
                 AND updated_at < now() - make_interval(days => $1)
                 AND (lease_until IS NULL OR lease_until < now())
               LIMIT $2
          )",
    )
    .bind(delivery_retention_days)
    .bind(chunk)
    .execute(tx.conn())
    .await?
    .rows_affected();
    removed += sqlx::query(
        "DELETE FROM notifications.oauth_states
          WHERE state_hash IN (
              SELECT state_hash FROM notifications.oauth_states
               WHERE expires_at < now()
               LIMIT $1
          )",
    )
    .bind(chunk)
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(removed)
}

/// Store one handshake's `state`, hashed, for the callback to consume.
pub async fn insert_oauth_state(
    tx: &mut Scoped<'_, Project>,
    state_hash: &str,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    user_id: &UserId,
    provider: Provider,
    ttl_secs: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO notifications.oauth_states
            (state_hash, project_id, organization_id, user_id, provider, expires_at, shard_key)
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6::bigint), $7)",
    )
    .bind(state_hash)
    .bind(project_id)
    .bind(organization_id)
    .bind(user_id)
    .bind(provider.as_str())
    .bind(ttl_secs)
    .bind(telmoni_shared::derive_shard_key(organization_id))
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Who started the handshake a `state` belongs to.
#[derive(Debug, FromRow)]
pub struct OAuthState {
    pub project_id: String,
    pub organization_id: String,
    pub user_id: String,
    pub provider: String,
}

/// Spend one `state`: live, unconsumed and visible under the caller's project
/// binding. Consumed on the way out, so a replayed callback finds nothing.
pub async fn consume_oauth_state(
    tx: &mut Scoped<'_, Project>,
    state_hash: &str,
) -> sqlx::Result<Option<OAuthState>> {
    sqlx::query_as::<_, OAuthState>(
        "UPDATE notifications.oauth_states
            SET consumed_at = now()
          WHERE state_hash = $1 AND consumed_at IS NULL AND expires_at > now()
      RETURNING project_id, organization_id, user_id, provider",
    )
    .bind(state_hash)
    .fetch_optional(tx.conn())
    .await
}

/// How old a row's paging time must be before the agent's index reads past
/// it. The emit's transaction stamps `created_at` at its start and the loop
/// stamps `updated_at` in its own, so a row committed after a later one
/// would otherwise land behind a cursor that had already moved on. Nothing
/// of this role's stays open past its two-minute idle cut-off.
const INDEX_SETTLE_SECONDS: i32 = 180;

/// A feed row as the agent's index reads it.
#[derive(Debug, FromRow)]
pub struct IndexedFeedItem {
    pub id: Uuid,
    pub project_id: Option<ProjectId>,
    pub organization_id: OrganizationId,
    pub subject_user_id: Option<UserId>,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// Every feed's rows after `(after_at, after_id)`, oldest first. Across
/// every tenant, so the lane.
pub async fn feed_after(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> sqlx::Result<Vec<IndexedFeedItem>> {
    let (after_at, after_id) = after.unwrap_or((DateTime::<Utc>::UNIX_EPOCH, Uuid::nil()));
    sqlx::query_as::<_, IndexedFeedItem>(
        "SELECT id, project_id, organization_id, subject_user_id, kind, title, body, created_at
           FROM notifications.feed
          WHERE (created_at, id) > ($1, $2)
            AND created_at < now() - make_interval(secs => $3)
          ORDER BY created_at, id
          LIMIT $4",
    )
    .bind(after_at)
    .bind(after_id)
    .bind(INDEX_SETTLE_SECONDS)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// A delivery as the agent's index reads it: its outcome and where it went,
/// not the text it carried, which the feed row already holds.
#[derive(Debug, FromRow)]
pub struct IndexedDelivery {
    pub id: Uuid,
    pub connection_id: Uuid,
    pub project_id: ProjectId,
    pub organization_id: OrganizationId,
    pub subject_user_id: Option<UserId>,
    pub kind: String,
    pub subject: String,
    pub status: String,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub provider: String,
    pub channel_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Every delivery changed after `(after_at, after_id)`, oldest change first:
/// by `updated_at`, so each attempt's outcome is read again as it lands.
pub async fn deliveries_after(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> sqlx::Result<Vec<IndexedDelivery>> {
    let (after_at, after_id) = after.unwrap_or((DateTime::<Utc>::UNIX_EPOCH, Uuid::nil()));
    sqlx::query_as::<_, IndexedDelivery>(
        "SELECT d.id, d.connection_id, d.project_id, d.organization_id, d.subject_user_id,
                d.kind, d.subject, d.status, d.attempts, d.last_error,
                c.provider, c.channel_name, d.created_at, d.updated_at
           FROM notifications.deliveries d
           JOIN notifications.connections c ON c.id = d.connection_id
          WHERE (d.updated_at, d.id) > ($1, $2)
            AND d.updated_at < now() - make_interval(secs => $3)
          ORDER BY d.updated_at, d.id
          LIMIT $4",
    )
    .bind(after_at)
    .bind(after_id)
    .bind(INDEX_SETTLE_SECONDS)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// The pending queue's depth and the age of its oldest attempt gate, for the
/// loop's once-a-minute log line. The lane, because under the service role
/// alone forced RLS hides every row and the count reads zero.
pub async fn queue_stats(
    tx: &mut Scoped<'_, Maintenance<NotificationsLane>>,
) -> sqlx::Result<(i64, Option<i64>)> {
    sqlx::query_as(
        "SELECT count(*), extract(epoch FROM (now() - min(next_attempt_at)))::bigint
           FROM notifications.deliveries
          WHERE status = 'pending'",
    )
    .fetch_one(tx.conn())
    .await
}
