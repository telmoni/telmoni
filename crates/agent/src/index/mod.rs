//! The index: what the other modules hold, pulled through their seams in
//! their own lanes, split, embedded and written here in this module's lane.
//! Nothing crosses a database role: auth reads audit events as auth, and
//! notifications its feed and deliveries as notifications.
//!
//! The loop runs in every replica of `serve`. Each source's cursor row is
//! leased for the length of a page, so replicas share the work without
//! embedding a page twice; a replica that finds the lease taken skips that
//! source this tick.

pub mod chunk;
pub mod conversations;
pub mod docs;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use telmoni_shared::db::tenant_session::{Maintenance, Scoped, maintenance_scope};
use telmoni_shared::seam::{ActivitySource, DocumentCursor, SourceDocument};
use telmoni_shared::{OrganizationId, ProjectId, TelmoniError, UserId};
use uuid::Uuid;

use crate::AppState;
use crate::db::{self, AgentLane, Lease, NewChunk, Source, Visibility};
use crate::embed::{BATCH, Embedder, literal};
use crate::model::{not_now, rate_limited, unavailable};

/// How often the loop looks for new rows.
const EVERY: Duration = Duration::from_secs(60);

/// How long after boot the first tick runs: past the startup probes.
const START_DELAY: Duration = Duration::from_secs(30);

/// How often the docs corpus is fetched again.
const DOCS_EVERY: Duration = Duration::from_hours(24);

/// How long after a failed fetch the next one runs: a docs site that does
/// not publish the corpus yet is one warning an hour, not one a minute.
const DOCS_RETRY: Duration = Duration::from_hours(1);

/// Rows per page: one lease's worth of embedding.
const PAGE: i64 = 64;

/// How long a replica holds a source's cursor unrenewed. The page it is
/// embedding puts the end off every third of this ([`holding`]), so a slow
/// page — a batch taken apart and texts cut down, an endpoint taking its
/// minute a request — keeps its lease, and only a replica gone quiet loses
/// it, to another that takes the page over this long after.
const LEASE_SECS: i32 = 15 * 60;

/// Pages one source takes per tick, so a backlog cannot starve the others.
const PAGES_PER_TICK: usize = 20;

/// How long one source pages per tick before the next takes its turn.
const SOURCE_TICK: Duration = Duration::from_mins(5);

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
    let mut width_checked = false;
    loop {
        ticker.tick().await;
        // Boot gives up on an endpoint that does not answer, so the width it
        // could not check is checked here, once the endpoint answers and
        // before anything is written with it. A wrong one stops the indexer
        // rather than warning on every write forever.
        if !width_checked && let Ok(embedder) = state.embedder() {
            match crate::boot::probe_width(embedder).await {
                Ok(crate::boot::Probe::Fits) => width_checked = true,
                Ok(crate::boot::Probe::Unanswered) => continue,
                Err(e) => {
                    tracing::error!(error = %e, "agent indexing stopped");
                    return;
                }
            }
        }
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
    let started = Instant::now();
    for _ in 0..PAGES_PER_TICK {
        // A source whose pages are slow — waiting out an endpoint's rate
        // limit, say — leaves the rest of its backlog to the next tick, so
        // the others and the docs are not held behind it.
        if started.elapsed() >= SOURCE_TICK {
            break;
        }
        let Some(lease) = lease(state, source).await? else {
            return Ok(taken);
        };
        match index_page(state, paged, embedder, &lease).await? {
            Page::Done => return Ok(taken),
            Page::Took { rows, full } => {
                taken += rows;
                if !full {
                    break;
                }
            }
        }
    }
    Ok(taken)
}

/// What one leased page came to.
enum Page {
    /// Nothing new, or the lease ran out and another replica's work stands.
    Done,
    Took {
        rows: usize,
        full: bool,
    },
}

/// Fetch, embed and write one page under a lease, handing the lease back
/// when anything fails. The fetch and the embedding hold no transaction of
/// this module's; the write, the lease's settling and the cursor's move are
/// one, settled last.
async fn index_page(
    state: &AppState,
    paged: Paged,
    embedder: &dyn Embedder,
    lease: &Lease,
) -> Result<Page, TelmoniError> {
    let source = paged.source();
    let after = match (lease.after_at, lease.after_id.clone()) {
        (Some(at), Some(id)) => Some(DocumentCursor { at, id }),
        _ => None,
    };
    let page = match fetch(state, paged, after.as_ref()).await {
        Ok(page) => page,
        Err(e) => {
            release(state, source, lease.lease_id).await;
            return Err(e);
        }
    };
    let Some(last) = page.last().map(|d| d.cursor.clone()) else {
        release(state, source, lease.lease_id).await;
        return Ok(Page::Done);
    };
    let full = i64::try_from(page.len()).is_ok_and(|n| n >= PAGE);
    let rows = page.len();
    let entries: Vec<Entry> = page.into_iter().map(Entry::from).collect();

    let written = holding(state, source, lease.lease_id, async {
        let prepared = prepare(&state.db, embedder, source, &entries).await?;
        let mut tx = maintenance_scope(&state.db, AgentLane).await?;
        store(&mut tx, source, &entries, &prepared).await?;
        if !db::settle_lease(&mut tx, source, lease.lease_id).await? {
            tx.rollback().await?;
            tracing::warn!(
                source = source.as_str(),
                "an agent index page lost its lease to another replica or an erasure; it is read again"
            );
            return Ok(Page::Done);
        }
        db::advance_cursor(&mut tx, source, last.at, &last.id).await?;
        tx.commit().await?;
        Ok(Page::Took { rows, full })
    })
    .await;
    match written {
        Some(Ok(page)) => Ok(page),
        // The lease went mid-page; whoever holds the cursor reads it again.
        None => Ok(Page::Done),
        Some(Err(e)) => {
            if embedder_failed(&e) {
                back_off(state, source, lease.lease_id).await;
            } else {
                release(state, source, lease.lease_id).await;
            }
            Err(e)
        }
    }
}

/// Lease a source's cursor in a transaction of its own.
pub(crate) async fn lease(state: &AppState, source: Source) -> Result<Option<Lease>, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AgentLane).await?;
    let lease = db::lease_cursor(&mut tx, source, LEASE_SECS).await?;
    tx.commit().await?;
    Ok(lease)
}

/// Run `work` under a source's lease, putting the lease's end off every
/// third of [`LEASE_SECS`] while it runs: `None` once the lease is no
/// longer this replica's — an erasure ended it, or this replica went quiet
/// and another took it — and `work` is stopped there, its transaction
/// rolled back, as its settle would find the lease gone.
///
/// ⚠ **Unrenewed, a page slower than its lease never landed** once two
/// replicas ran: the other took the lease and began the same page, its own
/// settle failed when the first one took it back, and so on. The renewal
/// runs beside the work, never in its way: one that held the work up while
/// it waited on the cursor row the work's own settle had locked waited for
/// good where no lock timeout ends a wait.
pub(crate) async fn holding<T>(
    state: &AppState,
    source: Source,
    lease_id: Uuid,
    work: impl Future<Output = T>,
) -> Option<T> {
    let renewing = async {
        let every = Duration::from_secs(u64::try_from(LEASE_SECS / 3).unwrap_or(300));
        let mut beat = tokio::time::interval(every);
        beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick is immediate; the lease was taken just now.
        beat.tick().await;
        loop {
            beat.tick().await;
            if !renew(state, source, lease_id).await {
                return;
            }
        }
    };
    tokio::select! {
        biased;
        out = work => Some(out),
        () = renewing => {
            tracing::warn!(
                source = source.as_str(),
                "an agent index lease ended while its page was being written; the page is read again"
            );
            None
        }
    }
}

/// How long a source rests after the embedder failed its page: the endpoint
/// is down, busy, or takes no real text, and asking again every minute — a
/// page taken apart costs a few hundred requests — helps nobody. Its lease
/// is kept, unworked, till then.
const BACKOFF_SECS: i32 = 5 * 60;

/// Keep a failed page's lease, unworked, for [`BACKOFF_SECS`], so no replica
/// reads the source again before then. Best effort, as [`release`] is.
async fn back_off(state: &AppState, source: Source, lease_id: Uuid) {
    let kept = async {
        let mut tx = maintenance_scope(&state.db, AgentLane).await?;
        db::renew_lease(&mut tx, source, lease_id, BACKOFF_SECS).await?;
        tx.commit().await
    }
    .await;
    if let Err(e) = kept {
        tracing::warn!(
            source = source.as_str(),
            error = %e,
            "an agent index lease was not kept to rest the source; it runs out on its own"
        );
    }
}

/// Whether a page failed because the embedder did, rather than a sibling or
/// the database.
const fn embedder_failed(error: &TelmoniError) -> bool {
    matches!(error, TelmoniError::AgentModelUnavailable { .. })
}

/// Put a lease's end off once: `false` when it is no longer this replica's,
/// so nothing asks again. A failure to ask is not that, and asks again.
async fn renew(state: &AppState, source: Source, lease_id: Uuid) -> bool {
    let renewed = async {
        let mut tx = maintenance_scope(&state.db, AgentLane).await?;
        let held = db::renew_lease(&mut tx, source, lease_id, LEASE_SECS).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(held)
    }
    .await;
    renewed.unwrap_or_else(|e| {
        tracing::warn!(
            source = source.as_str(),
            error = %e,
            "an agent index lease was not put off; asking again"
        );
        true
    })
}

/// Hand a lease back — its page had nothing to write, or failed — so the
/// next tick reads from the cursor again now rather than when the lease
/// runs out. Best effort: a lease not handed back only runs out.
pub(crate) async fn release(state: &AppState, source: Source, lease_id: Uuid) {
    let released = async {
        let mut tx = maintenance_scope(&state.db, AgentLane).await?;
        db::release_lease(&mut tx, source, lease_id).await?;
        tx.commit().await
    }
    .await;
    if let Err(e) = released {
        tracing::warn!(
            source = source.as_str(),
            error = %e,
            "an agent index lease was not handed back; it runs out on its own"
        );
    }
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

/// The passages of a page that need writing, each with its vector. Built
/// with no transaction open; [`store`] writes it in one.
pub struct Prepared {
    model: String,
    /// `(entry, part, content hash, vector literal)`, indexes into the page.
    passages: Vec<(usize, i32, String, String)>,
    /// `(entry, part)` whose text changed and was passed over: the row it
    /// had holds text superseded, which a search would quote as current.
    superseded: Vec<(usize, i32)>,
}

/// Embed the passages of a page whose text or model changed. Reads what is
/// indexed in a short transaction, then embeds with none open: a slow model
/// would otherwise hold one past the role's idle cut-off and have it killed.
pub async fn prepare(
    pool: &PgPool,
    embedder: &dyn Embedder,
    source: Source,
    entries: &[Entry],
) -> Result<Prepared, TelmoniError> {
    let ids: Vec<&str> = entries.iter().map(|e| e.source_id.as_str()).collect();
    let indexed = {
        let mut tx = maintenance_scope(pool, AgentLane).await?;
        let indexed = db::indexed(&mut tx, source, &ids).await?;
        tx.commit().await?;
        indexed
    };
    let current: HashMap<(&str, i32), (&str, &str)> = indexed
        .iter()
        .map(|i| {
            (
                (i.source_id.as_str(), i.part),
                (i.content_hash.as_str(), i.model.as_str()),
            )
        })
        .collect();

    // `(entry, part, content hash, text, whether its row holds other text)`.
    let mut pending: Vec<(usize, i32, String, String, bool)> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        for (part, passage) in (0i32..).zip(&entry.passages) {
            let hash = chunk::content_hash(
                &passage.title,
                &passage.body,
                passage.url.as_deref(),
                entry.visibility,
            );
            let indexed_as = current.get(&(entry.source_id.as_str(), part));
            let unchanged = indexed_as.is_some_and(|(h, m)| *h == hash && *m == embedder.model());
            if !unchanged {
                let changed = indexed_as.is_some_and(|(h, _)| *h != hash);
                pending.push((index, part, hash, passage.embedding_text(), changed));
            }
        }
    }

    let texts: Vec<String> = pending
        .iter()
        .map(|(_, _, _, text, _)| text.clone())
        .collect();
    // A changed passage of a row this model already indexed: the endpoint has
    // taken this source's real text, so one it now takes at no length is the
    // text's to answer for — on a docs refresh above all, which sends only
    // what changed and may have nothing else to send.
    let proven = indexed.iter().any(|i| i.model == embedder.model());
    let vectors = embed_each(embedder, &texts, proven).await?;
    let mut passages = Vec::with_capacity(pending.len());
    let mut superseded = Vec::new();
    for ((index, part, hash, _, changed), vector) in pending.into_iter().zip(vectors) {
        let Some(vector) = vector else {
            tracing::warn!(
                source = source.as_str(),
                source_id = entries.get(index).map_or("", |e| e.source_id.as_str()),
                part,
                "the embedder took one passage at no length; it stays out of the index"
            );
            // Its text changed: the row it had goes too. One only a new model
            // must embed keeps its row, found by its terms, for a reindex.
            if changed {
                superseded.push((index, part));
            }
            continue;
        };
        passages.push((index, part, hash, vector));
    }
    Ok(Prepared {
        model: embedder.model().to_owned(),
        passages,
        superseded,
    })
}

/// Embed every text: each one's vector as pgvector reads it, or `None` for
/// one passed over. `proven` when this model has embedded the source's real
/// text before — passages of it already indexed — so a text failing at every
/// length may be passed over on a page where nothing else embedded.
///
/// ⚠ **A text the endpoint fails is cut down, never dropped for the
/// endpoint failing.** One input a model will not take — too long for its
/// context, refused outright, or answered with a 500 by a provider that does
/// not say why — fails the whole request it is in. So:
/// - **A batch that fails is split in two** and each half asked again, down
///   to the one text that fails alone, which is asked once more whole: a
///   failure the endpoint had once, for no fault of the text, costs nothing.
///   Halving blames no text, so a batch the endpoint had no time or budget
///   for is split too.
/// - **A text that fails whole twice is cut** to half and asked again, and
///   so on: it embeds from its opening, which carries its title and first
///   lines. One that fails at every length, down to [`FLOOR`], is passed
///   over — and only when another text of the page embedded, or the source
///   is `proven`, which shows the endpoint takes real text. Otherwise the
///   page fails.
/// - **The endpoint must embed a probe after every failure**, or the page
///   fails and nothing is cut or passed over; and a text alone the endpoint
///   says "not now" to ([`not_now`]: no answer in time, an endpoint
///   unavailable) fails the page, as nothing about the text is to blame. A
///   rate limit is waited out first ([`RATE_PAUSE`]). A failed page rests
///   ([`BACKOFF_SECS`]) and is read again.
/// - **A vector no column takes** — another width, zeros, a NaN — is a
///   failure like any other, the probe's included.
async fn embed_each(
    embedder: &dyn Embedder,
    texts: &[String],
    proven: bool,
) -> Result<Vec<Option<String>>, TelmoniError> {
    let mut out: Vec<Option<String>> = vec![None; texts.len()];
    // What is left to embed, each range asked whole, the first on top.
    let mut ranges: Vec<Range<usize>> = (0..texts.len())
        .step_by(BATCH)
        .map(|start| start..texts.len().min(start + BATCH))
        .rev()
        .collect();
    // Texts that failed whole twice, the endpoint embedding the probe after
    // each: cut down once every range is through, so that what embeds whole
    // has, by then.
    let mut suspects = Vec::new();
    while let Some(range) = ranges.pop() {
        let Some(batch) = texts.get(range.clone()) else {
            continue;
        };
        let failed = match embed_paced(embedder, batch).await {
            Ok(vectors) => {
                for (slot, vector) in out.iter_mut().skip(range.start).zip(vectors) {
                    *slot = Some(vector);
                }
                continue;
            }
            Err(e) => e,
        };
        let [text] = batch else {
            if !endpoint_works(embedder, &failed, Sent::Batch).await {
                return Err(failed);
            }
            let middle = range.start + range.len() / 2;
            ranges.push(middle..range.end);
            ranges.push(range.start..middle);
            continue;
        };
        if !endpoint_works(embedder, &failed, Sent::Alone).await {
            return Err(failed);
        }
        match embed_paced(embedder, std::slice::from_ref(text)).await {
            Ok(vectors) => {
                if let Some(slot) = out.get_mut(range.start) {
                    *slot = vectors.into_iter().next();
                }
            }
            Err(e) => {
                if !endpoint_works(embedder, &e, Sent::Alone).await {
                    return Err(e);
                }
                suspects.push(range.start);
            }
        }
    }
    let none_embedded = || unavailable("the embeddings endpoint took none of a page's texts");
    let mut lost = 0;
    for index in suspects {
        let Some(text) = texts.get(index) else {
            continue;
        };
        match cut_down(embedder, text).await? {
            Some(vector) => {
                if let Some(slot) = out.get_mut(index) {
                    *slot = Some(vector);
                }
            }
            None => lost += 1,
        }
        // Text after text failing at every length with nothing of the page
        // embedded is an endpoint that takes the probe and no real text:
        // cutting down the rest would cost hundreds of requests to learn so.
        if lost >= LOST_BEFORE_GIVING_UP && out.iter().all(Option::is_none) {
            return Err(none_embedded());
        }
    }
    if lost > 0 && !proven && out.iter().all(Option::is_none) {
        return Err(none_embedded());
    }
    Ok(out)
}

/// Texts of a page failing at every length, with nothing of the page
/// embedded, after which the rest are not cut down too.
const LOST_BEFORE_GIVING_UP: usize = 4;

/// The failures a text has, the probe embedding after each, before it is
/// passed over: twice whole, then at each length it is cut to — or, one too
/// short to cut, whole once more — so a text the endpoint failed for no
/// fault of its own is not lost to a bad moment or two.
const TRIES: usize = 3;

/// The shortest a text is cut to: a title and a line, which any model's
/// context takes, so a text failing this short fails for what it says.
const FLOOR: usize = 128;

/// A text that failed whole twice, the probe embedding after each: cut to
/// half and asked again while it fails and the probe embeds, down to
/// [`FLOOR`]. Answers its vector, from as much of it as the endpoint took;
/// `None` once it has failed at its shortest and [`TRIES`] times in all; or
/// the error when the endpoint is what fails ([`endpoint_works`]).
async fn cut_down(embedder: &dyn Embedder, text: &str) -> Result<Option<String>, TelmoniError> {
    let mut text = text;
    let mut failures = 2;
    loop {
        if let Some(opening) = first_half(text) {
            text = opening;
        } else if failures >= TRIES {
            return Ok(None);
        }
        match embed_paced(embedder, &[text.to_owned()]).await {
            Ok(vectors) => return Ok(vectors.into_iter().next()),
            Err(e) => {
                if !endpoint_works(embedder, &e, Sent::Alone).await {
                    return Err(e);
                }
                failures += 1;
            }
        }
    }
}

/// How long a rate limit is waited out before the request is asked again:
/// a provider's window is a minute.
const RATE_PAUSE: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_secs(60)
};

/// How many times one request waits out a rate limit before it counts as
/// failed. A per-minute budget smaller than a page, or than the docs corpus,
/// is met by pacing — not by failing the page, which threw away what it had
/// embedded and met the same budget at its next try.
const RATE_WAITS: usize = 2;

/// [`embed_usable`], waiting out a rate limit as often as [`RATE_WAITS`]
/// allows.
async fn embed_paced(
    embedder: &dyn Embedder,
    texts: &[String],
) -> Result<Vec<String>, TelmoniError> {
    let mut waited = 0;
    loop {
        match embed_usable(embedder, texts).await {
            Err(e) if rate_limited(&e) && waited < RATE_WAITS => {
                tokio::time::sleep(RATE_PAUSE).await;
                waited += 1;
            }
            done => return done,
        }
    }
}

/// Embed `texts` in one request, each vector as pgvector reads it: a vector
/// no column takes fails the request, as an error status does.
async fn embed_usable(
    embedder: &dyn Embedder,
    texts: &[String],
) -> Result<Vec<String>, TelmoniError> {
    embedder
        .embed(texts)
        .await?
        .iter()
        .map(|vector| {
            literal(vector).map_err(|e| {
                unavailable(format!("embeddings answered a vector no column takes: {e}"))
            })
        })
        .collect()
}

/// What a failed request carried, for [`endpoint_works`].
#[derive(Debug, Clone, Copy)]
enum Sent {
    /// Several texts, which halving may yet get through.
    Batch,
    /// One text, with nothing left to halve.
    Alone,
}

/// After a request failed with `failed`, whether the endpoint itself works,
/// so the request can be split, asked again or cut — or the page fails,
/// nothing cut or passed over. A probe failing too is the endpoint; and so
/// is "not now" to a text sent alone ([`Sent::Alone`]), which no halving of
/// a batch can help.
async fn endpoint_works(embedder: &dyn Embedder, failed: &TelmoniError, sent: Sent) -> bool {
    !(matches!(sent, Sent::Alone) && not_now(failed)) && embeds_probe(embedder).await
}

/// The first half of `text`, by characters, or `None` once it is no longer
/// than [`FLOOR`].
fn first_half(text: &str) -> Option<&str> {
    let chars = text.chars().count();
    if chars <= FLOOR {
        return None;
    }
    let end = text
        .char_indices()
        .nth(chars / 2)
        .map_or(text.len(), |(at, _)| at);
    text.get(..end)
}

/// Whether the endpoint embeds a probe now: a text shaped as every text is —
/// a title, a blank line, a body — short enough for any model's context,
/// and never the same twice, so no cache in front of an endpoint that is
/// down can answer it. Asked after every failure and never remembered: what
/// the endpoint answered a minute ago says nothing of whether it works now.
/// Paced as every request is: on a budget of a few requests a minute, a probe
/// sent straight after a failure meets the limit the failure spent, and a
/// page holding one too-long text failed the same way every time.
async fn embeds_probe(embedder: &dyn Embedder) -> bool {
    let probe = format!(
        "Telmoni\n\nOrganizations, projects, and the people in them. {}",
        Uuid::new_v4()
    );
    embed_paced(embedder, &[probe]).await.is_ok()
}

/// Write a prepared page in the lane, then drop whatever a row shrank away
/// from, and the passages whose changed text was passed over.
pub async fn store(
    tx: &mut Scoped<'_, Maintenance<AgentLane>>,
    source: Source,
    entries: &[Entry],
    prepared: &Prepared,
) -> Result<(), TelmoniError> {
    for (index, part, hash, embedding) in &prepared.passages {
        let (Some(entry), Ok(at)) = (entries.get(*index), usize::try_from(*part)) else {
            continue;
        };
        let Some(passage) = entry.passages.get(at) else {
            continue;
        };
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
                embedding,
                model: &prepared.model,
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

    let (ids, parts): (Vec<&str>, Vec<i32>) = prepared
        .superseded
        .iter()
        .filter_map(|(index, part)| Some((entries.get(*index)?.source_id.as_str(), *part)))
        .unzip();
    db::drop_parts(tx, source, &ids, &parts).await?;
    Ok(())
}

/// `telmoni sweep agent-reindex`: give every passage a different model
/// embedded a vector from the configured one, a page at a time. Answers
/// how many it redid.
///
/// Pages by id, so a passage the model refuses is passed over rather than
/// read again forever, and embeds with no transaction open, as the loop does.
pub async fn reindex(state: &AppState) -> Result<u64, TelmoniError> {
    let embedder = state.embedder()?;
    let mut redone = 0u64;
    let mut after = Uuid::nil();
    // Once a page has embedded, the model takes these passages' real text,
    // and a last page of one it takes at no length does not fail the run.
    let mut proven = false;
    loop {
        let stale = {
            let mut tx = maintenance_scope(&state.db, AgentLane).await?;
            let stale = db::stale_chunks(&mut tx, embedder.model(), after, PAGE).await?;
            tx.commit().await?;
            stale
        };
        let Some(last) = stale.last() else {
            return Ok(redone);
        };
        after = last.id;
        let texts: Vec<String> = stale
            .iter()
            .map(|s| embedding_text(&s.title, &s.body))
            .collect();
        // A passage passed over here keeps its old vector, and the next
        // reindex reads it again.
        let vectors = embed_each(embedder, &texts, proven).await?;
        proven |= vectors.iter().any(Option::is_some);
        let mut tx = maintenance_scope(&state.db, AgentLane).await?;
        for (chunk, vector) in stale.iter().zip(vectors) {
            let Some(vector) = vector else {
                tracing::warn!(chunk = %chunk.id, "the embedder took one passage at no length; it keeps its old vector");
                continue;
            };
            if db::reembed(&mut tx, chunk.id, &vector, embedder.model()).await? {
                redone += 1;
            }
        }
        tx.commit().await?;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::{Value, json};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::method};

    use super::*;
    use crate::config::{EMBEDDING_DIMENSIONS, EmbeddingsConfig};
    use crate::embed::OpenAiEmbedder;

    /// The texts an embeddings request carries.
    fn inputs(request: &Request) -> Vec<String> {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        body.get("input")
            .and_then(Value::as_array)
            .map(|all| {
                all.iter()
                    .filter_map(|i| i.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A vector of the column's width for each input, its first value the
    /// input's length in characters, which [`length`] reads back to tell a
    /// text embedded whole from one cut down.
    fn embeds(inputs: &[String]) -> ResponseTemplate {
        let data: Vec<Value> = inputs
            .iter()
            .enumerate()
            .map(|(i, input)| {
                let mut vector = vec![0.0f32; EMBEDDING_DIMENSIONS];
                vector[0] = f32::from(u16::try_from(input.chars().count()).unwrap());
                vector[1] = 1.0;
                json!({ "index": i, "embedding": vector })
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "data": data }))
    }

    /// The length of the text a vector was made from, as [`embeds`] marks it.
    fn length(vector: &str) -> usize {
        let first = vector.trim_start_matches('[').split(',').next().unwrap();
        first.parse().unwrap()
    }

    /// A failure, asked again at once when it is a busy one.
    fn fails(status: u16) -> ResponseTemplate {
        ResponseTemplate::new(status).insert_header("retry-after", "0")
    }

    /// Embeds every input, but answers `status` to a request holding
    /// `poison` — to every request when `poison` is empty.
    struct Fails {
        poison: &'static str,
        status: u16,
    }

    impl Respond for Fails {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let inputs = inputs(request);
            if inputs.iter().any(|s| s.contains(self.poison)) {
                return fails(self.status);
            }
            embeds(&inputs)
        }
    }

    async fn embedder(fails: Fails) -> (MockServer, OpenAiEmbedder) {
        serving(fails).await
    }

    /// Embeds every input up to `max` characters, and answers `status` to a
    /// request holding a longer one: a provider that fails what its context
    /// cannot hold rather than truncating it.
    struct TooLong {
        max: usize,
        status: u16,
    }

    impl Respond for TooLong {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let inputs = inputs(request);
            if inputs.iter().any(|s| s.chars().count() > self.max) {
                return fails(self.status);
            }
            embeds(&inputs)
        }
    }

    /// Answers 500 to the requests numbered `down` (from 0), and embeds every
    /// other: an endpoint that goes down for a while, then comes back.
    struct Outage {
        down: std::ops::Range<usize>,
        status: u16,
        seen: AtomicUsize,
    }

    impl Outage {
        fn new(down: std::ops::Range<usize>) -> Self {
            Self::answering(down, 500)
        }

        /// Answers `status` while down.
        fn answering(down: std::ops::Range<usize>, status: u16) -> Self {
            Self {
                down,
                status,
                seen: AtomicUsize::new(0),
            }
        }
    }

    impl Respond for Outage {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            if self
                .down
                .contains(&self.seen.fetch_add(1, Ordering::SeqCst))
            {
                return fails(self.status);
            }
            embeds(&inputs(request))
        }
    }

    /// Answers 503 to a request of more than `max` texts, and embeds a
    /// smaller one: an endpoint with no time for a whole batch.
    struct Crowded {
        max: usize,
    }

    impl Respond for Crowded {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let inputs = inputs(request);
            if inputs.len() > self.max {
                return fails(503);
            }
            embeds(&inputs)
        }
    }

    /// Answers a page that is not JSON, with a 200, to a request holding
    /// `word`: a gateway's block page in place of vectors.
    struct Blocks {
        word: &'static str,
    }

    impl Respond for Blocks {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let inputs = inputs(request);
            if inputs.iter().any(|s| s.contains(self.word)) {
                return ResponseTemplate::new(200)
                    .set_body_raw("<html>blocked</html>", "text/html");
            }
            embeds(&inputs)
        }
    }

    /// Answers every input with a vector two wide: a model no column takes.
    struct Narrow;

    impl Respond for Narrow {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let data: Vec<Value> = (0..inputs(request).len())
                .map(|i| json!({ "index": i, "embedding": [1.0, 0.5] }))
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({ "data": data }))
        }
    }

    async fn serving(respond: impl Respond + 'static) -> (MockServer, OpenAiEmbedder) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(respond)
            .mount(&server)
            .await;
        let embedder = OpenAiEmbedder::new(EmbeddingsConfig {
            url: server.uri(),
            model: "m".into(),
            api_key: None,
            request_dimensions: false,
        })
        .unwrap();
        (server, embedder)
    }

    fn texts(all: &[&str]) -> Vec<String> {
        all.iter().map(|s| (*s).to_owned()).collect()
    }

    fn embedded(out: &[Option<String>]) -> Vec<bool> {
        out.iter().map(Option::is_some).collect()
    }

    /// ⚠ The maintainer's case: a provider that answers an input too long for
    /// its context with a 500 — or refuses it, or answers busy — has that
    /// text embedded from its opening, and the page lands; the source is not
    /// held behind its row.
    #[tokio::test]
    async fn a_text_too_long_for_the_endpoint_embeds_from_its_opening() {
        for status in [500, 413, 502] {
            let (_server, embedder) = serving(TooLong { max: 300, status }).await;
            let long = "x".repeat(1_000);
            let page = texts(&["one", long.as_str(), "two"]);
            let out = embed_each(&embedder, &page, false).await.unwrap();
            assert_eq!(embedded(&out), [true, true, true], "{status}");
            let cut = length(out[1].as_deref().unwrap());
            assert!(
                cut > 0 && cut <= 300,
                "{status}: embedded from {cut} characters"
            );
            assert_eq!(length(out[0].as_deref().unwrap()), 3, "{status}");
        }
    }

    /// ⚠ A text the endpoint failed once, for no fault of its own, embeds
    /// whole: it is asked again before anything is cut.
    #[tokio::test]
    async fn a_text_the_endpoint_fails_once_embeds_whole() {
        let (_server, embedder) = serving(Outage::new(0..1)).await;
        let out = embed_each(&embedder, &texts(&["one alone"]), false)
            .await
            .unwrap();
        assert_eq!(length(out[0].as_deref().unwrap()), 9, "cut for a blip");
    }

    /// A text the endpoint fails at every length, short or long, while it
    /// embeds the rest and the probe, is passed over: the page lands without
    /// it, rather than the source stalling behind it.
    #[tokio::test]
    async fn a_text_failing_at_every_length_is_passed_over_when_others_embed() {
        for status in [400, 500, 502] {
            let (_server, embedder) = embedder(Fails {
                poison: "poison",
                status,
            })
            .await;
            let long = format!("poison {}", "x".repeat(1_000));
            let page = texts(&["one", "poison pill", long.as_str(), "two"]);
            let out = embed_each(&embedder, &page, false).await.unwrap();
            assert_eq!(embedded(&out), [true, false, false, true], "{status}");
        }
    }

    /// ⚠ Alone on its page, a text failing at every length fails the page
    /// rather than being passed over: nothing shows the endpoint takes real
    /// text, and a quiet source's pages are one row long.
    #[tokio::test]
    async fn a_lone_text_failing_at_every_length_fails_its_page() {
        let (_server, embedder) = embedder(Fails {
            poison: "poison",
            status: 500,
        })
        .await;
        assert!(
            embed_each(&embedder, &texts(&["poison pill"]), false)
                .await
                .is_err()
        );
    }

    /// An endpoint failing or refusing everything, the probe included, is an
    /// error and passes no text over.
    #[tokio::test]
    async fn an_endpoint_that_fails_everything_passes_no_text_over() {
        for status in [400, 413, 500, 503] {
            let (_server, embedder) = embedder(Fails { poison: "", status }).await;
            assert!(
                embed_each(&embedder, &texts(&["one", "two"]), false)
                    .await
                    .is_err(),
                "{status}"
            );
        }
    }

    /// ⚠ A text alone that the endpoint keeps saying "not now" to — here a
    /// rate limit, waited out and still there — fails the page: it is never
    /// cut or passed over for what is no fault of its own.
    #[tokio::test]
    async fn a_text_rate_limited_alone_fails_its_page() {
        let (_server, embedder) = embedder(Fails {
            poison: "poison",
            status: 429,
        })
        .await;
        let long = format!("poison {}", "x".repeat(1_000));
        assert!(
            embed_each(&embedder, &texts(&["one", long.as_str()]), true)
                .await
                .is_err()
        );
    }

    /// ⚠ A rate limit is waited out, not taken as a failure: the batch lands
    /// whole when the window lifts, nothing split or cut.
    #[tokio::test]
    async fn a_rate_limit_is_waited_out() {
        // One request's three tries are limited, then the window lifts.
        let (server, embedder) = serving(Outage::answering(0..3, 429)).await;
        let out = embed_each(&embedder, &texts(&["one", "two"]), false)
            .await
            .unwrap();
        assert_eq!(embedded(&out), [true, true]);
        assert_eq!(server.received_requests().await.unwrap().len(), 4);
    }

    /// ⚠ A batch the endpoint has no time for — a CPU-bound model, say,
    /// answering "not now" to 32 long passages — is split, as halving blames
    /// no text, and lands in smaller requests.
    #[tokio::test]
    async fn a_batch_the_endpoint_has_no_time_for_is_split() {
        let (_server, embedder) = serving(Crowded { max: 2 }).await;
        let out = embed_each(&embedder, &texts(&["a", "b", "c", "d", "e"]), false)
            .await
            .unwrap();
        assert!(out.iter().all(Option::is_some));
    }

    /// A whole answer that does not parse — a gateway's page for one text —
    /// is about what was sent, not the endpoint's state: the text is found
    /// and passed over, and the rest of the page lands.
    #[tokio::test]
    async fn a_body_that_does_not_parse_is_an_answer() {
        let (_server, embedder) = serving(Blocks { word: "blocked" }).await;
        let out = embed_each(&embedder, &texts(&["one", "blocked text", "two"]), false)
            .await
            .unwrap();
        assert_eq!(embedded(&out), [true, false, true]);
    }

    /// On a source this model has embedded before, a lone text failing at
    /// every length is passed over: the docs refresh sends only what
    /// changed, and may have nothing else to send.
    #[tokio::test]
    async fn a_lone_text_of_a_proven_source_is_passed_over() {
        let (_server, embedder) = embedder(Fails {
            poison: "poison",
            status: 500,
        })
        .await;
        let out = embed_each(&embedder, &texts(&["poison pill"]), true)
            .await
            .unwrap();
        assert_eq!(embedded(&out), [false]);
    }

    /// ⚠ An endpoint that goes down mid-page fails the page — nothing is
    /// passed over — and once it is back, the page embeds whole.
    #[tokio::test]
    async fn an_endpoint_down_mid_page_loses_no_text() {
        // The first batch lands; the second and the probe after it do not.
        let (_server, embedder) = serving(Outage::new(1..3)).await;
        let page: Vec<String> = (0..40).map(|i| format!("text {i}")).collect();
        assert!(embed_each(&embedder, &page, false).await.is_err());
        let out = embed_each(&embedder, &page, false).await.unwrap();
        assert!(out.iter().all(Option::is_some));
    }

    /// A model that answers vectors no column takes fails the page, the
    /// probe's vector failing with them.
    #[tokio::test]
    async fn a_vector_no_column_takes_fails_the_page() {
        let (_server, embedder) = serving(Narrow).await;
        assert!(
            embed_each(&embedder, &texts(&["one", "two"]), false)
                .await
                .is_err()
        );
    }

    /// An endpoint that embeds the probe and fails every real text is an
    /// error, not a page passed over.
    #[tokio::test]
    async fn a_page_none_of_whose_texts_embed_is_an_error() {
        let (_server, embedder) = embedder(Fails {
            poison: "page",
            status: 500,
        })
        .await;
        assert!(
            embed_each(&embedder, &texts(&["page one", "page two"]), false)
                .await
                .is_err()
        );
    }

    #[test]
    fn a_text_is_cut_at_a_character_not_a_byte() {
        let text = "é".repeat(FLOOR * 2);
        let half = first_half(&text).unwrap();
        assert_eq!(half.chars().count(), FLOOR);
        assert!(first_half(half).is_none(), "cut below the floor");
    }
}
