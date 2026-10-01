//! The customer docs, from the one text file the docs site publishes
//! (`DOCS_CORPUS_URL`, `llms-full.txt`): every page, each opening with its
//! `# Title` line and a `Source: <url>` line under it. A page is split at
//! its headings and each passage links to its own anchor. Only passages
//! whose text changed are embedded again, and pages that left the file
//! leave the index.

use std::time::Duration;

use chrono::Utc;
use sha2::{Digest, Sha256};
use telmoni_shared::TelmoniError;
use telmoni_shared::db::tenant_session::maintenance_scope;

use super::{Entry, Passage, chunk, write};
use crate::AppState;
use crate::db::{self, AgentLane, Source, Visibility};
use crate::model::{error_kind, unavailable};

const SOURCE: Source = Source::Docs;

/// The largest corpus taken: a docs site is a few hundred kilobytes, and a
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

    let mut tx = maintenance_scope(&state.db, AgentLane).await?;
    let Some(cursor) = db::lock_cursor(&mut tx, SOURCE).await? else {
        tx.commit().await?;
        return Ok(());
    };
    if cursor.digest.as_deref() == Some(digest.as_str()) {
        tx.commit().await?;
        return Ok(());
    }
    let entries = entries(&corpus, &site_of(url));
    write(&mut tx, embedder, SOURCE, &entries).await?;
    let keep: Vec<String> = entries.iter().map(|e| e.source_id.clone()).collect();
    let dropped = db::drop_source_except(&mut tx, SOURCE, &keep).await?;
    db::record_digest(&mut tx, SOURCE, &digest).await?;
    tx.commit().await?;
    tracing::info!(pages = entries.len(), dropped, "agent docs indexed");
    Ok(())
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

/// The site's origin, for a page that names no source of its own.
fn site_of(corpus_url: &str) -> String {
    corpus_url.split_once("://").map_or_else(
        || corpus_url.to_owned(),
        |(scheme, rest)| {
            let host = rest.split('/').next().unwrap_or(rest);
            format!("{scheme}://{host}")
        },
    )
}

/// The anchor a docs heading renders with: lowercase, words joined by `-`.
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
    for line in corpus.lines() {
        if let Some(title) = line.strip_prefix("# ") {
            pages.push((title.trim().to_owned(), String::new()));
        } else if let Some((_, body)) = pages.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }

    let now = Utc::now();
    let mut entries = Vec::new();
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
            .map(|p| Passage {
                url: Some(match &p.heading {
                    Some(heading) => format!("{page_url}#{}", slug(heading)),
                    None => page_url.clone(),
                }),
                title: match &p.heading {
                    Some(heading) => format!("{title} — {heading}"),
                    None => title.clone(),
                },
                body: p.text,
            })
            .collect();
        if passages.is_empty() {
            continue;
        }
        entries.push(Entry {
            source_id: format!("{page_url}|{title}"),
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
Source: https://docs.telmoni.com/integrations/webhooks/

Signed deliveries.

## Verifying signatures
Compute the HMAC.

# Errors
The catalog.
";
        let entries = entries(corpus, "https://docs.telmoni.com");
        assert_eq!(entries.len(), 2);
        let webhooks = &entries[0];
        assert_eq!(webhooks.passages.len(), 2);
        assert_eq!(
            webhooks.passages[1].url.as_deref(),
            Some("https://docs.telmoni.com/integrations/webhooks#verifying-signatures")
        );
        assert_eq!(
            webhooks.passages[1].title,
            "Webhooks — Verifying signatures"
        );
        assert!(!webhooks.passages[0].body.contains("Source:"));
        assert_eq!(
            entries[1].passages[0].url.as_deref(),
            Some("https://docs.telmoni.com")
        );
    }

    #[test]
    fn the_site_is_the_corpus_origin() {
        assert_eq!(
            site_of("https://docs.telmoni.com/llms-full.txt"),
            "https://docs.telmoni.com"
        );
    }
}
