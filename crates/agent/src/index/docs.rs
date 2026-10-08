//! The customer docs, from the one text file the console publishes
//! (`/llms-full.txt`, or whatever `DOCS_CORPUS_URL` names): every page, each
//! opening with its `# Title` line and a `Source: <url>` line under it. The
//! console writes a path there (`/docs/...`), which is kept as it is, so an
//! answer's link stays on the console's own origin whichever host serves it.
//! A page is split at its headings and each passage links to its own anchor.
//! Only passages whose text changed are embedded again, and pages that left
//! the file leave the index.

use std::collections::HashSet;
use std::time::Duration;

use chrono::Utc;
use sha2::{Digest, Sha256};
use telmoni_shared::TelmoniError;
use telmoni_shared::db::tenant_session::maintenance_scope;

use super::{Entry, Passage, chunk, holding, lease, prepare, release, store};
use crate::AppState;
use crate::db::{self, AgentLane, Source, Visibility};
use crate::model::{error_kind, unavailable};

const SOURCE: Source = Source::Docs;

/// The largest corpus taken: the docs are a few hundred kilobytes, and a
/// misconfigured URL must not stream a gigabyte into memory.
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// Fetch the corpus and index what changed. Nothing to do when no URL is
/// configured, when another replica holds the docs cursor, or when the file
/// is the one last indexed.
pub async fn refresh(state: &AppState) -> Result<(), TelmoniError> {
    let Some(url) = state.config.docs_corpus_url.as_deref() else {
        return Ok(());
    };
    let embedder = state.embedder()?;
    let corpus = fetch(url).await?;
    let digest = format!("{:x}", Sha256::digest(corpus.as_bytes()));
    let entries = entries(&corpus, &site_of(url));
    // ⚠ A corpus with no pages is not a docs site that emptied: it is a moved
    // URL answering some other page, or an empty file. Indexing it would drop
    // every docs passage until the next day's refresh.
    if entries.is_empty() {
        return Err(unavailable(
            "docs corpus has no pages; the index is left as it is",
        ));
    }

    let Some(lease) = lease(state, SOURCE).await? else {
        return Ok(());
    };
    if lease.digest.as_deref() == Some(digest.as_str()) {
        release(state, SOURCE, lease.lease_id).await;
        return Ok(());
    }
    let written = holding(state, SOURCE, lease.lease_id, async {
        let prepared = prepare(&state.db, embedder, SOURCE, &entries).await?;
        let mut tx = maintenance_scope(&state.db, AgentLane).await?;
        store(&mut tx, SOURCE, &entries, &prepared).await?;
        let keep: Vec<String> = entries.iter().map(|e| e.source_id.clone()).collect();
        let dropped = db::drop_source_except(&mut tx, SOURCE, &keep).await?;
        // Settled last, as a page is: the cursor row stays locked only for
        // the moment before the commit.
        if !db::settle_lease(&mut tx, SOURCE, lease.lease_id).await? {
            tx.rollback().await?;
            return Ok(None);
        }
        db::record_digest(&mut tx, SOURCE, &digest).await?;
        tx.commit().await?;
        Ok::<_, TelmoniError>(Some(dropped))
    })
    .await;
    match written {
        Some(Ok(Some(dropped))) => {
            tracing::info!(pages = entries.len(), dropped, "agent docs indexed");
            Ok(())
        }
        // The lease was lost mid-refresh — this replica went quiet and
        // another took the cursor: tried again within the hour, not after
        // the day a finished refresh waits.
        None | Some(Ok(None)) => Err(unavailable(
            "the docs refresh lost its lease; another replica took it",
        )),
        Some(Err(e)) => {
            release(state, SOURCE, lease.lease_id).await;
            Err(e)
        }
    }
}

async fn fetch(url: &str) -> Result<String, TelmoniError> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("telmoni-agent")
        .build()
        .map_err(|e| TelmoniError::internal("docs client", e))?;
    let mut response = http
        .get(url)
        .send()
        .await
        .map_err(|e| unavailable(format!("docs corpus {}", error_kind(&e))))?;
    if !response.status().is_success() {
        return Err(unavailable(format!(
            "docs corpus answered {}",
            response.status().as_u16()
        )));
    }
    // The corpus is plain text; an HTML page is a URL that moved somewhere else.
    let html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("text/html"));
    if html {
        return Err(unavailable(
            "docs corpus answered an HTML page, not the text corpus",
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| unavailable(format!("docs corpus {}", error_kind(&e))))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BYTES {
            return Err(unavailable("docs corpus is larger than the index takes"));
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// The corpus's origin, for a page that names no source of its own.
fn site_of(corpus_url: &str) -> String {
    corpus_url.split_once("://").map_or_else(
        || corpus_url.to_owned(),
        |(scheme, rest)| {
            let host = rest.split('/').next().unwrap_or(rest);
            format!("{scheme}://{host}")
        },
    )
}

/// A heading's text and its anchor. The console writes each heading's own id
/// after it, as the page renders it (`## Active sessions [#active-sessions]`),
/// and that is the anchor; a heading with none gets the id a heading renders
/// with by default.
fn heading_parts(heading: &str) -> (String, String) {
    let heading = heading.trim();
    if let Some((text, rest)) = heading.rsplit_once("[#")
        && let Some(id) = rest.strip_suffix(']')
        && !id.is_empty()
        && !id.contains(['[', ']', ' '])
    {
        return (text.trim_end().to_owned(), id.to_owned());
    }
    (heading.to_owned(), slug(heading))
}

/// The anchor a docs heading renders with when it names none: lowercase,
/// words joined by `-`.
fn slug(heading: &str) -> String {
    let mut out = String::new();
    for c in heading.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if (c.is_whitespace() || c == '-') && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

/// Every page of the corpus, one entry each.
pub(crate) fn entries(corpus: &str, site: &str) -> Vec<Entry> {
    let mut pages: Vec<(String, String)> = Vec::new();
    let mut fence = chunk::Fence::default();
    let mut lines = corpus.lines().peekable();
    while let Some(line) = lines.next() {
        // A page opens with its title and its `Source:` line, whatever came
        // before: one page's unclosed fence must not swallow every page
        // after it. Elsewhere a `# comment` in a code block is code.
        let sourced = lines
            .peek()
            .is_some_and(|next| next.trim_start().starts_with("Source:"));
        let title = line.strip_prefix("# ");
        if sourced && title.is_some() {
            fence = chunk::Fence::default();
        }
        let in_code = fence.read(line);
        if let Some(title) = title.filter(|_| sourced || !in_code) {
            pages.push((title.trim().to_owned(), String::new()));
        } else if let Some((_, body)) = pages.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }

    let now = Utc::now();
    let mut entries = Vec::new();
    // Each page once by the key its passages are stored under: two pages with
    // no `Source:` line and one title would otherwise write over each other's
    // passages, and one's dropping a passage would take the other's.
    let mut seen = HashSet::new();
    for (title, body) in pages {
        let mut url = None;
        let mut text = String::new();
        for line in body.lines() {
            if url.is_none()
                && let Some(source) = line.trim().strip_prefix("Source:")
            {
                url = Some(source.trim().trim_end_matches('/').to_owned());
                continue;
            }
            text.push_str(line);
            text.push('\n');
        }
        let page_url = url.unwrap_or_else(|| site.to_owned());
        let passages: Vec<Passage> = chunk::split(&text)
            .into_iter()
            .map(|p| {
                let heading = p.heading.as_deref().map(heading_parts);
                Passage {
                    url: Some(match &heading {
                        Some((_, anchor)) => format!("{page_url}#{anchor}"),
                        None => page_url.clone(),
                    }),
                    title: match &heading {
                        Some((text, _)) => format!("{title} — {text}"),
                        None => title.clone(),
                    },
                    body: p.text,
                }
            })
            .collect();
        let source_id = format!("{page_url}|{title}");
        if passages.is_empty() || !seen.insert(source_id.clone()) {
            continue;
        }
        entries.push(Entry {
            source_id,
            organization_id: None,
            project_id: None,
            user_id: None,
            subject_user_id: None,
            visibility: Visibility::Everyone,
            created_at: now,
            passages,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_split_at_their_titles_and_link_to_their_anchors() {
        let corpus = "\
# Webhooks
Source: /docs/integrations/webhooks

Signed deliveries.

## Verifying signatures
Compute the HMAC.

## Rotating the secret [#rotate]
Mint another.

# Errors
The catalog.
";
        let entries = entries(corpus, "http://localhost:3000");
        assert_eq!(entries.len(), 2);
        let webhooks = &entries[0];
        assert_eq!(webhooks.passages.len(), 3);
        // The console's path, kept as it is: the link stays on its origin.
        assert_eq!(
            webhooks.passages[1].url.as_deref(),
            Some("/docs/integrations/webhooks#verifying-signatures")
        );
        assert_eq!(
            webhooks.passages[1].title,
            "Webhooks — Verifying signatures"
        );
        // The id the page writes after its heading is the anchor, and is not
        // the title's.
        assert_eq!(
            webhooks.passages[2].url.as_deref(),
            Some("/docs/integrations/webhooks#rotate")
        );
        assert_eq!(webhooks.passages[2].title, "Webhooks — Rotating the secret");
        assert!(!webhooks.passages[0].body.contains("Source:"));
        assert_eq!(
            entries[1].passages[0].url.as_deref(),
            Some("http://localhost:3000")
        );
    }

    #[test]
    fn a_shell_comment_in_a_code_block_is_not_a_page() {
        let corpus = "# Install\nSource: /docs/install\n\n```console\n# install the binary\n$ make up\n```\n";
        let entries = entries(corpus, "http://localhost:3000");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].passages[0].body.contains("# install the binary"));
    }

    #[test]
    fn an_unclosed_fence_does_not_swallow_the_next_page() {
        let corpus = "# One\nSource: /docs/one\n\n```bash\nmake up\n\n# Two\nSource: /docs/two\n\nSecond page.\n";
        let entries = entries(corpus, "http://localhost:3000");
        assert_eq!(entries.len(), 2);
        assert!(entries[1].passages[0].body.contains("Second page."));
    }

    #[test]
    fn a_page_is_taken_once_by_the_key_it_is_stored_under() {
        let corpus = "# Same\nFirst.\n\n# Same\nSecond.\n";
        let entries = entries(corpus, "http://localhost:3000");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].passages[0].body.contains("First."));
    }

    #[test]
    fn a_corpus_with_no_pages_has_no_entries() {
        assert!(entries("<!doctype html><html></html>", "http://localhost:3000").is_empty());
        assert!(entries("", "http://localhost:3000").is_empty());
    }

    #[test]
    fn the_site_is_the_corpus_origin() {
        assert_eq!(
            site_of("http://localhost:3000/llms-full.txt"),
            "http://localhost:3000"
        );
    }
}
