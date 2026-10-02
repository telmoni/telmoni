//! The module's state from its configuration: its pool as its own role, the
//! model and embedding clients, and the width probe that refuses an
//! embedding model the column cannot hold.

use std::sync::Arc;

use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::seam::{Auth, Notifications};

use crate::config::{Config, EMBEDDING_DIMENSIONS};
use crate::embed::{Embedder, OpenAiEmbedder};
use crate::rerank::Reranker;
use crate::{AppState, model};

/// The module's pool, opened as its own database role.
pub async fn open_pool(config: &Config) -> anyhow::Result<sqlx::PgPool> {
    let db_ca = telmoni_shared::db::database_ca_from_env().map_err(anyhow::Error::msg)?;
    Ok(
        telmoni_shared::db::create_pool_for_service(
            &config.database_url,
            "agent",
            db_ca.as_deref(),
        )
        .await?,
    )
}

/// The state the binary runs the module on, with the siblings it reads
/// through. The model and the embedder exist only when the agent is on.
pub fn state(
    config: Config,
    db: sqlx::PgPool,
    service_secrets: ServiceSecrets,
    auth: Arc<dyn Auth>,
    notifications: Arc<dyn Notifications>,
) -> anyhow::Result<AppState> {
    let (model, embedder, reranker) = match &config.model {
        None => (None, None, None),
        Some(model_config) => {
            let embedder: Arc<dyn Embedder> =
                Arc::new(OpenAiEmbedder::new(config.embeddings.clone())?);
            let reranker = config
                .rerank
                .clone()
                .map(Reranker::new)
                .transpose()?
                .map(Arc::new);
            (
                Some(model::from_config(model_config)?),
                Some(embedder),
                reranker,
            )
        }
    };
    tracing::info!(
        enabled = config.enabled(),
        provider = config.model.as_ref().map(|m| m.provider.as_str()),
        model = config.model.as_ref().map(|m| m.model.as_str()),
        embeddings_model = %config.embeddings.model,
        rerank = config.rerank.is_some(),
        docs = config.docs_corpus_url.is_some(),
        "agent configured"
    );
    Ok(AppState {
        db,
        config,
        service_secrets,
        auth,
        notifications,
        model,
        embedder,
        reranker,
    })
}

/// How long boot waits on the width probe. The chart's liveness probe gives a
/// pod about 30 seconds before it restarts it.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// What the width probe learned.
#[derive(Debug, PartialEq, Eq)]
pub enum Probe {
    /// The model answers vectors of the column's width.
    Fits,
    /// The endpoint did not answer, so the width is unknown.
    Unanswered,
}

/// Embed one probe string and refuse to start unless the vector is the
/// column's width. A model of another width is either refused by pgvector
/// on every write or, worse, sized to fit and searching nonsense; better
/// the pod fails its rollout and says why.
///
/// ⚠ **An endpoint that does not answer is not a wrong model.** The agent
/// shares its process with sign-in and every other module, and a vendor's outage at
/// the moment a pod starts would otherwise crash-loop the whole server over
/// a feature it can run without. That comes back as [`Probe::Unanswered`];
/// the caller decides whether to go on.
///
/// ⚠ **Bounded well inside the liveness probe.** The probe runs before the
/// port is bound, and an endpoint that hangs rather than refuses (a cold
/// Ollama loading its model, a blackholed host) would otherwise hold boot
/// for the embedder's whole batch timeout, past the point Kubernetes kills
/// the pod — every module crash-looping over one it can run without.
pub async fn probe_width(embedder: &dyn Embedder) -> anyhow::Result<Probe> {
    probe_width_within(embedder, PROBE_TIMEOUT).await
}

async fn probe_width_within(
    embedder: &dyn Embedder,
    timeout: std::time::Duration,
) -> anyhow::Result<Probe> {
    let text = ["telmoni width probe".to_owned()];
    let vectors = match tokio::time::timeout(timeout, embedder.embed(&text)).await {
        Ok(Ok(vectors)) => vectors,
        Err(_) => {
            tracing::warn!(
                model = embedder.model(),
                timeout_secs = timeout.as_secs(),
                "the embeddings endpoint did not answer the width probe in time"
            );
            return Ok(Probe::Unanswered);
        }
        Ok(Err(e)) => {
            tracing::warn!(
                model = embedder.model(),
                error = %e,
                "the embeddings endpoint did not answer the width probe"
            );
            return Ok(Probe::Unanswered);
        }
    };
    let width = vectors.first().map_or(0, Vec::len);
    if width != EMBEDDING_DIMENSIONS {
        anyhow::bail!(
            "EMBEDDINGS_MODEL {} answers vectors of {width} dimensions, and agent.chunks holds \
             {EMBEDDING_DIMENSIONS}; configure a {EMBEDDING_DIMENSIONS}-dimension model \
             (nomic-embed-text, or a provider's model asked for {EMBEDDING_DIMENSIONS})",
            embedder.model()
        );
    }
    Ok(Probe::Fits)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use telmoni_shared::TelmoniError;

    use super::*;

    struct Width(usize);

    #[async_trait]
    impl Embedder for Width {
        fn model(&self) -> &str {
            "probe-model"
        }

        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, TelmoniError> {
            Ok(texts.iter().map(|_| vec![0.5; self.0]).collect())
        }
    }

    struct Down;

    #[async_trait]
    impl Embedder for Down {
        fn model(&self) -> &str {
            "probe-model"
        }

        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, TelmoniError> {
            Err(TelmoniError::Internal("connection refused".into()))
        }
    }

    #[tokio::test]
    async fn the_probe_refuses_a_model_of_another_width() {
        assert_eq!(
            probe_width(&Width(EMBEDDING_DIMENSIONS)).await.unwrap(),
            Probe::Fits
        );
        let err = probe_width(&Width(1536)).await.unwrap_err().to_string();
        assert!(err.contains("1536 dimensions"), "{err}");
        assert!(probe_width(&Width(0)).await.is_err());
    }

    #[tokio::test]
    async fn an_endpoint_that_does_not_answer_is_not_a_wrong_model() {
        assert_eq!(probe_width(&Down).await.unwrap(), Probe::Unanswered);
    }

    struct Hangs;

    #[async_trait]
    impl Embedder for Hangs {
        fn model(&self) -> &str {
            "probe-model"
        }

        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, TelmoniError> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn an_endpoint_that_hangs_is_given_up_on_not_waited_for() {
        let within = std::time::Duration::from_millis(20);
        assert_eq!(
            probe_width_within(&Hangs, within).await.unwrap(),
            Probe::Unanswered
        );
    }
}
