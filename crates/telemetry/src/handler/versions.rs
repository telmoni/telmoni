//! `GET /internal/v1/telemetry/agent-versions` — one agent's releases, its
//! models or one prompt's versions as its Agents page sets them side by
//! side ([`crate::versions`]), in one environment, over the Agents page's
//! days held to what the plan and the project keep. The agent is a
//! parameter, never a path segment: a name is any text, `..` among it.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use serde::Serialize;

use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::{ChangeKind, ProjectId, TelmoniError};

use crate::AppState;
use crate::contract::{AGENT_MAX, ATTRIBUTE_VALUE_MAX, ENVIRONMENT_MAX};
use crate::handler::agents::AGENTS_DAYS;
use crate::handler::traces::{read_failed, reader, window_prices};
use crate::store::Projects;
use crate::traces::TraceFilter;
use crate::versions::{self, Choice, Of, Version};

/// One agent's versions over the window.
#[derive(Debug, Serialize)]
pub struct Versions {
    /// The days the window reads.
    pub days: u16,
    pub by: ChangeKind,
    /// The environment the rows are of: the one asked for, else the one
    /// most of the agent's traces were sent from.
    pub environment: String,
    /// The agent's environments, the most traces first.
    pub environments: Vec<Choice>,
    /// The prompt whose versions the rows are, where they are a prompt's:
    /// the one asked for, else the one most traces name.
    pub prompt: Option<String>,
    /// The prompts the agent's traces name in the environment, read where the
    /// rows are a prompt's.
    pub prompts: Vec<Choice>,
    /// The newest first seen first, the traces that carried none last.
    pub versions: Vec<Version>,
    /// The scores the rows show, the most given first.
    pub scores: Vec<String>,
    /// Whether the window held more versions than the answer names.
    pub more: bool,
}

/// What the lane was asked.
#[derive(Debug, PartialEq, Eq)]
struct Asked {
    agent: String,
    by: ChangeKind,
    /// None where the query names none; the one with no name is named empty.
    environment: Option<String>,
    prompt: Option<String>,
}

/// `given`, once: a parameter the lane takes a single value of.
fn once(name: &str, slot: &mut Option<String>, given: String) -> Result<(), TelmoniError> {
    if slot.is_some() {
        return Err(TelmoniError::BadRequest(format!(
            "{name} is given more than once"
        )));
    }
    *slot = Some(given);
    Ok(())
}

/// A name of 1 to `max` characters, as it was sent: a name the answer
/// offered is asked for whole, its spaces among it.
fn named(name: &str, given: &str, max: usize) -> Result<(), TelmoniError> {
    if (1..=max).contains(&given.chars().count()) {
        Ok(())
    } else {
        Err(TelmoniError::BadRequest(format!(
            "{name} is 1 to {max} characters"
        )))
    }
}

/// The query read: one `agent`, and at most one `by`, `environment` and
/// `prompt`.
fn asked(pairs: Vec<(String, String)>) -> Result<Asked, TelmoniError> {
    let (mut agent, mut by, mut environment, mut prompt) = (None, None, None, None);
    for (name, value) in pairs {
        match name.as_str() {
            "agent" => once("agent", &mut agent, value)?,
            "by" => once("by", &mut by, value)?,
            "environment" => once("environment", &mut environment, value)?,
            "prompt" => once("prompt", &mut prompt, value)?,
            _ => {
                return Err(TelmoniError::BadRequest(
                    "the lane takes no parameter by that name".into(),
                ));
            }
        }
    }
    let agent = agent.ok_or_else(|| TelmoniError::BadRequest("agent names the agent".into()))?;
    named("agent", &agent, AGENT_MAX)?;
    let by = match by {
        None => ChangeKind::Release,
        Some(given) => given.parse().map_err(|_| {
            TelmoniError::BadRequest(format!(
                "by is one of {}",
                ChangeKind::all().map(ChangeKind::as_str).join(", ")
            ))
        })?,
    };
    if environment
        .as_deref()
        .is_some_and(|given| given.chars().count() > ENVIRONMENT_MAX)
    {
        return Err(TelmoniError::BadRequest(format!(
            "environment is longer than {ENVIRONMENT_MAX} characters"
        )));
    }
    if let Some(prompt) = &prompt {
        named("prompt", prompt, ATTRIBUTE_VALUE_MAX)?;
    }
    Ok(Asked {
        agent,
        by,
        environment,
        prompt,
    })
}

/// The days the rows read: the Agents page's, held to the plan's and to the
/// project's own retention, since neither store keeps a span past either.
async fn window_days(state: &AppState, project_id: &ProjectId) -> Result<u16, TelmoniError> {
    let visible = crate::capacity::visible_days(state, project_id, AGENTS_DAYS).await;
    let mut tx = project_scope(&state.db, project_id).await?;
    let kept = crate::db::retention_days(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(match kept.map(u16::try_from) {
        Some(Ok(kept)) => visible.min(kept),
        _ => visible,
    })
}

/// `GET /internal/v1/telemetry/agent-versions` — anyone on the project reads it.
pub async fn get_versions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Versions>, TelmoniError> {
    let reader = reader(&state, &headers).await?;
    let asked = asked(pairs)?;
    let days = window_days(&state, &reader.project_id).await?;
    let projects = Projects::one(reader.project_id.clone());
    let filter = TraceFilter {
        days,
        prices: window_prices(&state, &reader.project_id, days).await,
        ..TraceFilter::default()
    };
    let environments = versions::environments(&state.store, &projects, &filter, &asked.agent)
        .await
        .map_err(read_failed)?;
    let environment = asked
        .environment
        .or_else(|| environments.first().map(|first| first.value.clone()))
        .unwrap_or_default();
    let (prompts, prompt) = if asked.by == ChangeKind::Prompt {
        let prompts =
            versions::prompts(&state.store, &projects, &filter, &asked.agent, &environment)
                .await
                .map_err(read_failed)?;
        let prompt = asked
            .prompt
            .or_else(|| prompts.first().map(|first| first.value.clone()));
        (prompts, prompt)
    } else {
        (Vec::new(), None)
    };
    let of = Of {
        agent: &asked.agent,
        by: asked.by,
        environment: &environment,
        prompt: prompt.as_deref().unwrap_or_default(),
    };
    // A prompt's versions with no prompt to name are none.
    let (rows, more) = if asked.by == ChangeKind::Prompt && prompt.is_none() {
        (Vec::new(), false)
    } else {
        versions::rows(&state.store, &projects, &filter, &of)
            .await
            .map_err(read_failed)?
    };
    let means = versions::score_means(&state.store, &projects, &filter, &of, &rows)
        .await
        .map_err(read_failed)?;
    let (versions, scores) = versions::scored(rows, means);
    Ok(Json(Versions {
        days,
        by: asked.by,
        environment,
        environments,
        prompt,
        prompts,
        versions,
        scores,
        more,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(query: &[(&str, &str)]) -> Vec<(String, String)> {
        query
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn refused(query: &[(&str, &str)]) -> bool {
        matches!(asked(pairs(query)), Err(TelmoniError::BadRequest(_)))
    }

    #[test]
    fn an_agent_is_asked_for_as_it_is_named() {
        for agent in [
            ".",
            "..",
            "a/b",
            "%",
            "100%25",
            " padded ",
            "a b",
            "agént-日本",
        ] {
            let read = asked(pairs(&[("agent", agent)])).expect("a name is any text");
            assert_eq!(read.agent, agent);
            assert_eq!(read.by, ChangeKind::Release, "releases where none is asked");
            assert_eq!((read.environment, read.prompt), (None, None));
        }
    }

    #[test]
    fn one_agent_of_1_to_128_characters_is_asked_for() {
        assert!(refused(&[]), "none");
        assert!(refused(&[("agent", "")]), "an empty one");
        assert!(refused(&[("agent", "a"), ("agent", "b")]), "two");
        assert!(refused(&[("agent", "a"), ("agent", "a")]), "one twice");
        assert!(asked(pairs(&[("agent", &"é".repeat(AGENT_MAX))])).is_ok());
        assert!(refused(&[("agent", &"é".repeat(AGENT_MAX + 1))]));
    }

    #[test]
    fn the_split_is_one_of_its_three_words() {
        for (word, by) in [
            ("release", ChangeKind::Release),
            ("model", ChangeKind::Model),
            ("prompt", ChangeKind::Prompt),
        ] {
            let read = asked(pairs(&[("agent", "a"), ("by", word)])).expect("a word of its own");
            assert_eq!(read.by, by);
        }
        assert!(refused(&[("agent", "a"), ("by", "tool")]));
        assert!(refused(&[("agent", "a"), ("by", "")]));
        assert!(refused(&[
            ("agent", "a"),
            ("by", "model"),
            ("by", "release")
        ]));
    }

    #[test]
    fn a_prompt_is_as_long_as_an_attribute_and_an_environment_as_its_own() {
        let prompt = |given: &str| {
            asked(pairs(&[
                ("agent", "a"),
                ("by", "prompt"),
                ("prompt", given),
            ]))
        };
        assert!(prompt(&"p".repeat(128)).is_ok());
        assert!(prompt(&"p".repeat(129)).is_ok(), "longer than an agent's");
        assert!(prompt(&"é".repeat(ATTRIBUTE_VALUE_MAX)).is_ok());
        assert!(prompt(&"p".repeat(ATTRIBUTE_VALUE_MAX + 1)).is_err());
        assert!(prompt("").is_err(), "a prompt has a name");
        assert!(refused(&[("agent", "a"), ("prompt", "p"), ("prompt", "q")]));
        let environment = |given: &str| asked(pairs(&[("agent", "a"), ("environment", given)]));
        assert_eq!(
            environment("").expect("the one with no name").environment,
            Some(String::new())
        );
        assert!(environment(&"e".repeat(ENVIRONMENT_MAX)).is_ok());
        assert!(environment(&"e".repeat(ENVIRONMENT_MAX + 1)).is_err());
        assert!(refused(&[
            ("agent", "a"),
            ("environment", "x"),
            ("environment", "y")
        ]));
    }

    #[test]
    fn a_parameter_the_lane_does_not_take_is_refused() {
        assert!(refused(&[("agent", "a"), ("days", "7")]));
    }
}
