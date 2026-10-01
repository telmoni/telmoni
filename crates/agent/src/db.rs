//! The agent's data layer: the index, the conversations and the cursors.
//!
//! Vectors cross the wire as pgvector's text form (`'[…]'::vector`), built
//! and width-checked by [`crate::embed::literal`], so no driver type for
//! them is needed.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use telmoni_shared::db::tenant_session::{
    Binding, HasOrganization, HasPerson, Lane, Maintenance, MaintenanceLane,
    PersonOrganizationProject, Scoped,
};
use telmoni_shared::seam::Audience;
use telmoni_shared::{OrganizationId, ProjectId, UserId};
use uuid::Uuid;

/// This module's cross-tenant lane. See `tenant_session::Lane`.
#[derive(Debug, Clone, Copy)]
pub struct AgentLane;
impl Lane for AgentLane {
    const SET_ROLE: &'static str = MaintenanceLane::Agent.set_role();
}

/// What a conversation read binds: the person and their organization.
pub trait AuthorBinding: HasPerson + HasOrganization {}
impl<B: HasPerson + HasOrganization> AuthorBinding for B {}

/// Where a passage came from: `agent.chunks.source`, and the key of a
/// source's row in `agent.cursors`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Docs,
    Audit,
    Feed,
    Delivery,
    Conversation,
}

impl Source {
    pub const ALL: [Self; 5] = [
        Self::Docs,
        Self::Audit,
        Self::Feed,
        Self::Delivery,
        Self::Conversation,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Docs => "docs",
            Self::Audit => "audit",
            Self::Feed => "feed",
            Self::Delivery => "delivery",
            Self::Conversation => "conversation",
        }
    }
}

/// Who reads a passage, beyond what RLS admits: `agent.chunks.visibility`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Every role on the project, or anyone at all for the docs.
    Everyone,
    /// A role that may read the audit log.
    Audit,
    /// The organization's owner.
    Owner,
    /// The person who asked, whom RLS alone decides.
    Author,
}

impl Visibility {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Everyone => "everyone",
            Self::Audit => "audit",
            Self::Owner => "owner",
            Self::Author => "author",
        }
    }
}

impl From<Audience> for Visibility {
    fn from(audience: Audience) -> Self {
        match audience {
            Audience::Everyone => Self::Everyone,
            Audience::Audit => Self::Audit,
            Audience::Owner => Self::Owner,
        }
    }
}

/// Who wrote a message: `agent.messages.role`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
}

// ── The index ────────────────────────────────────────────────────────────────

/// One passage to write, with its vector.
#[derive(Debug, Clone)]
pub struct NewChunk<'a> {
    pub organization_id: Option<&'a OrganizationId>,
    pub project_id: Option<&'a ProjectId>,
    pub user_id: Option<&'a UserId>,
    pub subject_user_id: Option<&'a UserId>,
    pub source: Source,
    pub source_id: &'a str,
    pub part: i32,
    pub visibility: Visibility,
    pub title: &'a str,
    pub body: &'a str,
    pub url: Option<&'a str>,
    pub content_hash: &'a str,
    /// `[…]`, from [`crate::embed::literal`].
    pub embedding: &'a str,
    pub model: &'a str,
    pub source_created_at: DateTime<Utc>,
}

/// Write one passage, or rewrite the one already at its key.
pub async fn upsert_chunk<B: Binding>(
    tx: &mut Scoped<'_, B>,
    chunk: &NewChunk<'_>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO agent.chunks
             (organization_id, project_id, user_id, subject_user_id, source, source_id, part,
              visibility, title, body, url, content_hash, embedding, model, source_created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13::vector, $14, $15)
         ON CONFLICT (source, source_id, part) DO UPDATE SET
             organization_id   = excluded.organization_id,
             project_id        = excluded.project_id,
             user_id           = excluded.user_id,
             subject_user_id   = excluded.subject_user_id,
             visibility        = excluded.visibility,
             title             = excluded.title,
             body              = excluded.body,
             url               = excluded.url,
             content_hash      = excluded.content_hash,
             embedding         = excluded.embedding,
             model             = excluded.model,
             source_created_at = excluded.source_created_at,
             updated_at        = now()",
    )
    .bind(chunk.organization_id)
    .bind(chunk.project_id)
    .bind(chunk.user_id)
    .bind(chunk.subject_user_id)
    .bind(chunk.source)
    .bind(chunk.source_id)
    .bind(chunk.part)
    .bind(chunk.visibility)
    .bind(chunk.title)
    .bind(chunk.body)
    .bind(chunk.url)
    .bind(chunk.content_hash)
    .bind(chunk.embedding)
    .bind(chunk.model)
    .bind(chunk.source_created_at)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// A passage already indexed, as the indexer compares it.
#[derive(Debug, FromRow)]
pub struct Indexed {
    pub source_id: String,
    pub part: i32,
    pub content_hash: String,
    pub model: String,
}

/// What is already indexed for these rows of one source.
pub async fn indexed(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    source_ids: &[&str],
) -> sqlx::Result<Vec<Indexed>> {
    sqlx::query_as::<_, Indexed>(
        "SELECT source_id, part, content_hash, model FROM agent.chunks
          WHERE source = $1 AND source_id = ANY($2)",
    )
    .bind(source)
    .bind(source_ids)
    .fetch_all(tx.conn())
    .await
}

/// Drop each row's passages past its last, after it shrank: `parts[i]` is
/// how many `source_ids[i]` has now. One statement for a page of rows.
pub async fn drop_parts_past(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    source_ids: &[&str],
    parts: &[i32],
) -> sqlx::Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM agent.chunks c
          USING unnest($2::text[], $3::int[]) AS kept(source_id, parts)
          WHERE c.source = $1 AND c.source_id = kept.source_id AND c.part >= kept.parts",
    )
    .bind(source)
    .bind(source_ids)
    .bind(parts)
    .execute(tx.conn())
    .await?
    .rows_affected())
}

/// Drop every passage of a source whose row is not among `keep`: the docs
/// pages that left the corpus.
pub async fn drop_source_except(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    keep: &[String],
) -> sqlx::Result<u64> {
    Ok(
        sqlx::query("DELETE FROM agent.chunks WHERE source = $1 AND NOT (source_id = ANY($2))")
            .bind(source)
            .bind(keep)
            .execute(tx.conn())
            .await?
            .rows_affected(),
    )
}

/// One passage the search found.
#[derive(Debug, Clone, FromRow)]
pub struct Found {
    pub id: Uuid,
    pub source: Source,
    pub title: String,
    pub body: String,
    pub url: Option<String>,
    pub source_created_at: DateTime<Utc>,
    pub score: f64,
}

/// How deep each half of the search reads before the two are fused.
const CANDIDATES: i64 = 40;

/// Reciprocal rank fusion's constant: the rank at which a hit's weight has
/// halved. 60 is the value the method was published with, and nothing here
/// has been tuned against it.
const RRF_K: f64 = 60.0;

/// The hybrid search, under the asker's own scope: the nearest passages by
/// meaning and the best by exact terms, each ranked, fused by reciprocal
/// rank. The terms are OR'd: a question AND'd word by word matches almost
/// nothing, and `ts_rank_cd` already ranks a passage holding more of them
/// higher. RLS decides which tenant's rows exist; `visibility` narrows them to
/// what this person's role reads.
///
/// ⚠ `iterative_scan`: an HNSW scan returns its `ef_search` nearest across
/// every tenant and only then applies the filters, so in a large index a
/// small tenant's rows could all fall outside that first batch and the
/// search would come back empty. pgvector 0.8 keeps scanning until the
/// filtered result is full.
pub async fn search(
    tx: &mut Scoped<'_, PersonOrganizationProject>,
    embedding: &str,
    query: &str,
    visibility: &[Visibility],
    limit: i64,
) -> sqlx::Result<Vec<Found>> {
    let visibility: Vec<&str> = visibility.iter().map(|v| v.as_str()).collect();
    sqlx::query("SET LOCAL hnsw.iterative_scan = relaxed_order")
        .execute(tx.conn())
        .await?;
    sqlx::query_as::<_, Found>(
        "WITH semantic AS (
             SELECT id, row_number() OVER (ORDER BY distance) AS rank
               FROM (SELECT id, embedding <=> $1::vector AS distance
                       FROM agent.chunks
                      WHERE visibility = ANY($3)
                      ORDER BY embedding <=> $1::vector
                      LIMIT $4) nearest
         ),
         terms AS (
             SELECT to_tsquery('simple',
                        replace(plainto_tsquery('simple', $2)::text, ' & ', ' | ')) AS q
         ),
         lexical AS (
             SELECT id, row_number() OVER (ORDER BY weight DESC) AS rank
               FROM (SELECT c.id, ts_rank_cd(c.tsv, terms.q) AS weight
                       FROM agent.chunks c, terms
                      WHERE c.tsv @@ terms.q
                        AND c.visibility = ANY($3)
                      ORDER BY weight DESC
                      LIMIT $4) matched
         ),
         fused AS (
             SELECT COALESCE(s.id, l.id) AS id,
                    COALESCE(1.0 / ($5 + s.rank), 0) + COALESCE(1.0 / ($5 + l.rank), 0) AS score
               FROM semantic s FULL JOIN lexical l ON l.id = s.id
         )
         SELECT c.id, c.source, c.title, c.body, c.url, c.source_created_at,
                f.score::double precision AS score
           FROM fused f JOIN agent.chunks c ON c.id = f.id
          ORDER BY f.score DESC, c.source_created_at DESC
          LIMIT $6",
    )
    .bind(embedding)
    .bind(query)
    .bind(&visibility)
    .bind(CANDIDATES)
    .bind(RRF_K)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

// ── Cursors ──────────────────────────────────────────────────────────────────

/// A source's cursor, as the replica that locked it reads it.
#[derive(Debug, FromRow)]
pub struct Cursor {
    pub after_at: Option<DateTime<Utc>>,
    pub after_id: Option<String>,
    pub digest: Option<String>,
}

/// Lock a source's cursor for this transaction, or `None` when another
/// replica is indexing it — the same `SKIP LOCKED` the delivery loop leases by.
pub async fn lock_cursor(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
) -> sqlx::Result<Option<Cursor>> {
    sqlx::query_as::<_, Cursor>(
        "SELECT after_at, after_id, digest FROM agent.cursors
          WHERE source = $1
          FOR UPDATE SKIP LOCKED",
    )
    .bind(source)
    .fetch_optional(tx.conn())
    .await
}

/// Move a source's cursor past the last row taken.
pub async fn advance_cursor(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    at: DateTime<Utc>,
    id: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE agent.cursors SET after_at = $2, after_id = $3, updated_at = now()
          WHERE source = $1",
    )
    .bind(source)
    .bind(at)
    .bind(id)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Record the corpus a source was last indexed from.
pub async fn record_digest(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    digest: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE agent.cursors SET digest = $2, updated_at = now() WHERE source = $1")
        .bind(source)
        .bind(digest)
        .execute(tx.conn())
        .await?;
    Ok(())
}

// ── Re-embedding ─────────────────────────────────────────────────────────────

/// A passage a different model embedded.
#[derive(Debug, FromRow)]
pub struct Stale {
    pub id: Uuid,
    pub title: String,
    pub body: String,
}

/// The next passages whose model is not `model`.
pub async fn stale_chunks(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    model: &str,
    limit: i64,
) -> sqlx::Result<Vec<Stale>> {
    sqlx::query_as::<_, Stale>(
        "SELECT id, title, body FROM agent.chunks WHERE model <> $1 ORDER BY id LIMIT $2",
    )
    .bind(model)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// Give one passage its new vector.
pub async fn reembed(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    id: Uuid,
    embedding: &str,
    model: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE agent.chunks SET embedding = $2::vector, model = $3, updated_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(embedding)
    .bind(model)
    .execute(tx.conn())
    .await?;
    Ok(())
}

// ── Conversations ────────────────────────────────────────────────────────────

/// A person's questions in the last hour, and when the oldest of them was
/// asked — which is when the window next has room.
///
/// ⚠ `role = 'user'` stays a literal: it is `messages_rate_idx`'s predicate,
/// and a bound `$n` in a generic plan cannot be proved to match it, so the
/// planner would stop using the index.
pub async fn recent_questions<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
) -> sqlx::Result<(i64, Option<DateTime<Utc>>)> {
    sqlx::query_as(
        "SELECT count(*), min(created_at) FROM agent.messages
          WHERE user_id = $1 AND role = 'user' AND created_at > now() - interval '1 hour'",
    )
    .bind(user_id)
    .fetch_one(tx.conn())
    .await
}

/// Start a conversation.
pub async fn create_conversation<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    user_id: &UserId,
    title: &str,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO agent.conversations (organization_id, project_id, user_id, title)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(organization_id)
    .bind(project_id)
    .bind(user_id)
    .bind(title)
    .fetch_one(tx.conn())
    .await
}

/// Whether the person holds this conversation here, and mark it used.
pub async fn touch_conversation<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    id: Uuid,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE agent.conversations SET updated_at = now() WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .execute(tx.conn())
    .await?
    .rows_affected()
        == 1)
}

/// Add one message to a conversation.
#[expect(
    clippy::too_many_arguments,
    reason = "each is one column of the row; a struct for one call site would only rename them"
)]
pub async fn insert_message<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    conversation_id: Uuid,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    user_id: &UserId,
    role: MessageRole,
    content: &str,
    citations: &Value,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO agent.messages
             (conversation_id, organization_id, project_id, user_id, role, content, citations)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(conversation_id)
    .bind(organization_id)
    .bind(project_id)
    .bind(user_id)
    .bind(role)
    .bind(content)
    .bind(citations)
    .fetch_one(tx.conn())
    .await
}

/// One message as the panel and the model's transcript read it.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct StoredMessage {
    pub id: Uuid,
    pub role: MessageRole,
    pub content: String,
    pub citations: Value,
    pub created_at: DateTime<Utc>,
}

/// The conversation's last `limit` messages, oldest first.
pub async fn messages<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    conversation_id: Uuid,
    limit: i64,
) -> sqlx::Result<Vec<StoredMessage>> {
    sqlx::query_as::<_, StoredMessage>(
        "SELECT id, role, content, citations, created_at FROM (
             SELECT id, role, content, citations, created_at FROM agent.messages
              WHERE conversation_id = $1
              ORDER BY created_at DESC, id DESC
              LIMIT $2
         ) latest
         ORDER BY created_at, id",
    )
    .bind(conversation_id)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// A conversation as the panel lists it.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ConversationSummary {
    pub id: Uuid,
    pub title: String,
    pub project_id: ProjectId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The person's conversations asked from this project, newest first.
pub async fn list_conversations<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    project_id: &ProjectId,
    limit: i64,
) -> sqlx::Result<Vec<ConversationSummary>> {
    sqlx::query_as::<_, ConversationSummary>(
        "SELECT id, title, project_id, created_at, updated_at FROM agent.conversations
          WHERE user_id = $1 AND project_id = $2
          ORDER BY updated_at DESC, id DESC
          LIMIT $3",
    )
    .bind(user_id)
    .bind(project_id)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// One of the person's conversations.
pub async fn conversation<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    id: Uuid,
) -> sqlx::Result<Option<ConversationSummary>> {
    sqlx::query_as::<_, ConversationSummary>(
        "SELECT id, title, project_id, created_at, updated_at FROM agent.conversations
          WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}

/// Delete one of the person's conversations, and the passages it left in
/// the index. Its messages cascade.
pub async fn delete_conversation<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    id: Uuid,
) -> sqlx::Result<bool> {
    sqlx::query(
        "DELETE FROM agent.chunks
          WHERE source = $1 AND user_id = $2 AND source_id LIKE $3 || ':%'",
    )
    .bind(Source::Conversation)
    .bind(user_id)
    .bind(id.to_string())
    .execute(tx.conn())
    .await?;
    Ok(
        sqlx::query("DELETE FROM agent.conversations WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(tx.conn())
            .await?
            .rows_affected()
            == 1,
    )
}

/// Every conversation the person holds in the organization, with its
/// messages, for their export.
pub async fn export<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    organization_id: &OrganizationId,
) -> sqlx::Result<Value> {
    sqlx::query_scalar(
        "SELECT COALESCE(json_agg(row_to_json(t) ORDER BY t.created_at), '[]'::json)::jsonb FROM (
             SELECT c.id, c.title, c.project_id, c.created_at, c.updated_at,
                    COALESCE((SELECT json_agg(json_build_object(
                                  'role', m.role, 'content', m.content,
                                  'citations', m.citations, 'created_at', m.created_at)
                                  ORDER BY m.created_at, m.id)
                                FROM agent.messages m WHERE m.conversation_id = c.id),
                             '[]'::json) AS messages
               FROM agent.conversations c
              WHERE c.user_id = $1 AND c.organization_id = $2
         ) t",
    )
    .bind(user_id)
    .bind(organization_id)
    .fetch_one(tx.conn())
    .await
}

// ── Retention and purges ─────────────────────────────────────────────────────

/// Whether this process may sweep: one advisory lock, held to the
/// transaction, so replicas take turns rather than queue.
pub async fn try_retention_lock(tx: &mut Scoped<'_, Maintenance<AgentLane>>) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended('agent:retention', 0))")
        .fetch_one(tx.conn())
        .await
}

/// Up to `chunk` passages of one source older than its window.
pub async fn sweep_chunks(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    days: i32,
    chunk: i64,
) -> sqlx::Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM agent.chunks
          WHERE id IN (
              SELECT id FROM agent.chunks
               WHERE source = $1 AND source_created_at < now() - make_interval(days => $2)
               LIMIT $3
          )",
    )
    .bind(source)
    .bind(days)
    .bind(chunk)
    .execute(tx.conn())
    .await?
    .rows_affected())
}

/// Up to `chunk` conversations unused for `days`; their messages cascade.
pub async fn sweep_conversations(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    days: i32,
    chunk: i64,
) -> sqlx::Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM agent.conversations
          WHERE id IN (
              SELECT id FROM agent.conversations
               WHERE updated_at < now() - make_interval(days => $1)
               LIMIT $2
          )",
    )
    .bind(days)
    .bind(chunk)
    .execute(tx.conn())
    .await?
    .rows_affected())
}

/// Everything held of one person: their conversations and every passage
/// that is theirs or names them.
pub async fn erase_person(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    user_id: &UserId,
) -> sqlx::Result<u64> {
    let mut gone = 0;
    for sql in [
        "DELETE FROM agent.chunks WHERE user_id = $1 OR subject_user_id = $1",
        "DELETE FROM agent.conversations WHERE user_id = $1",
    ] {
        gone += sqlx::query(sql)
            .bind(user_id)
            .execute(tx.conn())
            .await?
            .rows_affected();
    }
    Ok(gone)
}

/// Everything held of one organization.
pub async fn purge_organization(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    organization_id: &OrganizationId,
) -> sqlx::Result<u64> {
    let mut gone = 0;
    for sql in [
        "DELETE FROM agent.chunks WHERE organization_id = $1",
        "DELETE FROM agent.conversations WHERE organization_id = $1",
    ] {
        gone += sqlx::query(sql)
            .bind(organization_id)
            .execute(tx.conn())
            .await?
            .rows_affected();
    }
    Ok(gone)
}

/// Everything held of one project.
pub async fn purge_project(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    project_id: &ProjectId,
) -> sqlx::Result<u64> {
    let mut gone = 0;
    for sql in [
        "DELETE FROM agent.chunks WHERE project_id = $1",
        "DELETE FROM agent.conversations WHERE project_id = $1",
    ] {
        gone += sqlx::query(sql)
            .bind(project_id)
            .execute(tx.conn())
            .await?
            .rows_affected();
    }
    Ok(gone)
}
