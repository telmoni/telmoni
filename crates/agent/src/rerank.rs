//! An optional second pass over the fused results: a cross-encoder reads the
//! question beside each passage and scores them together, which neither
//! half of the search does. Without `RERANK_URL` the fused order stands.

use std::time::Duration;

use serde_json::{Value, json};
use telmoni_shared::TelmoniError;

use crate::config::RerankConfig;
use crate::model::{error_kind, http_client, send, unavailable};

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
        let mut request = self.http.post(&self.config.url).json(&json!({
            "model": self.config.model,
            "query": query,
            "documents": documents,
            "top_n": documents.len(),
        }));
        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key.expose());
        }
        let response = send("rerank", request).await?;
        let body: Value = response
            .json()
            .await
            .map_err(|e| unavailable(format!("rerank {}", error_kind(&e))))?;
        Ok(order(&body, documents.len()))
    }
}

/// `results` by descending score, keeping only indexes that were sent, each
/// once; anything the endpoint left out follows in its fused order.
pub(crate) fn order(body: &Value, sent: usize) -> Vec<usize> {
    let mut scored: Vec<(usize, f64)> = body
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let index = usize::try_from(r.get("index")?.as_u64()?).ok()?;
            let score = r
                .get("relevance_score")
                .or_else(|| r.get("score"))?
                .as_f64()?;
            (index < sent).then_some((index, score))
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
        assert_eq!(order(&body, 4), vec![2, 0, 1, 3]);
    }
}
