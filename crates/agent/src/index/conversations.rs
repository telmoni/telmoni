//! Past exchanges, so a person can ask about what they asked before. Each
//! answered question becomes one passage, keyed to its author: only their
//! own searches find it (`author_access`), and it goes with the
//! conversation, the person, the project or the organization.

use chrono::{DateTime, Utc};
use telmoni_shared::TelmoniError;
use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::person_scope;
use uuid::Uuid;

use super::chunk::{MAX_CHARS, content_hash};
use super::embedding_text;
use crate::AppState;
use crate::db::{self, NewChunk, Source, Visibility};
use crate::embed::literal;

const VISIBILITY: Visibility = Visibility::Author;

/// One answered question, as it was saved.
pub struct Exchange<'a> {
    pub conversation_id: Uuid,
    pub message_id: Uuid,
    pub question: &'a str,
    pub answer: &'a str,
    /// When the question was written, in the database's time.
    pub asked_at: DateTime<Utc>,
}

/// Index one answered question, under the asker's own scope.
pub async fn remember(
    state: &AppState,
    acting: &Acting,
    exchange: Exchange<'_>,
) -> Result<(), TelmoniError> {
    let Some(project) = acting.project.as_ref() else {
        return Ok(());
    };
    let Exchange {
        conversation_id,
        message_id,
        question,
        answer,
        asked_at,
    } = exchange;
    let embedder = state.embedder()?;
    let title: String = format!("Earlier question: {question}")
        .chars()
        .take(200)
        .collect();
    let body: String = format!("Question: {question}\n\nAnswer: {answer}")
        .chars()
        .take(MAX_CHARS)
        .collect();
    let vector = embedder
        .embed(&[embedding_text(&title, &body)])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| TelmoniError::Internal("no embedding for the exchange".into()))?;
    let embedding = literal(&vector)?;
    let source_id = format!("{conversation_id}:{message_id}");
    let hash = content_hash(&title, &body, None, VISIBILITY.as_str());

    let tx = person_scope(&state.db, &acting.user_id).await?;
    let mut tx = tx.bind_organization(&acting.organization_id).await?;
    // Reached by an erasure since the question was asked, whose scrub cannot
    // see this passage: not remembered. Or deleted while the exchange was
    // being embedded: nothing to remember. The erasure lock first, as an
    // answer's save takes it before it touches the conversation's row.
    if db::erased_during(&mut tx, &acting.organization_id, asked_at).await?
        || !db::hold_conversation(&mut tx, conversation_id).await?
    {
        tx.commit().await?;
        return Ok(());
    }
    db::upsert_chunk(
        &mut tx,
        &NewChunk {
            organization_id: Some(&acting.organization_id),
            project_id: Some(&project.project_id),
            user_id: Some(&acting.user_id),
            subject_user_id: None,
            source: Source::Conversation,
            source_id: &source_id,
            part: 0,
            visibility: VISIBILITY,
            title: &title,
            body: &body,
            url: None,
            content_hash: &hash,
            embedding: &embedding,
            model: embedder.model(),
            source_created_at: chrono::Utc::now(),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
