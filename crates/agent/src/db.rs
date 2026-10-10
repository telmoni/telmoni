//! The agent's data layer: the index, the conversations and the cursors.
//!
//! Vectors cross the wire as pgvector's text form (`'[…]'::vector`), built
//! and width-checked by [`crate::embed::literal`], so no driver type for
//! them is needed.

use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use telmoni_shared::db::tenant_session::{
    Binding, HasOrganization, HasPerson, Lane, Maintenance, MaintenanceLane,
    PersonOrganizationProject, Scoped, maintenance_scope,
};
use telmoni_shared::seam::Audience;
use telmoni_shared::{OrganizationId, ParseEnumError, ProjectId, UserId};
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
    /// Every source, in the migration's order.
    #[must_use]
    pub const fn all() -> [Self; 5] {
        [
            Self::Docs,
            Self::Audit,
            Self::Feed,
            Self::Delivery,
            Self::Conversation,
        ]
    }

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

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Source {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::all()
            .into_iter()
            .find(|source| source.as_str() == s)
            .ok_or_else(|| ParseEnumError::new(s, "docs, audit, feed, delivery, conversation"))
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
    /// The organization's owner and its admins.
    OrganizationAdmin,
    /// The person who asked, whom RLS alone decides.
    Author,
}

impl Visibility {
    /// Every visibility, in the migration's order.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [
            Self::Everyone,
            Self::Audit,
            Self::OrganizationAdmin,
            Self::Author,
        ]
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Everyone => "everyone",
            Self::Audit => "audit",
            Self::OrganizationAdmin => "organization_admin",
            Self::Author => "author",
        }
    }
}

impl fmt::Display for Visibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Visibility {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::all()
            .into_iter()
            .find(|visibility| visibility.as_str() == s)
            .ok_or_else(|| ParseEnumError::new(s, "everyone, audit, organization_admin, author"))
    }
}

impl From<Audience> for Visibility {
    fn from(audience: Audience) -> Self {
        match audience {
            Audience::Everyone => Self::Everyone,
            Audience::Audit => Self::Audit,
            Audience::OrganizationAdmin => Self::OrganizationAdmin,
        }
    }
}

/// Who wrote a message: `agent.messages.role`. The console reads it as
/// `AGENT_ROLES` (`web/lib/types/agent.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
}

impl MessageRole {
    /// Every role, in the migration's order.
    #[must_use]
    pub const fn all() -> [Self; 2] {
        [Self::User, Self::Assistant]
    }
}

impl fmt::Display for MessageRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Assistant => write!(f, "assistant"),
        }
    }
}

impl FromStr for MessageRole {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            _ => Err(ParseEnumError::new(s, "user, assistant")),
        }
    }
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

/// Drop the passages `parts[i]` of `source_ids[i]`, each one. One statement
/// for a page of rows.
pub async fn drop_parts(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    source_ids: &[&str],
    parts: &[i32],
) -> sqlx::Result<u64> {
    if source_ids.is_empty() {
        return Ok(0);
    }
    Ok(sqlx::query(
        "DELETE FROM agent.chunks c
          USING unnest($2::text[], $3::int[]) AS gone(source_id, part)
          WHERE c.source = $1 AND c.source_id = gone.source_id AND c.part = gone.part",
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

/// How many rows one person may read in a project, and its organization,
/// for them to be compared exactly rather than through the HNSW graph. Each
/// vector is read from its out-of-line storage, two random reads apiece on a
/// cold cache, so this is a second or two at worst, inside the role's
/// statement timeout.
const EXACT_MAX: i64 = 1_000;

/// The hybrid search, under the asker's own scope: the nearest passages by
/// meaning and the best by exact terms, each ranked, fused by reciprocal
/// rank. The terms are OR'd: a question AND'd word by word matches almost
/// nothing, and `ts_rank_cd` already ranks a passage holding more of them
/// higher. `visibility` narrows what the scope admits to what this person's
/// role reads.
///
/// ⚠ **Every half names the tenant itself, not just RLS.** The policies admit
/// the docs, the project's rows and the organization's, and the asker's own
/// exchanges; each half says the same in its `WHERE`. A module connected as
/// the database's superuser — the Docker Compose quickstart — is held by no
/// policy at all, and a search that trusted them read every organization's
/// passages there.
///
/// ⚠ **A small tenant's rows are compared exactly, never read past.** An HNSW
/// scan returns its `ef_search` nearest across every tenant and only then
/// applies the filters, so in a large index a small tenant's rows could all
/// fall outside it, and the search came back empty. pgvector 0.8 scans on
/// until the filtered result is full, but only as far as
/// `hnsw.max_scan_tuples`, a heap read apiece: for a tenant that small a
/// share of the index, the scan ran to that limit, slowly, and still came
/// back short. So the meaning half reads three ways:
/// - **the docs** through their own graph, which no tenant's rows crowd;
/// - **what this person reads here**, when it is under `EXACT_MAX` rows —
///   counted no further than that, by who may read it — compared whole,
///   through its own indexes, and the shared graph not scanned at all;
/// - **a larger tenant's** through the tenants' graph, scanning on until its
///   own rows fill the list — and when that comes back short, a small share
///   of a very large index, its newest rows this person reads compared whole
///   as well, as many as a small tenant's.
///
/// The asker's past exchanges count only from this project: one asked in
/// another can carry that project's members and audit into this one's turn.
/// And only vectors of the query's `model` are compared to it: another
/// model's distances mean nothing against this one's, so until a reindex
/// has caught up those passages are found by their terms alone.
pub async fn search(
    tx: &mut Scoped<'_, PersonOrganizationProject>,
    embedding: &str,
    model: &str,
    query: &str,
    visibility: &[Visibility],
    limit: i64,
) -> sqlx::Result<Vec<Found>> {
    let visibility: Vec<&str> = visibility.iter().map(|v| v.as_str()).collect();
    sqlx::query("SET LOCAL hnsw.iterative_scan = relaxed_order")
        .execute(tx.conn())
        .await?;
    sqlx::query_as::<_, Found>(
        "WITH readable AS MATERIALIZED (
             -- What this person reads here, counted no further than the cap:
             -- three ranges of their own indexes, never a read of the
             -- organization's rows, nor of other members' exchanges.
             SELECT count(*) AS n
               FROM ((SELECT 1 FROM agent.chunks
                       WHERE organization_id = current_setting('app.organization_id', true)
                         AND project_id = current_setting('app.project_id', true)
                         AND visibility = ANY($3) AND user_id IS NULL
                       LIMIT $8)
                     UNION ALL
                     (SELECT 1 FROM agent.chunks
                       WHERE organization_id = current_setting('app.organization_id', true)
                         AND project_id IS NULL
                         AND visibility = ANY($3) AND user_id IS NULL
                       LIMIT $8)
                     UNION ALL
                     (SELECT 1 FROM agent.chunks
                       WHERE user_id = current_setting('app.user_id', true)
                         AND organization_id = current_setting('app.organization_id', true)
                         AND project_id = current_setting('app.project_id', true)
                       LIMIT $8)) counted
         ),
         docs AS (
             SELECT id, embedding <=> $1::vector AS distance
               FROM agent.chunks
              WHERE organization_id IS NULL AND visibility = ANY($3) AND model = $7
              ORDER BY embedding <=> $1::vector
              LIMIT $4
         ),
         -- The tenants' graph, for a tenant too large to compare whole; not
         -- scanned at all for a smaller one.
         nearest AS (
             SELECT id, embedding <=> $1::vector AS distance
               FROM agent.chunks
              WHERE organization_id = current_setting('app.organization_id', true)
                AND (project_id IS NULL OR project_id = current_setting('app.project_id', true))
                AND (user_id IS NULL
                     OR (user_id = current_setting('app.user_id', true)
                         AND project_id = current_setting('app.project_id', true)))
                AND visibility = ANY($3)
                AND model = $7
                AND (SELECT n FROM readable) >= $8
              ORDER BY embedding <=> $1::vector
              LIMIT $4
         ),
         -- Everything a smaller tenant's person reads here, compared whole:
         -- the same three ranges the count read.
         own AS MATERIALIZED (
             SELECT id, embedding <=> $1::vector AS distance
               FROM agent.chunks
              WHERE organization_id = current_setting('app.organization_id', true)
                AND project_id = current_setting('app.project_id', true)
                AND visibility = ANY($3) AND user_id IS NULL AND model = $7
                AND (SELECT n FROM readable) < $8
             UNION ALL
             SELECT id, embedding <=> $1::vector AS distance
               FROM agent.chunks
              WHERE organization_id = current_setting('app.organization_id', true)
                AND project_id IS NULL
                AND visibility = ANY($3) AND user_id IS NULL AND model = $7
                AND (SELECT n FROM readable) < $8
             UNION ALL
             SELECT id, embedding <=> $1::vector AS distance
               FROM agent.chunks
              WHERE user_id = current_setting('app.user_id', true)
                AND organization_id = current_setting('app.organization_id', true)
                AND project_id = current_setting('app.project_id', true)
                AND visibility = ANY($3) AND model = $7
                AND (SELECT n FROM readable) < $8
         ),
         -- A larger tenant whose graph scan came back short: its newest rows
         -- this person reads, compared whole, as many as a small tenant's —
         -- the three ranges again, each read newest first. The shared ones a
         -- visibility at a time: the index orders each by date, and all of
         -- them together it cannot, so asked at once they were read whole and
         -- sorted, a large tenant's every row.
         recent AS MATERIALIZED (
             SELECT c.id, c.embedding <=> $1::vector AS distance
               FROM (SELECT id
                       FROM ((SELECT shared.id, shared.source_created_at
                                FROM unnest($3::text[]) AS read_as (visibility)
                                CROSS JOIN LATERAL
                                     (SELECT id, source_created_at FROM agent.chunks
                                       WHERE organization_id = current_setting('app.organization_id', true)
                                         AND project_id = current_setting('app.project_id', true)
                                         AND visibility = read_as.visibility AND user_id IS NULL
                                       ORDER BY source_created_at DESC LIMIT $8) shared)
                             UNION ALL
                             (SELECT shared.id, shared.source_created_at
                                FROM unnest($3::text[]) AS read_as (visibility)
                                CROSS JOIN LATERAL
                                     (SELECT id, source_created_at FROM agent.chunks
                                       WHERE organization_id = current_setting('app.organization_id', true)
                                         AND project_id IS NULL
                                         AND visibility = read_as.visibility AND user_id IS NULL
                                       ORDER BY source_created_at DESC LIMIT $8) shared)
                             UNION ALL
                             (SELECT id, source_created_at FROM agent.chunks
                               WHERE user_id = current_setting('app.user_id', true)
                                 AND organization_id = current_setting('app.organization_id', true)
                                 AND project_id = current_setting('app.project_id', true)
                               ORDER BY source_created_at DESC LIMIT $8)) newest
                      ORDER BY source_created_at DESC
                      LIMIT $8) picked
               JOIN agent.chunks c ON c.id = picked.id
              WHERE c.model = $7
                AND (SELECT n FROM readable) >= $8
                AND (SELECT count(*) FROM nearest) < $4
         ),
         semantic AS (
             SELECT id, row_number() OVER (ORDER BY distance) AS rank
               FROM (SELECT id, min(distance) AS distance
                       FROM (SELECT id, distance FROM docs
                              UNION ALL
                             SELECT id, distance FROM nearest
                              UNION ALL
                             SELECT id, distance
                               FROM (SELECT id, distance FROM own ORDER BY distance LIMIT $4) closest
                              UNION ALL
                             SELECT id, distance
                               FROM (SELECT id, distance FROM recent ORDER BY distance LIMIT $4) latest
                            ) candidates
                      GROUP BY id) found
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
                        AND (c.organization_id IS NULL
                             OR (c.organization_id = current_setting('app.organization_id', true)
                                 AND (c.project_id IS NULL OR c.project_id = current_setting('app.project_id', true))
                                 AND (c.user_id IS NULL OR c.user_id = current_setting('app.user_id', true))))
                        AND (c.user_id IS NULL OR c.project_id = current_setting('app.project_id', true))
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
    .bind(model)
    .bind(EXACT_MAX)
    .fetch_all(tx.conn())
    .await
}

// ── Cursors ──────────────────────────────────────────────────────────────────

/// A source's cursor, leased to one replica until `leased_until`.
#[derive(Debug, FromRow)]
pub struct Lease {
    pub lease_id: Uuid,
    pub after_at: Option<DateTime<Utc>>,
    pub after_id: Option<String>,
    pub digest: Option<String>,
}

/// Lease a source's cursor for `secs`, or `None` while another replica holds
/// it. Committed on its own: the lease outlives this transaction, so the
/// embedding that follows holds none open.
pub async fn lease_cursor(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    secs: i32,
) -> sqlx::Result<Option<Lease>> {
    sqlx::query_as::<_, Lease>(
        "UPDATE agent.cursors
            SET lease_id = gen_random_uuid(),
                leased_until = now() + make_interval(secs => $2)
          WHERE source = $1 AND (leased_until IS NULL OR leased_until <= now())
          RETURNING lease_id, after_at, after_id, digest",
    )
    .bind(source)
    .bind(secs)
    .fetch_optional(tx.conn())
    .await
}

/// Put a lease's end off to `secs` from now, while it is still this one:
/// `false` once it is not — taken by another replica, or ended by an
/// erasure — which nothing here takes back.
pub async fn renew_lease(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    lease_id: Uuid,
    secs: i32,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE agent.cursors SET leased_until = now() + make_interval(secs => $3)
          WHERE source = $1 AND lease_id = $2",
    )
    .bind(source)
    .bind(lease_id)
    .bind(secs)
    .execute(tx.conn())
    .await?
    .rows_affected()
        == 1)
}

/// End a lease and lock its row for this transaction: `false` when the lease
/// ran out and another replica took the cursor, whose work then stands and
/// this one's is dropped, or an erasure ended it.
///
/// ⚠ **Last in the write, before the cursor moves and the commit.** Settled
/// first, the row stayed locked while every passage was written, and an
/// erasure ending the leases queued behind the whole write.
pub async fn settle_lease(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    lease_id: Uuid,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE agent.cursors SET lease_id = NULL, leased_until = NULL
          WHERE source = $1 AND lease_id = $2",
    )
    .bind(source)
    .bind(lease_id)
    .execute(tx.conn())
    .await?
    .rows_affected()
        == 1)
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

/// Hand a lease back before it runs out: its page had nothing to write, or
/// failed and is read again from the cursor.
pub async fn release_lease(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    lease_id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE agent.cursors SET lease_id = NULL, leased_until = NULL
          WHERE source = $1 AND lease_id = $2",
    )
    .bind(source)
    .bind(lease_id)
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
    after: Uuid,
    limit: i64,
) -> sqlx::Result<Vec<Stale>> {
    sqlx::query_as::<_, Stale>(
        "SELECT id, title, body FROM agent.chunks
          WHERE model <> $1 AND id > $2 ORDER BY id LIMIT $3",
    )
    .bind(model)
    .bind(after)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// Give one passage its new vector, unless the indexer already has: it
/// writes with the configured model too, from text that may be newer than
/// what this vector was made from. Answers whether it was redone.
pub async fn reembed(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    id: Uuid,
    embedding: &str,
    model: &str,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE agent.chunks SET embedding = $2::vector, model = $3, updated_at = now()
          WHERE id = $1 AND model <> $3",
    )
    .bind(id)
    .bind(embedding)
    .bind(model)
    .execute(tx.conn())
    .await?
    .rows_affected()
        == 1)
}

// ── Conversations ────────────────────────────────────────────────────────────

/// A person's questions in the last hour, and when the oldest of them was
/// asked — which is when the window next has room.
///
/// ⚠ `role = 'user'` stays a literal: it is `messages_rate_idx`'s predicate,
/// and a bound `$n` in a generic plan cannot be proved to match it, so the
/// planner would stop using the index.
///
/// ⚠ **Counted under a lock on the person**, held until the question is
/// written: otherwise every request sent at once counted the same hour,
/// each found room, and the cap let through as many as arrived together.
pub async fn recent_questions<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
) -> sqlx::Result<(i64, Option<DateTime<Utc>>)> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('agent:asks:' || $1, 0))")
        .bind(user_id)
        .execute(tx.conn())
        .await?;
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
///
/// Only from the project it was asked in: continued from another, its
/// history would feed one project's members and audit into a turn whose
/// tools run against the other.
pub async fn touch_conversation<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE agent.conversations SET updated_at = now()
          WHERE id = $1 AND user_id = $2 AND project_id = $3",
    )
    .bind(id)
    .bind(user_id)
    .bind(project_id)
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

/// The transaction's own time: when a turn's question was written.
pub async fn now<B: Binding>(tx: &mut Scoped<'_, B>) -> sqlx::Result<DateTime<Utc>> {
    sqlx::query_scalar("SELECT now()")
        .fetch_one(tx.conn())
        .await
}

/// How long a fence holds answers past its scrub's last step: every step
/// puts it down again as it commits (`keep_fence`), so a scrub of any length
/// holds it while it runs, and one whose process went away lets it lapse —
/// rather than hold every answer in the person's organizations until a retry
/// came, or for good when none did. Past the longest step (a minute) and any
/// turn begun before the fence, which its deadline ends (`turn::TURN_BUDGET`,
/// tool calls included). Every erase fences on rows of its own, so one that
/// fences and dies cannot release a turn another's close still holds.
const OPEN_FENCE_MINUTES: i32 = 15;

/// How long saving an answer may wait on its organization's erasure lock: a
/// fence holds it only as long as its own two-second waits for the other
/// keys allow. The turn's budget, this and the rest of the save stay under
/// the console's cut-off (`turn::TURN_BUDGET`).
pub(crate) const SAVE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The save's statement on the lock as a whole, the wait in it.
pub(crate) const SAVE_STATEMENT: std::time::Duration = std::time::Duration::from_secs(8);

/// Whether an erasure reached this organization during a turn begun at
/// `asked_at`: one still scrubbing, or one whose scrub was done after the
/// turn began. The turn may have read the person before the scrub took
/// them, so what it wrote is not kept. Under the organization's erasure
/// lock, shared and held to the transaction: the write it guards lands
/// wholly before a fence goes down (and the scrub behind it finds it), or
/// after (and sees it).
pub async fn erased_during<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    asked_at: DateTime<Utc>,
) -> sqlx::Result<bool> {
    // A fence going down waits on other organizations' saves too, and a save
    // given up on the role's two seconds loses the answer it was keeping.
    // The statement's own cut-off too: the role's five seconds count the
    // lock's wait, and would end it first.
    sqlx::query(
        "SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $2, true)",
    )
    .bind(format!("{}ms", SAVE_LOCK_WAIT.as_millis()))
    .bind(format!("{}ms", SAVE_STATEMENT.as_millis()))
    .execute(tx.conn())
    .await?;
    lock_key(tx, ERASURE_KEY, organization_id.as_str(), Hold::Shared).await?;
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM agent.erasures
                         WHERE organization_id = $1
                           AND (((scrubbed_at IS NULL OR scrubbed_at < fenced_at)
                                 AND fenced_at > now() - make_interval(mins => $3))
                                OR scrubbed_at >= $2))",
    )
    .bind(organization_id)
    .bind(asked_at)
    .bind(OPEN_FENCE_MINUTES)
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

/// One of the person's conversations, asked from this project.
pub async fn conversation<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<ConversationSummary>> {
    sqlx::query_as::<_, ConversationSummary>(
        "SELECT id, title, project_id, created_at, updated_at FROM agent.conversations
          WHERE id = $1 AND user_id = $2 AND project_id = $3",
    )
    .bind(id)
    .bind(user_id)
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await
}

/// Whether the conversation still stands, holding it until this transaction
/// ends: a delete that comes after waits, then sweeps what this one wrote.
pub async fn hold_conversation<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    id: Uuid,
) -> sqlx::Result<bool> {
    Ok(
        sqlx::query("SELECT 1 FROM agent.conversations WHERE id = $1 FOR SHARE")
            .bind(id)
            .fetch_optional(tx.conn())
            .await?
            .is_some(),
    )
}

/// Delete one of the person's conversations, and the passages it left in
/// the index. Its messages cascade.
///
/// ⚠ **The conversation first, then its passages.** Deleting the row waits
/// on an exchange being remembered ([`hold_conversation`]); the passages are
/// swept after, so the one it wrote goes too rather than outliving the
/// conversation for its 90 days.
pub async fn delete_conversation<B: AuthorBinding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    id: Uuid,
) -> sqlx::Result<bool> {
    let deleted = sqlx::query("DELETE FROM agent.conversations WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(tx.conn())
        .await?
        .rows_affected()
        == 1;
    sqlx::query(
        "DELETE FROM agent.chunks
          WHERE source = $1 AND user_id = $2 AND source_id LIKE $3 || ':%'",
    )
    .bind(Source::Conversation)
    .bind(user_id)
    .bind(id.to_string())
    .execute(tx.conn())
    .await?;
    Ok(deleted)
}

// ── Retention and purges ─────────────────────────────────────────────────────

/// Whether this process may sweep: one advisory lock, held to the
/// transaction, so replicas take turns rather than queue.
pub async fn try_retention_lock(tx: &mut Scoped<'_, Maintenance<AgentLane>>) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended('agent:retention', 0))")
        .fetch_one(tx.conn())
        .await
}

/// [`try_retention_lock`] for the pass over what projects left behind: a
/// lock of its own, so the windows' rounds and its pages never cut each
/// other short.
pub async fn try_projects_lock(tx: &mut Scoped<'_, Maintenance<AgentLane>>) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended('agent:projects', 0))")
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

/// What an erased person's name becomes in someone else's conversation.
const FORMER_MEMBER: &str = "a former member";

/// An organization's erasure lock: a fence goes down and comes up under it,
/// exclusively, and every answer's save there takes it shared
/// ([`erased_during`]). Per organization, so an erasure holds back the
/// answers of the organizations it reaches and nobody else's.
const ERASURE_KEY: &str = "agent:erasure:";

/// How an advisory lock is held.
#[derive(Debug, Clone, Copy)]
enum Hold {
    Shared,
    Exclusive,
}

/// Take the advisory lock `prefix` + `key` names until the transaction
/// ends. Whoever takes several takes them in sorted order, so no two
/// transactions ever wait on each other in a ring.
async fn lock_key<B: Binding>(
    tx: &mut Scoped<'_, B>,
    prefix: &str,
    key: &str,
    hold: Hold,
) -> sqlx::Result<()> {
    let sql = match hold {
        Hold::Shared => "SELECT pg_advisory_xact_lock_shared(hashtextextended($1 || $2, 0))",
        Hold::Exclusive => "SELECT pg_advisory_xact_lock(hashtextextended($1 || $2, 0))",
    };
    sqlx::query(sql)
        .bind(prefix)
        .bind(key)
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// End every paged source's index lease, so a page fetched before an
/// erasure or an organization's purge — still carrying the name, or the
/// organization, that is going — cannot be written after it: the page's own
/// lease no longer settles, and the next tick reads its rows again from the
/// source. The docs carry no tenant and no person, so a refresh keeps its
/// lease. In a short transaction, never the one that deletes: the cursor
/// rows stay locked until it commits.
pub async fn revoke_leases(tx: &mut Scoped<'_, Maintenance<AgentLane>>) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE agent.cursors SET lease_id = NULL, leased_until = NULL
          WHERE lease_id IS NOT NULL AND source <> 'docs'",
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// A person's erasure, fenced on the organizations it reaches. Only its own
/// [`close_erasure`] lifts it: a later erase of the same person that put its
/// own fence down keeps that one down.
#[derive(Debug)]
pub struct Fence {
    pub id: Uuid,
    /// Sorted, each once.
    pub organizations: Vec<String>,
}

/// Fence a person's erasure on every organization it reaches — the
/// `organizations` given, and those an earlier erase of theirs reached —
/// and end the index leases, before [`erase_person`] scrubs.
///
/// ⚠ **Down before the scrub, and up only once it is done.** A turn still
/// answering may have read the person before the scrub took them. Its save
/// takes the organization's lock shared ([`erased_during`]): one that lands
/// before the fence is found by the scrub behind it, and one that lands
/// while the fence is down, or after it came up but began before, is
/// withheld. The earlier erase's organizations count because auth erases
/// again once the memberships are gone, when it can no longer name them.
///
/// ⚠ **The role's short lock wait, never a long one.** Each key is taken
/// while the ones before it are held, and every save in their organizations
/// queues behind those; a fence that waited long on one key would hold them
/// past the saves' own wait, and lose their answers. One that cannot take a
/// key in two seconds gives back the ones it took, and is asked again.
pub async fn fence_erasure(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    user_id: &UserId,
    organizations: &[String],
) -> sqlx::Result<Fence> {
    let mut reached: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT organization_id FROM agent.erasures WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(tx.conn())
    .await?;
    reached.extend_from_slice(organizations);
    reached.sort_unstable();
    reached.dedup();
    for organization in &reached {
        lock_key(tx, ERASURE_KEY, organization, Hold::Exclusive).await?;
    }
    let id = Uuid::new_v4();
    // Rows of this fence's own, beside any other erase's: an earlier close
    // still holds the turns begun before it, and another erase of the person
    // running at the same time keeps its fence down until it lifts it.
    sqlx::query(
        "INSERT INTO agent.erasures (user_id, organization_id, fence_id, fenced_at)
         SELECT $1, organization_id, $3, clock_timestamp()
           FROM unnest($2::text[]) AS reached (organization_id)",
    )
    .bind(user_id)
    .bind(&reached)
    .bind(id)
    .execute(tx.conn())
    .await?;
    revoke_leases(tx).await?;
    Ok(Fence {
        id,
        organizations: reached,
    })
}

/// Lift a fence once its scrub has committed, or failed: answers in its
/// organizations are kept again, except one whose turn began before now.
/// With the role's short lock wait, as [`fence_erasure`] takes its keys.
pub async fn close_erasure(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    user_id: &UserId,
    fence: &Fence,
) -> sqlx::Result<()> {
    for organization in &fence.organizations {
        lock_key(tx, ERASURE_KEY, organization, Hold::Exclusive).await?;
    }
    sqlx::query(
        "UPDATE agent.erasures SET scrubbed_at = clock_timestamp()
          WHERE user_id = $1 AND fence_id = $2",
    )
    .bind(user_id)
    .bind(fence.id)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Everything held of one person: their conversations and every passage
/// that is theirs, is about them, or quotes them. Behind `fence`, each
/// statement in a transaction of its own — the reads of whole tables in
/// batches of rows — so none holds its rows locked, or runs, long: the fence
/// stands across all of them, put down again as each commits. Answers how
/// many rows went, and every organization the name was scrubbed in: the
/// fence's, and those the scrub found the person in.
///
/// Other people's answers quoted them from the member list, with no id to
/// find them by, so those are found by the text: their messages, the
/// titles those cite and conversation titles rewritten, their passages gone
/// from the index (a copy; its sources keep what they keep). `addresses`
/// and `names` are case-insensitive patterns.
///
/// - **An address is theirs alone**, so it is scrubbed wherever it is.
/// - **A name is not**: it is scrubbed only in the organizations it could
///   have been quoted in — the fence's, and every one where they asked the
///   agent (their conversations go for 90 days after their last use), where
///   their address turns up, or where a passage about them does — and never
///   in the docs.
///
/// ⚠ **Every organization found is recorded in the transaction that found
/// it** (`record_reached`). A statement that commits changes what the next
/// attempt can find — an address once scrubbed is not found again — so one
/// held only here, by an erase that then failed, was never fenced or
/// scrubbed by name again.
///
/// ⚠ **Conversations before passages**, as [`delete_conversation`] does: a
/// remembered exchange holds its conversation while it writes, so deleting
/// the row first waits for it, and the passages swept after include it.
pub async fn erase_person(
    pool: &PgPool,
    user_id: &UserId,
    fence: &Fence,
    addresses: &[String],
    names: &[String],
) -> sqlx::Result<(u64, Vec<String>)> {
    let mut organizations = fence.organizations.clone();
    let mut gone = 0u64;
    for sql in [
        "DELETE FROM agent.conversations WHERE user_id = $1 RETURNING organization_id",
        "DELETE FROM agent.chunks WHERE user_id = $1 OR subject_user_id = $1
         RETURNING organization_id",
    ] {
        let mut tx = scrub_step(pool).await?;
        let found: Vec<Option<String>> = sqlx::query_scalar(sql)
            .bind(user_id)
            .fetch_all(tx.conn())
            .await?;
        gone += u64::try_from(found.len()).unwrap_or(u64::MAX);
        let found: Vec<String> = found.into_iter().flatten().collect();
        record_reached(&mut tx, user_id, fence.id, &found).await?;
        step_done(tx, user_id, fence.id).await?;
        organizations.extend(found);
    }
    for address in addresses {
        for batched in Batched::ALL {
            let (went, found) =
                scrub_in_batches(pool, batched, address, Reach::Everywhere, user_id, fence.id)
                    .await?;
            gone += went;
            organizations.extend(found);
        }
    }
    organizations.sort_unstable();
    organizations.dedup();
    for name in names {
        for organization in &organizations {
            for batched in Batched::ALL {
                let reach = Reach::In(organization);
                let (went, _) =
                    scrub_in_batches(pool, batched, name, reach, user_id, fence.id).await?;
                gone += went;
            }
        }
    }
    Ok((gone, organizations))
}

/// One statement of a scrub's, in the lane and a transaction of its own,
/// waiting thirty seconds a lock and a minute a statement, past the role's
/// two and five. The scrub reads text no index narrows and rewrites rows a
/// save or another erasure may hold, behind a fence rather than inside a
/// request; one that always timed out would never reach the identity it
/// exists to delete. It takes no key, so no save waits on it.
async fn scrub_step(pool: &PgPool) -> sqlx::Result<Scoped<'_, Maintenance<AgentLane>>> {
    let mut tx = maintenance_scope(pool, AgentLane).await?;
    sqlx::query("SET LOCAL lock_timeout = '30s'")
        .execute(tx.conn())
        .await?;
    sqlx::query("SET LOCAL statement_timeout = '1min'")
        .execute(tx.conn())
        .await?;
    Ok(tx)
}

/// Commit one step of a scrub, putting its fence down again in the same
/// commit: the fence holds answers for `OPEN_FENCE_MINUTES` past the last
/// step however long the scrub runs, and lapses once nothing steps.
async fn step_done(
    mut tx: Scoped<'_, Maintenance<AgentLane>>,
    user_id: &UserId,
    fence_id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE agent.erasures SET fenced_at = clock_timestamp()
          WHERE user_id = $1 AND fence_id = $2
            AND (scrubbed_at IS NULL OR scrubbed_at < fenced_at)",
    )
    .bind(user_id)
    .bind(fence_id)
    .execute(tx.conn())
    .await?;
    tx.commit().await
}

/// Rows of a table one statement of a scrub reads.
const SCRUB_BATCH: i64 = 20_000;

/// The last id there is: the end of a scrub's last batch, so every batch is
/// a range with two ends and its index scan stops at the second.
const LAST_ID: Uuid = Uuid::from_u128(u128::MAX);

/// Where a scrub looks for its pattern.
#[derive(Debug, Clone, Copy)]
enum Reach<'a> {
    /// Every organization's rows: an address is the person's alone.
    Everywhere,
    /// One organization's rows, through its `(organization_id, id)` index: a
    /// name is many people's, and goes only where they could have been
    /// quoted, at a cost that follows those organizations, not the index.
    In(&'a str),
}

/// A scrub's three reads of a table, each taken a batch of rows at a time
/// in id order — every row there is, or one organization's — so no
/// statement outlasts its minute however large the table grows.
#[derive(Debug, Clone, Copy)]
enum Batched {
    /// Passages quoting the pattern go.
    Passages,
    /// Messages quoting it, and the titles they cite, are rewritten.
    Messages,
    /// Conversation titles quoting it are rewritten.
    Titles,
}

impl Batched {
    /// Passages before messages and titles, as the scrub has always gone.
    const ALL: [Self; 3] = [Self::Passages, Self::Messages, Self::Titles];

    /// Where the batch after `$1` ends: the id `$2` rows on — in the
    /// organization `$3`, reaching [`Reach::In`] it — or none for the last.
    const fn end(self, reach: Reach<'_>) -> &'static str {
        match (self, reach) {
            (Self::Passages, Reach::Everywhere) => {
                "SELECT id FROM agent.chunks WHERE id > $1 ORDER BY id OFFSET $2 LIMIT 1"
            }
            (Self::Passages, Reach::In(_)) => {
                "SELECT id FROM agent.chunks WHERE organization_id = $3 AND id > $1
                  ORDER BY id OFFSET $2 LIMIT 1"
            }
            (Self::Messages, Reach::Everywhere) => {
                "SELECT id FROM agent.messages WHERE id > $1 ORDER BY id OFFSET $2 LIMIT 1"
            }
            (Self::Messages, Reach::In(_)) => {
                "SELECT id FROM agent.messages WHERE organization_id = $3 AND id > $1
                  ORDER BY id OFFSET $2 LIMIT 1"
            }
            (Self::Titles, Reach::Everywhere) => {
                "SELECT id FROM agent.conversations WHERE id > $1 ORDER BY id OFFSET $2 LIMIT 1"
            }
            (Self::Titles, Reach::In(_)) => {
                "SELECT id FROM agent.conversations WHERE organization_id = $3 AND id > $1
                  ORDER BY id OFFSET $2 LIMIT 1"
            }
        }
    }

    /// The statement over the batch `($2, $3]` matching the pattern `$1` —
    /// in the organization `$4`, reaching [`Reach::In`] it; a rewrite binds
    /// the replacement after. Answers the organizations it touched.
    const fn statement(self, reach: Reach<'_>) -> &'static str {
        match (self, reach) {
            (Self::Passages, Reach::Everywhere) => {
                "DELETE FROM agent.chunks
                  WHERE id > $2 AND id <= $3
                    AND source <> 'docs' AND (body ~* $1 OR title ~* $1)
                 RETURNING organization_id"
            }
            (Self::Passages, Reach::In(_)) => {
                "DELETE FROM agent.chunks
                  WHERE organization_id = $4 AND id > $2 AND id <= $3
                    AND source <> 'docs' AND (body ~* $1 OR title ~* $1)
                 RETURNING organization_id"
            }
            (Self::Messages, Reach::Everywhere) => {
                "UPDATE agent.messages
                    SET content = regexp_replace(content, $1, $4, 'gi'),
                        citations = (SELECT COALESCE(jsonb_agg(
                                         CASE WHEN jsonb_typeof(cited->'title') = 'string'
                                              THEN jsonb_set(cited, '{title}',
                                                       to_jsonb(regexp_replace(cited->>'title', $1, $4, 'gi')))
                                              ELSE cited END
                                         ORDER BY n), '[]'::jsonb)
                                       FROM jsonb_array_elements(citations) WITH ORDINALITY AS c (cited, n))
                  WHERE id > $2 AND id <= $3
                    AND (content ~* $1 OR citations::text ~* $1)
                 RETURNING organization_id"
            }
            (Self::Messages, Reach::In(_)) => {
                "UPDATE agent.messages
                    SET content = regexp_replace(content, $1, $5, 'gi'),
                        citations = (SELECT COALESCE(jsonb_agg(
                                         CASE WHEN jsonb_typeof(cited->'title') = 'string'
                                              THEN jsonb_set(cited, '{title}',
                                                       to_jsonb(regexp_replace(cited->>'title', $1, $5, 'gi')))
                                              ELSE cited END
                                         ORDER BY n), '[]'::jsonb)
                                       FROM jsonb_array_elements(citations) WITH ORDINALITY AS c (cited, n))
                  WHERE organization_id = $4 AND id > $2 AND id <= $3
                    AND (content ~* $1 OR citations::text ~* $1)
                 RETURNING organization_id"
            }
            (Self::Titles, Reach::Everywhere) => {
                "UPDATE agent.conversations
                    SET title = left(regexp_replace(title, $1, $4, 'gi'), 200)
                  WHERE id > $2 AND id <= $3 AND title ~* $1
                 RETURNING organization_id"
            }
            (Self::Titles, Reach::In(_)) => {
                "UPDATE agent.conversations
                    SET title = left(regexp_replace(title, $1, $5, 'gi'), 200)
                  WHERE organization_id = $4 AND id > $2 AND id <= $3 AND title ~* $1
                 RETURNING organization_id"
            }
        }
    }
}

/// One read of a table for `pattern`, as far as `reach`, a batch of rows at
/// a time, each batch a step of its own that records the organizations it
/// touched. Answers how many rows it touched, and those organizations.
async fn scrub_in_batches(
    pool: &PgPool,
    batched: Batched,
    pattern: &str,
    reach: Reach<'_>,
    user_id: &UserId,
    fence_id: Uuid,
) -> sqlx::Result<(u64, Vec<String>)> {
    let mut touched = 0u64;
    let mut reached = Vec::new();
    let mut after = Uuid::nil();
    loop {
        let mut tx = scrub_step(pool).await?;
        let end = sqlx::query_scalar(batched.end(reach))
            .bind(after)
            .bind(SCRUB_BATCH - 1);
        let end = match reach {
            Reach::Everywhere => end,
            Reach::In(organization) => end.bind(organization),
        };
        let end: Option<Uuid> = end.fetch_optional(tx.conn()).await?;
        let query = sqlx::query_scalar::<_, Option<String>>(batched.statement(reach))
            .bind(pattern)
            .bind(after)
            .bind(end.unwrap_or(LAST_ID));
        let query = match reach {
            Reach::Everywhere => query,
            Reach::In(organization) => query.bind(organization),
        };
        let query = match batched {
            Batched::Passages => query,
            Batched::Messages | Batched::Titles => query.bind(FORMER_MEMBER),
        };
        let found: Vec<Option<String>> = query.fetch_all(tx.conn()).await?;
        touched += u64::try_from(found.len()).unwrap_or(u64::MAX);
        let found: Vec<String> = found.into_iter().flatten().collect();
        record_reached(&mut tx, user_id, fence_id, &found).await?;
        step_done(tx, user_id, fence_id).await?;
        reached.extend(found);
        match end {
            Some(end) => after = end,
            None => return Ok((touched, reached)),
        }
    }
}

/// Record organizations a scrub found the person in, in the transaction
/// that found them: fenced with this erase, already closed — the rows are
/// scrubbed as they commit — so nothing begun since is held back, and the
/// next erase of theirs fences them too. Put down and lifted at one instant,
/// read once: two readings of the clock could leave the lift before the
/// fence, which reads as a fence still down.
async fn record_reached(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    user_id: &UserId,
    fence_id: Uuid,
    found: &[String],
) -> sqlx::Result<()> {
    if found.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "WITH clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         INSERT INTO agent.erasures (user_id, organization_id, fence_id, fenced_at, scrubbed_at)
         SELECT $1, found.organization_id, $2, clock.at, clock.at
           FROM (SELECT DISTINCT unnest($3::text[])) AS found (organization_id), clock
         ON CONFLICT (user_id, organization_id, fence_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(fence_id)
    .bind(found)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Up to `chunk` of an organization's conversations, their messages with
/// them, and up to `chunk` of its passages, after [`revoke_leases`]
/// committed. Taken a chunk a transaction, as a large organization's rows
/// in one statement outlasted the role's five seconds on every attempt, and
/// the organization was never deleted.
pub async fn purge_organization(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    organization_id: &OrganizationId,
    chunk: i64,
) -> sqlx::Result<u64> {
    let mut gone = 0;
    for sql in [
        "DELETE FROM agent.conversations
          WHERE id IN (SELECT id FROM agent.conversations WHERE organization_id = $1 LIMIT $2)",
        "DELETE FROM agent.chunks
          WHERE id IN (SELECT id FROM agent.chunks WHERE organization_id = $1 LIMIT $2)",
    ] {
        gone += sqlx::query(sql)
            .bind(organization_id)
            .bind(chunk)
            .execute(tx.conn())
            .await?
            .rows_affected();
    }
    Ok(gone)
}

/// The next `limit` pairs of an organization and a project that rows are
/// held under, after `after`, in order. A loose scan of each table's tenant
/// index — one probe a pair — rather than a read of every row.
pub async fn held_pairs(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    after: (&str, &str),
    limit: i64,
) -> sqlx::Result<Vec<(String, String)>> {
    sqlx::query_as(
        "WITH RECURSIVE
         chunk_pairs (organization_id, project_id) AS (
             (SELECT organization_id, project_id FROM agent.chunks
               WHERE project_id IS NOT NULL AND (organization_id, project_id) > ($1, $2)
               ORDER BY organization_id, project_id LIMIT 1)
           UNION ALL
             SELECT next.organization_id, next.project_id
               FROM chunk_pairs held,
                    LATERAL (SELECT organization_id, project_id FROM agent.chunks
                              WHERE project_id IS NOT NULL
                                AND (organization_id, project_id)
                                    > (held.organization_id, held.project_id)
                              ORDER BY organization_id, project_id LIMIT 1) next
         ),
         conversation_pairs (organization_id, project_id) AS (
             (SELECT organization_id, project_id FROM agent.conversations
               WHERE (organization_id, project_id) > ($1, $2)
               ORDER BY organization_id, project_id LIMIT 1)
           UNION ALL
             SELECT next.organization_id, next.project_id
               FROM conversation_pairs held,
                    LATERAL (SELECT organization_id, project_id FROM agent.conversations
                              WHERE (organization_id, project_id)
                                    > (held.organization_id, held.project_id)
                              ORDER BY organization_id, project_id LIMIT 1) next
         )
         SELECT organization_id, project_id
           FROM ((SELECT organization_id, project_id FROM chunk_pairs LIMIT $3)
                 UNION
                 (SELECT organization_id, project_id FROM conversation_pairs LIMIT $3)) pairs
          ORDER BY organization_id, project_id
          LIMIT $3",
    )
    .bind(after.0)
    .bind(after.1)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// Up to `chunk` conversations, and up to `chunk` passages, held of
/// `project` under `organization`, passing over rows another transaction
/// holds. Answers how many went.
pub async fn drop_held(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    organization: &str,
    project: &str,
    chunk: i64,
) -> sqlx::Result<u64> {
    let mut gone = 0;
    for sql in [
        "DELETE FROM agent.conversations
          WHERE id IN (SELECT id FROM agent.conversations
                        WHERE organization_id = $1 AND project_id = $2
                        LIMIT $3 FOR UPDATE SKIP LOCKED)",
        "DELETE FROM agent.chunks
          WHERE id IN (SELECT id FROM agent.chunks
                        WHERE organization_id = $1 AND project_id = $2
                        LIMIT $3 FOR UPDATE SKIP LOCKED)",
    ] {
        gone += sqlx::query(sql)
            .bind(organization)
            .bind(project)
            .bind(chunk)
            .execute(tx.conn())
            .await?
            .rows_affected();
    }
    Ok(gone)
}

/// Forget the erasure fences last put down, and last lifted, more than
/// `days` ago. A fence put down again keeps its row's last close, so the
/// later of the two says when the row was last of use.
pub async fn forget_fences(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    days: i32,
) -> sqlx::Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM agent.erasures
          WHERE GREATEST(fenced_at, scrubbed_at) < now() - make_interval(days => $1)",
    )
    .bind(days)
    .execute(tx.conn())
    .await?
    .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for source in Source::all() {
            match source {
                Source::Docs
                | Source::Audit
                | Source::Feed
                | Source::Delivery
                | Source::Conversation => {}
            }
        }
        for visibility in Visibility::all() {
            match visibility {
                Visibility::Everyone
                | Visibility::Audit
                | Visibility::OrganizationAdmin
                | Visibility::Author => {}
            }
        }
        for role in MessageRole::all() {
            match role {
                MessageRole::User | MessageRole::Assistant => {}
            }
        }
        listed_once(&Source::all());
        listed_once(&Visibility::all());
        listed_once(&MessageRole::all());
    }

    fn listed_once<T: fmt::Display>(all: &[T]) {
        let mut seen: Vec<String> = all.iter().map(ToString::to_string).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), all.len(), "all() repeats a variant");
    }

    #[test]
    fn from_str_round_trips_and_serde_matches_display() {
        round_trips(&Source::all());
        round_trips(&Visibility::all());
        round_trips(&MessageRole::all());
    }

    fn round_trips<T>(all: &[T])
    where
        T: Copy + PartialEq + fmt::Debug + fmt::Display + FromStr<Err = ParseEnumError> + Serialize,
    {
        for v in all {
            assert_eq!(v.to_string().parse::<T>(), Ok(*v));
            assert_eq!(serde_json::to_string(v).unwrap(), format!("\"{v}\""));
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        for word in ["Docs", "conversations", "chunk", ""] {
            assert!(word.parse::<Source>().is_err(), "parsed: {word:?}");
        }
        for word in ["Everyone", "owner", "organization-admin", ""] {
            assert!(word.parse::<Visibility>().is_err(), "parsed: {word:?}");
        }
        for word in ["User", "system", "tool", ""] {
            assert!(word.parse::<MessageRole>().is_err(), "parsed: {word:?}");
        }
    }
}
