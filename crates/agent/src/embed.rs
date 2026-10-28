//! Embeddings from any OpenAI-compatible `/embeddings` endpoint: a local
//! Ollama by default, a hosted provider by a change of URL.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use telmoni_shared::TelmoniError;

use crate::config::{EMBEDDING_DIMENSIONS, EmbeddingsConfig};
use crate::model::{Endpoint, error_kind, http_client, not_now_error, send, unavailable, with_key};

/// How many texts one request carries.
pub(crate) const BATCH: usize = 32;

/// How long one batch may take: a local model on a CPU is slow to embed 32
/// passages, and the indexer is not waiting on anyone.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Turns text into vectors of [`EMBEDDING_DIMENSIONS`].
#[async_trait]
pub trait Embedder: Send + Sync {
    /// The model's name, recorded on every row it embeds.
    fn model(&self) -> &str;

    /// One vector per text, in order.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, TelmoniError>;
}

pub struct OpenAiEmbedder {
    http: reqwest::Client,
    config: EmbeddingsConfig,
}

impl OpenAiEmbedder {
    pub fn new(config: EmbeddingsConfig) -> reqwest::Result<Self> {
        Ok(Self {
            http: http_client(TIMEOUT)?,
            config,
        })
    }

    async fn batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, TelmoniError> {
        let body = if self.config.request_dimensions {
            json!({ "model": self.config.model, "input": texts, "dimensions": EMBEDDING_DIMENSIONS })
        } else {
            json!({ "model": self.config.model, "input": texts })
        };
        let request = with_key(
            self.http
                .post(format!("{}/embeddings", self.config.url))
                .json(&body),
            self.config.api_key.as_ref(),
        );
        let response = send(Endpoint::Embeddings, request).await?;
        // Read, then parsed: a body that never arrived whole is the
        // endpoint's state, and a whole one that does not parse — a
        // gateway's page in place of vectors, say — is its answer, and may be
        // about what it was sent. reqwest calls both a decode error.
        let body = response
            .bytes()
            .await
            .map_err(|e| not_now_error(Endpoint::Embeddings, error_kind(&e)))?;
        let answer: Embeddings = serde_json::from_slice(&body)
            .map_err(|_| unavailable("embeddings answered a body that does not parse"))?;
        vectors(answer, texts.len())
    }
}

/// The endpoint's answer: one vector per input, each saying which.
#[derive(Deserialize)]
pub(crate) struct Embeddings {
    data: Vec<Embedded>,
}

#[derive(Deserialize)]
struct Embedded {
    #[serde(default)]
    index: Option<u64>,
    embedding: Vec<f32>,
}

/// The `data` array, put back in input order by each entry's `index`.
pub(crate) fn vectors(answer: Embeddings, expected: usize) -> Result<Vec<Vec<f32>>, TelmoniError> {
    let mut out: Vec<Option<Vec<f32>>> = vec![None; expected];
    for (position, entry) in answer.data.into_iter().enumerate() {
        let index = entry
            .index
            .and_then(|i| usize::try_from(i).ok())
            .unwrap_or(position);
        match out.get_mut(index) {
            Some(slot) => *slot = Some(entry.embedding),
            None => return Err(unavailable("an embedding for an input that was not sent")),
        }
    }
    out.into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| unavailable("embeddings answered fewer vectors than inputs"))
}

#[async_trait]
impl Embedder for OpenAiEmbedder {
    fn model(&self) -> &str {
        &self.config.model
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, TelmoniError> {
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH) {
            out.extend(self.batch(batch).await?);
        }
        Ok(out)
    }
}

/// A vector as pgvector's text input reads it: `[0.1,0.2,…]`, cast with
/// `::vector` in the statement. Refused unless it is the column's width.
pub fn literal(vector: &[f32]) -> Result<String, TelmoniError> {
    if vector.len() != EMBEDDING_DIMENSIONS {
        return Err(TelmoniError::Internal(format!(
            "an embedding of {} dimensions for a column of {EMBEDDING_DIMENSIONS}",
            vector.len()
        )));
    }
    if vector.iter().any(|x| !x.is_finite()) {
        return Err(TelmoniError::Internal(
            "an embedding with a non-finite value".into(),
        ));
    }
    // Cosine distance to a zero vector is NaN, which sorts anywhere.
    if vector.iter().all(|x| *x == 0.0) {
        return Err(TelmoniError::Internal("an embedding of all zeros".into()));
    }
    let mut out = String::with_capacity(vector.len() * 12);
    out.push('[');
    for (i, x) in vector.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&x.to_string());
    }
    out.push(']');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_come_back_in_input_order() {
        let body = json!({"data": [
            {"index": 1, "embedding": [2.0, 2.5]},
            {"index": 0, "embedding": [1.0, 1.5]},
        ]});
        let answer = |n| vectors(serde_json::from_value(body.clone()).unwrap(), n);
        assert_eq!(answer(2).unwrap(), vec![vec![1.0, 1.5], vec![2.0, 2.5]]);
        assert!(answer(3).is_err());
    }

    #[test]
    fn only_a_full_width_finite_vector_becomes_a_literal() {
        assert!(literal(&[0.5; 3]).is_err());
        let mut v = vec![0.0f32; EMBEDDING_DIMENSIONS];
        assert!(literal(&v).is_err(), "a zero vector has no direction");
        v[1] = 0.5;
        assert!(literal(&v).unwrap().starts_with("[0,0.5,"));
        v[3] = f32::NAN;
        assert!(literal(&v).is_err());
    }
}
