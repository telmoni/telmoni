//! The console's lanes under `/internal/agent`, each opening with who is
//! asking and from which project.
//!
//! A turn answers as server-sent events: `text` as the model writes,
//! `tool` as it looks something up, `citation` for each source the reply
//! cites, then `done` — or `error` when the turn fails after the stream has
//! begun. A refusal before that (the agent off, the hourly cap, a bad body)
//! is a problem response like every other lane's.

use std::convert::Infallible;
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{PersonAndOrganization, Scoped, person_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::{AuthError, AuthzError, ProblemDetails, ProjectId, TelmoniError};

use crate::AppState;
use crate::db::{self, MessageRole, StoredMessage};
use crate::index::conversations;
use crate::model::{Message, Model, ToolCall, ToolResult};
use crate::prompt;
use crate::tools::{self, Citation, Citations};
use crate::turn::{self, Ending, Runner};

/// The longest question, in characters. The console refuses past it too
/// (`AGENT_MESSAGE_MAX` in `web/lib/types/agent.ts`).
const MESSAGE_MAX_CHARS: usize = 4_000;

/// How much of the first question titles a new conversation.
const TITLE_CHARS: usize = 80;

/// What the person reads when the model wrote nothing at all.
const EMPTY_ANSWER: &str = "I could not put an answer together. Try asking again.";

/// What is kept, and said, in place of an answer an erasure fenced while it
/// was written: it may quote the person erased.
const WITHHELD: &str = "This answer was not kept: something it may have quoted was deleted \
                        while it was being written. Ask again.";

/// How long the check for an asker gone, before an answer is saved, may take.
pub(crate) const FINAL_RECHECK: std::time::Duration = std::time::Duration::from_secs(2);

/// How much of a conversation the model rereads each turn.
const HISTORY: i64 = 20;

/// How many conversations the panel lists.
const LIST_LIMIT: i64 = 50;

/// How many messages an opened conversation shows.
const OPEN_LIMIT: i64 = 200;

/// Who is asking, from which project, as auth resolves the headers.
async fn asker(state: &AppState, headers: &HeaderMap) -> Result<(Acting, ProjectId), TelmoniError> {
    let acting = state.auth.resolve(headers).await?;
    let project = acting.project_or_bad_request()?.project_id.clone();
    Ok((acting, project))
}

/// The person's scope in the organization their conversations belong to.
async fn author_scope<'a>(
    state: &'a AppState,
    acting: &Acting,
) -> Result<Scoped<'a, PersonAndOrganization>, TelmoniError> {
    let tx = person_scope(&state.db, &acting.user_id).await?;
    Ok(tx.bind_organization(&acting.organization_id).await?)
}

fn not_found() -> TelmoniError {
    AuthError::NotFound("no such conversation".into()).into()
}

/// `GET /internal/agent/status`.
pub async fn status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    asker(&state, &headers).await?;
    Ok(Json(json!({ "enabled": state.config.enabled() })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnRequest {
    conversation_id: Option<Uuid>,
    message: String,
}

/// `POST /internal/agent/turns`.
pub async fn post_turn(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TurnRequest>,
) -> Result<Response, TelmoniError> {
    let (acting, project_id) = asker(&state, &headers).await?;
    let model = state.model.clone().ok_or(TelmoniError::AgentDisabled)?;
    let message = body.message.trim().to_owned();
    if message.is_empty() || message.chars().count() > MESSAGE_MAX_CHARS {
        return Err(TelmoniError::BadRequest(format!(
            "message is 1-{MESSAGE_MAX_CHARS} characters"
        )));
    }

    let mut tx = author_scope(&state, &acting).await?;
    let (asked, oldest) = db::recent_questions(&mut tx, &acting.user_id).await?;
    if asked >= state.config.messages_per_hour {
        tx.commit().await?;
        let next = oldest.map_or(0, |t| {
            (t + chrono::Duration::hours(1) - Utc::now()).num_seconds()
        });
        return Err(TelmoniError::AgentRateLimited {
            retry_after_secs: u64::try_from(next).unwrap_or(0).max(1),
        });
    }
    let (conversation_id, history) = match body.conversation_id {
        Some(id) => {
            if !db::touch_conversation(&mut tx, &acting.user_id, &project_id, id).await? {
                return Err(not_found());
            }
            (id, db::messages(&mut tx, id, HISTORY).await?)
        }
        None => {
            let title: String = message.chars().take(TITLE_CHARS).collect();
            let id = db::create_conversation(
                &mut tx,
                &acting.organization_id,
                &project_id,
                &acting.user_id,
                &title,
            )
            .await?;
            (id, Vec::new())
        }
    };
    db::insert_message(
        &mut tx,
        conversation_id,
        &acting.organization_id,
        &project_id,
        &acting.user_id,
        MessageRole::User,
        &message,
        &json!([]),
    )
    .await?;
    let asked_at = db::now(&mut tx).await?;
    tx.commit().await?;

    let transcript = alternating(history, &message);

    let (sender, receiver) = mpsc::unbounded_channel::<Event>();
    tokio::spawn(run_turn(
        state.clone(),
        Asker { acting, asked_at },
        model,
        conversation_id,
        transcript,
        message,
        Events(sender),
    ));
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver
            .recv()
            .await
            .map(|event| (Ok::<_, Infallible>(event), receiver))
    });
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

/// One event of a turn's stream, as `web/lib/agent/stream.ts` parses it.
enum TurnEvent<'a> {
    Text(&'a str),
    Tool(&'a str),
    Citation(&'a Citation),
    Done {
        conversation_id: Uuid,
        message_id: Uuid,
    },
    Error {
        conversation_id: Uuid,
        problem: ProblemDetails,
    },
}

impl TurnEvent<'_> {
    fn into_sse(self) -> Event {
        let (name, data) = match self {
            Self::Text(delta) => ("text", json!({ "delta": delta })),
            Self::Tool(name) => ("tool", json!({ "name": name })),
            Self::Citation(citation) => ("citation", json!(citation)),
            Self::Done {
                conversation_id,
                message_id,
            } => (
                "done",
                json!({ "conversation_id": conversation_id, "message_id": message_id }),
            ),
            Self::Error {
                conversation_id,
                problem,
            } => (
                "error",
                json!({
                    "conversation_id": conversation_id,
                    "type": problem.type_uri,
                    "title": problem.title,
                    "detail": problem.detail,
                }),
            ),
        };
        Event::default().event(name).data(data.to_string())
    }
}

/// The stream's sending half. A send fails only once the asker has gone,
/// which the loop learns from [`Runner::cancelled`], so the error is moot.
#[derive(Clone)]
struct Events(mpsc::UnboundedSender<Event>);

impl Events {
    fn send(&self, event: TurnEvent<'_>) {
        let _ = self.0.send(event.into_sse());
    }

    fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

/// Who asked a turn, as it began.
struct Asker {
    acting: Acting,
    /// When the question was written, in the database's time.
    asked_at: DateTime<Utc>,
}

/// The live tools, reporting each call to the stream as it starts.
struct LiveRunner<'a> {
    state: &'a AppState,
    asker: &'a Asker,
    citations: Citations,
    events: &'a Events,
    /// Why the turn ends before its answer: the asker is no longer who began it.
    ended: Option<TelmoniError>,
}

#[async_trait]
impl Runner for LiveRunner<'_> {
    async fn run(&mut self, call: &ToolCall) -> ToolResult {
        let asker = self.asker_now().await;
        if !self.cancelled() {
            self.events.send(TurnEvent::Tool(&call.name));
        }
        match asker {
            Ok(acting) => tools::run(self.state, &acting, call, &mut self.citations).await,
            Err(e) => tools::refusal(call, e),
        }
    }

    async fn recheck(&mut self) {
        // Auth failing to answer just now ends nothing; only an asker gone does.
        let _ = self.asker_now().await;
    }

    fn cancelled(&self) -> bool {
        self.events.is_closed() || self.ended.is_some()
    }
}

impl LiveRunner<'_> {
    /// ⚠ **Who is asking, read again.** A call can come a minute and a half
    /// into the turn, and a role taken away, a seat removed or a project
    /// moved to another organization since it began holds for it, as it
    /// would for the console's next page. Read by the person and project,
    /// not the bearer, which may lapse mid-turn.
    ///
    /// One no longer who began the turn ends it ([`Self::ended`]), with
    /// nothing more written to the stream or saved: a sign-out everywhere is
    /// how a person stops a session they lost, and that session's open
    /// stream is one. A role that changed only narrows the lookups that
    /// follow; auth failing to answer refuses just the one.
    async fn asker_now(&mut self) -> Result<Acting, TelmoniError> {
        match self.state.auth.resolve_again(&self.asker.acting).await {
            Ok(acting) if acting.organization_id == self.asker.acting.organization_id => Ok(acting),
            Ok(_) => Err(self.end(
                AuthzError::Forbidden("this project has moved to another organization".into())
                    .into(),
            )),
            Err(e) if no_longer_the_asker(&e) => Err(self.end(e)),
            Err(e) => Err(e),
        }
    }

    /// End the turn; the loop stops before the model writes again. Answers
    /// an error for the lookup that found it out, which goes no further.
    fn end(&mut self, why: TelmoniError) -> TelmoniError {
        self.ended = Some(why);
        AuthzError::Forbidden("the turn has ended".into()).into()
    }
}

/// Whether auth's refusal to read who is asking again says they are no
/// longer who began the turn — signed out everywhere, their account on its
/// way out, off the project, or the project gone — rather than that auth
/// could not answer just now, which refuses only the one lookup.
const fn no_longer_the_asker(e: &TelmoniError) -> bool {
    match e {
        TelmoniError::Auth(AuthError::IdentityUnavailable) => false,
        TelmoniError::Auth(_) | TelmoniError::Authz(_) => true,
        _ => false,
    }
}

/// The turn, behind the stream. Its answer is saved, cited and indexed
/// only when it finished; a failure is an `error` event and a log line.
async fn run_turn(
    state: Arc<AppState>,
    asker: Asker,
    model: Arc<dyn Model>,
    conversation_id: Uuid,
    mut transcript: Vec<Message>,
    question: String,
    events: Events,
) {
    let acting = &asker.acting;
    let system = prompt::system(acting, &Utc::now().format("%Y-%m-%d").to_string());
    let specs = tools::specs();
    let mut runner = LiveRunner {
        state: &state,
        asker: &asker,
        citations: Citations::default(),
        events: &events,
        ended: None,
    };
    let sink = |text: &str| events.send(TurnEvent::Text(text));
    let outcome = turn::converse(
        model.as_ref(),
        &system,
        &mut transcript,
        &specs,
        &mut runner,
        &sink,
        tokio::time::Instant::now() + turn::TURN_BUDGET,
    )
    .await;
    // Once more before anything is saved: the asker may have gone since the
    // last check. Bounded, as an auth slow to answer must not eat the time
    // left to save and say `done`; one that does not answer ends nothing.
    if outcome
        .as_ref()
        .is_ok_and(|outcome| outcome.ending != Ending::Cancelled)
    {
        let _ = tokio::time::timeout(FINAL_RECHECK, runner.recheck()).await;
    }
    let citations = runner.citations;
    if let Some(why) = runner.ended {
        return failed(&events, acting, conversation_id, &why);
    }

    let outcome = match outcome {
        Ok(outcome) if outcome.ending == Ending::Cancelled => {
            tracing::debug!("agent turn abandoned by the asker");
            return;
        }
        Ok(outcome) => outcome,
        Err(e) => return failed(&events, acting, conversation_id, &e),
    };
    let mut answer = outcome.text;
    if let Some(note) = outcome.ending.note() {
        sink(note);
        answer.push_str(note);
    }
    if answer.trim().is_empty() {
        sink(EMPTY_ANSWER);
        EMPTY_ANSWER.clone_into(&mut answer);
    }
    let cited = citations.cited_in(&answer);
    for citation in &cited {
        events.send(TurnEvent::Citation(citation));
    }
    let saved = match save_answer(&state, &asker, conversation_id, &answer, &cited).await {
        Ok(saved) => saved,
        Err(e) => return failed(&events, acting, conversation_id, &e),
    };
    if saved.withheld {
        sink(&format!("\n\n{WITHHELD}"));
    }
    events.send(TurnEvent::Done {
        conversation_id,
        message_id: saved.id,
    });
    drop(events);
    if saved.withheld {
        return;
    }
    if let Err(e) = conversations::remember(
        &state,
        acting,
        conversations::Exchange {
            conversation_id,
            message_id: saved.id,
            question: &question,
            answer: &answer,
            asked_at: asker.asked_at,
        },
    )
    .await
    {
        tracing::warn!(error = %e, "the exchange was not indexed for later searches");
    }
}

/// A turn that failed after its stream began: an `error` event, and a log
/// line when the failure is the platform's.
fn failed(events: &Events, acting: &Acting, conversation_id: Uuid, e: &TelmoniError) {
    if events.is_closed() {
        tracing::debug!("agent turn abandoned by the asker");
        return;
    }
    let problem = e.to_problem_details();
    if problem.status >= 500 {
        tracing::warn!(user_id = %acting.user_id, error = %e, "agent turn failed");
    }
    events.send(TurnEvent::Error {
        conversation_id,
        problem,
    });
}

/// An answer as it was saved.
struct Saved {
    id: Uuid,
    /// [`WITHHELD`] was kept in its place.
    withheld: bool,
}

/// Save the answer — or, when an erasure reached its organization while the
/// turn ran, [`WITHHELD`] in its place: the turn may have read the person it
/// erased before the scrub took them, and the scrub cannot see this message.
async fn save_answer(
    state: &AppState,
    asker: &Asker,
    conversation_id: Uuid,
    answer: &str,
    citations: &[Citation],
) -> Result<Saved, TelmoniError> {
    let acting = &asker.acting;
    let project = acting.project_or_bad_request()?;
    let mut tx = author_scope(state, acting).await?;
    let withheld = db::erased_during(&mut tx, &acting.organization_id, asker.asked_at).await?;
    let (content, citations) = if withheld {
        (WITHHELD, json!([]))
    } else {
        (answer, json!(citations))
    };
    let id = db::insert_message(
        &mut tx,
        conversation_id,
        &acting.organization_id,
        &project.project_id,
        &acting.user_id,
        MessageRole::Assistant,
        content,
        &citations,
    )
    .await?;
    tx.commit().await?;
    Ok(Saved { id, withheld })
}

/// `GET /internal/agent/conversations` — the person's own, asked from this
/// project, newest first.
pub async fn list_conversations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let (acting, project_id) = asker(&state, &headers).await?;
    let mut tx = author_scope(&state, &acting).await?;
    let conversations =
        db::list_conversations(&mut tx, &acting.user_id, &project_id, LIST_LIMIT).await?;
    tx.commit().await?;
    Ok(Json(json!({ "conversations": conversations })))
}

/// `GET /internal/agent/conversations/{id}`.
pub async fn get_conversation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, TelmoniError> {
    let (acting, project_id) = asker(&state, &headers).await?;
    let mut tx = author_scope(&state, &acting).await?;
    let Some(conversation) = db::conversation(&mut tx, &acting.user_id, &project_id, id).await?
    else {
        return Err(not_found());
    };
    let messages = db::messages(&mut tx, id, OPEN_LIMIT).await?;
    tx.commit().await?;
    Ok(Json(json!({
        "id": conversation.id,
        "title": conversation.title,
        "project_id": conversation.project_id,
        "created_at": conversation.created_at,
        "messages": messages,
    })))
}

/// `DELETE /internal/agent/conversations/{id}` — the conversation, its
/// messages, and what it left in the index.
pub async fn delete_conversation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, TelmoniError> {
    let (acting, _) = asker(&state, &headers).await?;
    let mut tx = author_scope(&state, &acting).await?;
    let deleted = db::delete_conversation(&mut tx, &acting.user_id, id).await?;
    tx.commit().await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}

/// Every lane, when the deployment has no agent at all.
pub async fn absent_status() -> impl IntoResponse {
    Json(json!({ "enabled": false }))
}

/// Every other lane, when the deployment has no agent at all.
pub async fn absent() -> TelmoniError {
    TelmoniError::AgentDisabled
}

/// The history and the new question as a transcript every provider reads:
/// it opens on a question and alternates. The window is the last messages,
/// and a turn that failed or was stopped left its question unanswered, so
/// the window can open on an answer or hold two questions in a row — which
/// a strict chat template refuses on every turn of the conversation after.
/// An unanswered question is dropped; the one asked now stands.
fn alternating(history: Vec<StoredMessage>, question: &str) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(history.len() + 1);
    for m in history {
        match m.role {
            MessageRole::User => {
                if matches!(out.last(), Some(Message::User(_))) {
                    out.pop();
                }
                out.push(Message::User(m.content));
            }
            MessageRole::Assistant => {
                if matches!(out.last(), Some(Message::User(_))) {
                    out.push(Message::Assistant {
                        text: m.content,
                        tool_calls: Vec::new(),
                        raw: None,
                    });
                }
            }
        }
    }
    if matches!(out.last(), Some(Message::User(_))) {
        out.pop();
    }
    out.push(Message::User(question.to_owned()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(role: MessageRole, content: &str) -> StoredMessage {
        StoredMessage {
            id: Uuid::nil(),
            role,
            content: content.to_owned(),
            citations: json!([]),
            created_at: Utc::now(),
        }
    }

    fn roles(transcript: &[Message]) -> Vec<&'static str> {
        transcript
            .iter()
            .map(|m| match m {
                Message::User(_) => "user",
                Message::Assistant { .. } => "assistant",
                Message::ToolResults(_) => "tools",
            })
            .collect()
    }

    #[test]
    fn the_transcript_opens_on_a_question_and_alternates() {
        use MessageRole::{Assistant, User};
        let history = vec![
            stored(Assistant, "an answer whose question fell out of the window"),
            stored(User, "q1"),
            stored(Assistant, "a1"),
            stored(User, "q2, never answered"),
            stored(User, "q3"),
            stored(Assistant, "a3"),
            stored(User, "q4, stopped"),
        ];
        let transcript = alternating(history, "q5");
        assert_eq!(
            roles(&transcript),
            ["user", "assistant", "user", "assistant", "user"]
        );
        assert!(matches!(transcript.last(), Some(Message::User(q)) if q == "q5"));
        assert!(matches!(transcript.get(2), Some(Message::User(q)) if q == "q3"));
    }
}
