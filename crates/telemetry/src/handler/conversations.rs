//! The console's conversation lanes, under `Resource::Trace`, which every
//! role on a project holds: `GET /internal/telemetry/conversations`, the page
//! — every conversation the project's runs belonged to in a window of days,
//! with the end user it was with and what its runs came to, sorted as the
//! URL says — and `GET /internal/telemetry/conversations/{conversation_id}`,
//! one conversation's traces in the order they ran: a replay in metadata,
//! never a message, since content is not metadata. Each reads the spans of
//! the project `x-project-id` names and no other.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{AuthError, ConversationSort, SortDirection, TelmoniError, TraceSort};

use crate::AppState;
use crate::contract::CONVERSATION_ID_MAX;
use crate::conversations::{self, ConversationRow};
use crate::handler::traces::{Trace, parse_days, parse_limit, read_failed};
use crate::store::Projects;
use crate::traces::{self, TraceFilter, Window};

/// The page's query string, every value text until it is checked, so an
/// empty value asks for the default and a value that is not what its
/// parameter takes is a 400 that echoes none of it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationsQuery {
    /// The column to sort by ([`ConversationSort`]).
    #[serde(default)]
    pub sort: String,
    /// Which way ([`SortDirection`]); the column's own default when unset.
    #[serde(default)]
    pub dir: String,
    /// How many days back.
    #[serde(default)]
    pub days: String,
    /// The most conversations to answer.
    #[serde(default)]
    pub limit: String,
}

impl ConversationsQuery {
    /// The window and the sort the query asks for, each value checked.
    ///
    /// # Errors
    ///
    /// A value that is not what its parameter takes, named by the parameter
    /// and never echoed.
    pub fn into_parts(self) -> Result<(Window, ConversationSort, SortDirection), TelmoniError> {
        let sort = match self.sort.trim() {
            "" => ConversationSort::LastSeen,
            given => given.parse::<ConversationSort>().map_err(|_| {
                TelmoniError::BadRequest(
                    "sort is one of conversation, user, traces, errors, steps, tokens, \
                     first_seen, last_seen"
                        .into(),
                )
            })?,
        };
        let direction = match self.dir.trim() {
            "" => default_direction(sort),
            given => given
                .parse::<SortDirection>()
                .map_err(|_| TelmoniError::BadRequest("dir is asc or desc".into()))?,
        };
        Ok((
            Window {
                days: parse_days(&self.days)?,
                limit: parse_limit(&self.limit)?,
            },
            sort,
            direction,
        ))
    }
}

/// The way a column sorts when the URL names none: the most and the latest
/// first; ids in the alphabet's order, and the earliest seen first.
#[must_use]
pub const fn default_direction(sort: ConversationSort) -> SortDirection {
    match sort {
        ConversationSort::Conversation | ConversationSort::User | ConversationSort::FirstSeen => {
            SortDirection::Asc
        }
        ConversationSort::Traces
        | ConversationSort::Errors
        | ConversationSort::Steps
        | ConversationSort::Tokens
        | ConversationSort::LastSeen => SortDirection::Desc,
    }
}

/// A conversation's own page's query string: the window its traces are read
/// in.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationQuery {
    /// How many days back.
    #[serde(default)]
    pub days: String,
    /// The most traces to answer.
    #[serde(default)]
    pub limit: String,
}

/// What the page's lane answers.
#[derive(Debug, Serialize)]
pub struct ConversationList {
    /// The conversations, in the order asked.
    pub conversations: Vec<Conversation>,
    /// How many days back the page read.
    pub days: u16,
    /// The most it would answer.
    pub limit: u32,
    /// Whether the window held more conversations than the limit.
    pub more: bool,
    /// The sort it is in.
    pub sort: ConversationSort,
    /// And its direction.
    pub dir: SortDirection,
}

/// One conversation of the page: its runs, summed ([`ConversationRow`]).
#[derive(Debug, Serialize)]
pub struct Conversation {
    pub conversation_id: String,
    pub user_id: String,
    pub traces: u64,
    pub errors: u64,
    pub steps: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

impl From<ConversationRow> for Conversation {
    fn from(row: ConversationRow) -> Self {
        Self {
            conversation_id: row.conversation_id,
            user_id: row.user_id,
            traces: row.traces,
            errors: row.errors,
            steps: row.steps,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            first_seen: row.first_seen,
            last_seen: row.last_seen,
        }
    }
}

/// What one conversation's lane answers: its traces in the order they ran.
#[derive(Debug, Serialize)]
pub struct ConversationDetail {
    pub conversation_id: String,
    /// The conversation's runs, earliest first, each summed as the trace
    /// list sums it.
    pub traces: Vec<Trace>,
    /// How many days back the lane read.
    pub days: u16,
    /// The most traces it would answer.
    pub limit: u32,
    /// Whether the window held more of the conversation's traces than the
    /// limit.
    pub more: bool,
}

/// `GET /internal/telemetry/conversations` — the project's conversations in
/// the window, sorted as the query says, up to its limit, and whether the
/// window held more.
pub async fn list_conversations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ConversationsQuery>,
) -> Result<Json<ConversationList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Trace)?
        .project_id
        .clone();
    let (window, sort, dir) = query.into_parts()?;
    // One past the limit, so the answer can say whether there were more.
    let read = Window {
        limit: window.limit + 1,
        ..window
    };
    let rows = conversations::list(&state.store, &Projects::one(project_id), read, sort, dir)
        .await
        .map_err(read_failed)?;
    let more = rows.len() > window.limit as usize;
    Ok(Json(ConversationList {
        conversations: rows
            .into_iter()
            .take(window.limit as usize)
            .map(Conversation::from)
            .collect(),
        days: window.days,
        limit: window.limit,
        more,
        sort,
        dir,
    }))
}

/// `GET /internal/telemetry/conversations/{conversation_id}` — one
/// conversation's traces in the window, earliest first; `404` for a
/// conversation the project's runs in the window did not belong to, which is
/// also what another project's conversation id gets.
pub async fn get_conversation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Query(query): Query<ConversationQuery>,
) -> Result<Json<ConversationDetail>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Trace)?
        .project_id
        .clone();
    let conversation_id = conversation_id.trim().to_owned();
    if conversation_id.is_empty() || conversation_id.chars().count() > CONVERSATION_ID_MAX {
        return Err(TelmoniError::BadRequest(format!(
            "a conversation is named by its id, up to {CONVERSATION_ID_MAX} characters"
        )));
    }
    let window = Window {
        days: parse_days(&query.days)?,
        limit: parse_limit(&query.limit)?,
    };
    let filter = TraceFilter {
        conversation_id: conversation_id.clone(),
        days: window.days,
        // One past the limit, so the answer can say whether there were more.
        limit: window.limit + 1,
        ..TraceFilter::default()
    };
    let rows = traces::list(
        &state.store,
        &Projects::one(project_id),
        &filter,
        TraceSort::Started,
        SortDirection::Asc,
    )
    .await
    .map_err(read_failed)?;
    if rows.is_empty() {
        return Err(AuthError::NotFound(
            "no conversation with that id among this project's runs in the window".into(),
        )
        .into());
    }
    let more = rows.len() > window.limit as usize;
    Ok(Json(ConversationDetail {
        conversation_id,
        traces: rows
            .into_iter()
            .take(window.limit as usize)
            .map(Trace::from)
            .collect(),
        days: window.days,
        limit: window.limit,
        more,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traces::{DEFAULT_DAYS, DEFAULT_LIMIT};

    #[test]
    fn an_empty_query_is_the_latest_first_over_a_week() {
        let (window, sort, dir) = ConversationsQuery::default()
            .into_parts()
            .expect("the default");
        assert_eq!(window.days, DEFAULT_DAYS);
        assert_eq!(window.limit, DEFAULT_LIMIT);
        assert_eq!(sort, ConversationSort::LastSeen);
        assert_eq!(dir, SortDirection::Desc);
    }

    #[test]
    fn each_column_has_its_own_first_direction() {
        for sort in ConversationSort::all() {
            let (_, parsed, dir) = ConversationsQuery {
                sort: sort.to_string(),
                ..ConversationsQuery::default()
            }
            .into_parts()
            .expect("a column");
            assert_eq!(parsed, sort);
            assert_eq!(dir, default_direction(sort));
        }
        assert_eq!(
            default_direction(ConversationSort::Conversation),
            SortDirection::Asc
        );
        assert_eq!(
            default_direction(ConversationSort::LastSeen),
            SortDirection::Desc
        );
    }

    /// A value a parameter does not take is refused by the parameter's name,
    /// and never echoed.
    #[test]
    fn a_bad_value_names_the_parameter_and_echoes_nothing() {
        let refused = ConversationsQuery {
            sort: "spend<script>".into(),
            ..ConversationsQuery::default()
        }
        .into_parts()
        .expect_err("refused")
        .to_problem_details();
        assert_eq!(refused.status, 400);
        let detail = refused.detail.expect("a detail");
        assert!(detail.starts_with("sort"), "{detail}");
        assert!(!detail.contains("script"));
    }
}
