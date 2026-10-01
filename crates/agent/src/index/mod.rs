//! The index: what the other modules hold, pulled through their seams in
//! their own lanes, split, embedded and written here in this module's lane.
//! Nothing crosses a database role: auth reads audit events as auth, and
//! notifications its feed and deliveries as notifications.
//!
//! The loop runs in every replica of `serve`. Each source's cursor row is
//! locked `FOR UPDATE SKIP LOCKED` for the length of a page, so replicas
//! share the work without reading a page twice; a replica that finds the
//! row taken skips that source this tick.

pub mod chunk;
pub mod conversations;
pub mod docs;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use telmoni_shared::db::tenant_session::{Maintenance, Scoped, maintenance_scope};
use telmoni_shared::seam::{ActivitySource, DocumentCursor, SourceDocument};
use telmoni_shared::{OrganizationId, ProjectId, TelmoniError, UserId};

use crate::AppState;
use crate::db::{self, AgentLane, NewChunk, Source, Visibility};
use crate::embed::{Embedder, literal};

/// How often the loop looks for new rows.
const EVERY: Duration = Duration::from_secs(60);

/// How long after boot the first tick runs: past the startup probes.
const START_DELAY: Duration = Duration::from_secs(30);

/// How often the docs corpus is fetched again.
const DOCS_EVERY: Duration = Duration::from_hours(24);

/// How long after a failed fetch the next one runs: a docs site that does
/// not publish the corpus yet is one warning an hour, not one a minute.
const DOCS_RETRY: Duration = Duration::from_hours(1);

/// Rows per page. A page is embedded while its cursor row is locked, so it
/// stays small enough to finish well inside the role's idle cut-off.
const PAGE: i64 = 64;

/// Pages one source takes per tick, so a backlog cannot starve the others.
const PAGES_PER_TICK: usize = 20;

/// The sources the loop pages through a sibling's seam. The docs come whole
/// from one file and conversations as they are answered, so neither pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paged {
    Audit,
    Feed,
    Delivery,
}

impl Paged {
    pub const ALL: [Self; 3] = [Self::Audit, Self::Feed, Self::Delivery];

    /// The source its cursor row and its passages carry.
    #[must_use]
    pub const fn source(self) -> Source {
        match self {
            Self::Audit => Source::Audit,
            Self::Feed => Source::Feed,
            Self::Delivery => Source::Delivery,
        }
    }
}

/// One row of a source, split into the passages that get a vector each.
#[derive(Debug, Clone)]
pub struct Entry {
    pub source_id: String,
    pub organization_id: Option<OrganizationId>,
    pub project_id: Option<ProjectId>,
    pub user_id: Option<UserId>,
    pub subject_user_id: Option<UserId>,
    pub visibility: Visibility,
    pub created_at: DateTime<Utc>,
    pub passages: Vec<Passage>,
}

/// One passage as it is written.
#[derive(Debug, Clone)]
pub struct Passage {
    pub title: String,
    pub body: String,
    pub url: Option<String>,
}

impl Passage {
    #[must_use]
    pub fn embedding_text(&self) -> String {
        embedding_text(&self.title, &self.body)
    }
}

/// What the embedding reads: the title carries the passage's subject even
/// when the body never names it. The indexer, the re-embed and a remembered
/// exchange all read it this way, so one model's vectors stay comparable.
#[must_use]
pub fn embedding_text(title: &str, body: &str) -> String {
    format!("{title}\n\n{body}")
}

impl From<SourceDocument> for Entry {
    fn from(doc: SourceDocument) -> Self {
        let passages = chunk::split(&doc.body)
            .into_iter()
            .map(|p| Passage {
                title: match p.heading {
                    Some(heading) => format!("{} — {heading}", doc.title),
                    None => doc.title.clone(),
                },
                body: p.text,
                url: Some(doc.url.clone()),
            })
            .collect();
        Self {
            source_id: doc.source_id,
            organization_id: Some(doc.organization_id),
            project_id: doc.project_id,
            user_id: None,
            subject_user_id: doc.subject_user_id,
            visibility: doc.audience.into(),
            created_at: doc.created_at,
            passages,
        }
    }
}

/// The loop, forever. A failed tick is logged and the next one retries from
/// the cursor, which only moves when a page's passages are written.
pub async fn run(state: Arc<AppState>) {
    tokio::time::sleep(START_DELAY).await;
    let mut ticker = tokio::time::interval(EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut docs_due = Instant::now();
    loop {
        ticker.tick().await;
        for paged in Paged::ALL {
            let source = paged.source().as_str();
            match index_source(&state, paged).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(source, rows = n, "agent index updated"),
                Err(e) => tracing::warn!(
                    source,
                    error = %e,
                    "agent indexing failed; the next tick retries from the cursor"
                ),
            }
        }
        if Instant::now() >= docs_due {
            match docs::refresh(&state).await {
                Ok(()) => docs_due = Instant::now() + DOCS_EVERY,
                Err(e) => {
                    docs_due = Instant::now() + DOCS_RETRY;
                    tracing::warn!(error = %e, "agent docs refresh failed; retrying in an hour");
                }
            }
        }
    }
}

/// Page one source forward from its cursor. Answers how many rows it took.
pub async fn index_source(state: &AppState, paged: Paged) -> Result<usize, TelmoniError> {
    let embedder = state.embedder()?;
    let source = paged.source();
    let mut taken = 0;
    for _ in 0..PAGES_PER_TICK {
        let mut tx = maintenance_scope(&state.db, AgentLane).await?;
        let Some(cursor) = db::lock_cursor(&mut tx, source).await? else {
            tx.commit().await?;
            return Ok(taken);
        };
        let after = match (cursor.after_at, cursor.after_id) {
            (Some(at), Some(id)) => Some(DocumentCursor { at, id }),
            _ => None,
        };
        let page = fetch(state, paged, after.as_ref()).await?;
        let Some(last) = page.last().map(|d| d.cursor.clone()) else {
            tx.commit().await?;
            return Ok(taken);
        };
        let full = i64::try_from(page.len()).is_ok_and(|n| n >= PAGE);
        taken += page.len();
        let entries: Vec<Entry> = page.into_iter().map(Entry::from).collect();
        write(&mut tx, embedder, source, &entries).await?;
        db::advance_cursor(&mut tx, source, last.at, &last.id).await?;
        tx.commit().await?;
        if !full {
            break;
        }
    }
    Ok(taken)
}

async fn fetch(
    state: &AppState,
    paged: Paged,
    after: Option<&DocumentCursor>,
) -> Result<Vec<SourceDocument>, TelmoniError> {
    match paged {
        Paged::Audit => state.auth.audit_documents(after, PAGE).await,
        Paged::Feed => {
            state
                .notifications
                .activity_documents(ActivitySource::Feed, after, PAGE)
                .await
        }
        Paged::Delivery => {
            state
                .notifications
                .activity_documents(ActivitySource::Delivery, after, PAGE)
                .await
        }
    }
}

/// Write a page of one source's rows in the lane: embed only the passages
/// whose text or model changed, then drop whatever a row shrank away from.
pub async fn write(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    embedder: &dyn Embedder,
    source: Source,
    entries: &[Entry],
) -> Result<(), TelmoniError> {
    let ids: Vec<&str> = entries.iter().map(|e| e.source_id.as_str()).collect();
    let indexed = db::indexed(tx, source, &ids).await?;
    let current: HashMap<(&str, i32), (&str, &str)> = indexed
        .iter()
        .map(|i| {
            (
                (i.source_id.as_str(), i.part),
                (i.content_hash.as_str(), i.model.as_str()),
            )
        })
        .collect();

    let mut pending: Vec<(&Entry, i32, &Passage, String)> = Vec::new();
    for entry in entries {
        for (part, passage) in (0i32..).zip(&entry.passages) {
            let hash = chunk::content_hash(
                &passage.title,
                &passage.body,
                passage.url.as_deref(),
                entry.visibility.as_str(),
            );
            let unchanged = current
                .get(&(entry.source_id.as_str(), part))
                .is_some_and(|(h, m)| *h == hash && *m == embedder.model());
            if !unchanged {
                pending.push((entry, part, passage, hash));
            }
        }
    }

    let texts: Vec<String> = pending
        .iter()
        .map(|(_, _, p, _)| p.embedding_text())
        .collect();
    let vectors = if texts.is_empty() {
        Vec::new()
    } else {
        embedder.embed(&texts).await?
    };
    for ((entry, part, passage, hash), vector) in pending.iter().zip(vectors) {
        let embedding = literal(&vector)?;
        db::upsert_chunk(
            tx,
            &NewChunk {
                organization_id: entry.organization_id.as_ref(),
                project_id: entry.project_id.as_ref(),
                user_id: entry.user_id.as_ref(),
                subject_user_id: entry.subject_user_id.as_ref(),
                source,
                source_id: &entry.source_id,
                part: *part,
                visibility: entry.visibility,
                title: &passage.title,
                body: &passage.body,
                url: passage.url.as_deref(),
                content_hash: hash,
                embedding: &embedding,
                model: embedder.model(),
                source_created_at: entry.created_at,
            },
        )
        .await?;
    }

    let mut seen = HashSet::new();
    let (ids, parts): (Vec<&str>, Vec<i32>) = entries
        .iter()
        .filter(|entry| seen.insert(entry.source_id.as_str()))
        .map(|entry| {
            let parts = i32::try_from(entry.passages.len()).unwrap_or(i32::MAX);
            (entry.source_id.as_str(), parts)
        })
        .unzip();
    db::drop_parts_past(tx, source, &ids, &parts).await?;
    Ok(())
}

/// `telmoni sweep agent-reindex`: give every passage a different model
/// embedded a vector from the configured one, a page at a time. Answers
/// how many it redid.
pub async fn reindex(state: &AppState) -> Result<u64, TelmoniError> {
    let embedder = state.embedder()?;
    let mut redone = 0u64;
    loop {
        let mut tx = maintenance_scope(&state.db, AgentLane).await?;
        let stale = db::stale_chunks(&mut tx, embedder.model(), PAGE).await?;
        if stale.is_empty() {
            tx.commit().await?;
            return Ok(redone);
        }
        let texts: Vec<String> = stale
            .iter()
            .map(|s| embedding_text(&s.title, &s.body))
            .collect();
        let vectors = embedder.embed(&texts).await?;
        for (chunk, vector) in stale.iter().zip(vectors) {
            db::reembed(&mut tx, chunk.id, &literal(&vector)?, embedder.model()).await?;
            redone += 1;
        }
        tx.commit().await?;
    }
}
