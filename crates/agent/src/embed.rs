//! Embeddings from any OpenAI-compatible `/embeddings` endpoint: a local
//! Ollama by default, a hosted provider by a change of URL.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use telmoni_shared::TelmoniError;

use crate::config::{EMBEDDING_DIMENSIONS, EmbeddingsConfig};
use crate::model::{error_kind, http_client, send, unavailable};

/// How many texts one request carries.
const BATCH: usize = 32;

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
        let mut request = self
            .http
            .post(format!("{}/embeddings", self.config.url))
            .json(&body);
        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key.expose());
        }
        let response = send("embeddings", request).await?;
        let body: Value = response
            .json()
            .await
            .map_err(|e| unavailable(format!("embeddings {}", error_kind(&e))))?;
        vectors(&body, texts.len())
    }
}

/// The `data` array, put back in input order by each entry's `index`.
pub(crate) fn vectors(body: &Value, expected: usize) -> Result<Vec<Vec<f32>>, TelmoniError> {
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| unavailable("embeddings answered without `data`"))?;
    let mut out: Vec<Option<Vec<f32>>> = vec![None; expected];
    for (position, entry) in data.iter().enumerate() {
        let index = entry
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|i| usize::try_from(i).ok())
            .unwrap_or(position);
        let vector: Vec<f32> = entry
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| unavailable("an embedding without a vector"))?
            .iter()
            .map(|x| x.as_f64().map(narrow))
            .collect::<Option<_>>()
            .ok_or_else(|| unavailable("an embedding with a non-number in it"))?;
        match out.get_mut(index) {
            Some(slot) => *slot = Some(vector),
            None => return Err(unavailable("an embedding for an input that was not sent")),
        }
    }
    out.into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| unavailable("embeddings answered fewer vectors than inputs"))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "an embedding is f32 on every provider's side; the JSON number is its widening"
)]
const fn narrow(x: f64) -> f32 {
    x as f32
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
        assert_eq!(
            vectors(&body, 2).unwrap(),
            vec![vec![1.0, 1.5], vec![2.0, 2.5]]
        );
        assert!(vectors(&body, 3).is_err());
    }

    #[test]
    fn only_a_full_width_finite_vector_becomes_a_literal() {
        assert!(literal(&[0.5; 3]).is_err());
        let mut v = vec![0.0f32; EMBEDDING_DIMENSIONS];
        assert!(literal(&v).unwrap().starts_with("[0,0,"));
        v[3] = f32::NAN;
        assert!(literal(&v).is_err());
    }
}
