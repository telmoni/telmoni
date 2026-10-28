//! An optional second pass over the fused results: a cross-encoder reads the
//! question beside each passage and scores them together, which neither
//! half of the search does. Without `RERANK_URL` the fused order stands.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use telmoni_shared::TelmoniError;

use crate::config::RerankConfig;
use crate::model::{Endpoint, error_kind, http_client, send, unavailable, with_key};

/// How long a rerank may take. It sits inside a search the model is
/// waiting on, and past this the fused order is good enough.
const TIMEOUT: Duration = Duration::from_secs(20);

pub struct Reranker {
    http: reqwest::Client,
    config: RerankConfig,
}

impl Reranker {
    pub fn new(config: RerankConfig) -> reqwest::Result<Self> {
        Ok(Self {
            http: http_client(TIMEOUT)?,
            config,
        })
    }

    /// The indexes of `documents`, best first.
    pub async fn rank(
        &self,
        query: &str,
        documents: &[String],
    ) -> Result<Vec<usize>, TelmoniError> {
        let request = with_key(
            self.http.post(&self.config.url).json(&json!({
                "model": self.config.model,
                "query": query,
                "documents": documents,
                "top_n": documents.len(),
            })),
            self.config.api_key.as_ref(),
        );
        let response = send(Endpoint::Rerank, request).await?;
        let answer: Results = response
            .json()
            .await
            .map_err(|e| unavailable(format!("rerank {}", error_kind(&e))))?;
        Ok(order(answer, documents.len()))
    }
}

/// The endpoint's answer: each document sent, scored.
#[derive(Deserialize)]
pub(crate) struct Results {
    #[serde(default)]
    results: Vec<Ranked>,
}

#[derive(Deserialize)]
struct Ranked {
    index: u64,
    #[serde(alias = "score")]
    relevance_score: f64,
}

/// `results` by descending score, keeping only indexes that were sent, each
/// once; anything the endpoint left out follows in its fused order.
pub(crate) fn order(answer: Results, sent: usize) -> Vec<usize> {
    let mut scored: Vec<(usize, f64)> = answer
        .results
        .into_iter()
        .filter_map(|r| {
            let index = usize::try_from(r.index).ok()?;
            (index < sent).then_some((index, r.relevance_score))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut out: Vec<usize> = Vec::with_capacity(sent);
    for (index, _) in scored {
        if !out.contains(&index) {
            out.push(index);
        }
    }
    for index in 0..sent {
        if !out.contains(&index) {
            out.push(index);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_reorder_and_the_rest_keep_their_place() {
        let body = json!({"results": [
            {"index": 2, "relevance_score": 0.9},
            {"index": 0, "relevance_score": 0.1},
            {"index": 7, "relevance_score": 1.0},
        ]});
        assert_eq!(
            order(serde_json::from_value(body).unwrap(), 4),
            vec![2, 0, 1, 3]
        );
    }
}
