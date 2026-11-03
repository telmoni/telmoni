//! Evaluators taught by people, as LangSmith's few-shot evaluators are: a
//! member's score of a run or a span an evaluator scored, under the same
//! name, is a correction, kept with what the evaluator read of it — its input
//! and output — as content, sealed under the project's key in the run's own
//! content (`example/<evaluator>/<target>`), dated as that content is, so a
//! trace's deletion, an end user's erasure, retention and the content switch
//! take it as they take the run's. An evaluator whose prompt holds `{{examples}}` is given its newest
//! few. A session's score teaches nothing: no run would carry it through an
//! erasure.

use serde_json::json;
use uuid::Uuid;

use telmoni_shared::{ContentMode, EvaluationTarget, OrganizationId, ProjectId, TelmoniError};

use crate::AppState;
use crate::scores::{Judgement, TargetKey};

/// The field an example is kept as, and what its span's id begins with.
pub const FIELD: &str = "telmoni.evaluator.example";
pub const SPAN_PREFIX: &str = "example/";

/// How many corrections an evaluator is given, and the most characters of
/// each of its input and output.
pub const GIVEN_MAX: i64 = 5;
pub const TEXT_MAX: usize = 2_000;

/// What every example of `evaluator` is kept under.
#[must_use]
pub fn prefix_of(evaluator: Uuid) -> String {
    format!("{SPAN_PREFIX}{evaluator}/")
}

/// The span one example is kept under: its evaluator's, then its target's.
fn span_of(evaluator: Uuid, target: &TargetKey) -> Option<String> {
    let at = match target {
        TargetKey::Run(run) => format!("run/{run}"),
        TargetKey::Span(run, span) => format!("span/{run}/{span}"),
        TargetKey::Session(_) => return None,
    };
    Some(format!("{}{at}", prefix_of(evaluator)))
}

/// A judgement as an example says it: a yes or no, a number, or its categories.
fn said(judgement: &Judgement) -> String {
    match judgement.data_type {
        telmoni_shared::ScoreType::Boolean => if judgement.value.unwrap_or(0.0) >= 0.5 {
            "yes"
        } else {
            "no"
        }
        .to_owned(),
        telmoni_shared::ScoreType::Number => {
            judgement.value.map_or_else(String::new, |v| v.to_string())
        }
        telmoni_shared::ScoreType::Category => judgement.categories.join(", "),
    }
}

/// `text` cut to [`TEXT_MAX`] characters.
fn cut(text: &str) -> String {
    text.chars().take(TEXT_MAX).collect()
}

/// One evaluator's score of the target under the name, as its row has it.
pub struct Judged {
    pub evaluator: Uuid,
    pub judgement: Judgement,
}

/// A member's `given` score of `target`, kept as a correction of each
/// evaluator in `judged` that scored it: in place of the last, on a project
/// that keeps content and holds room for it. Best effort: the member's score
/// stands whatever this meets, logged by kind.
pub async fn learn(
    state: &AppState,
    (project_id, organization_id): (&ProjectId, &OrganizationId),
    target: &TargetKey,
    given: &Judgement,
    judged: &[Judged],
) {
    if judged.is_empty() || matches!(target, TargetKey::Session(_)) {
        return;
    }
    if let Err(error) = learned(state, (project_id, organization_id), target, given, judged).await {
        tracing::warn!(project_id = %project_id, error = %error,
            "telemetry: a member's correction was not kept as an evaluator's example; their score stands");
    }
}

async fn learned(
    state: &AppState,
    (project_id, organization_id): (&ProjectId, &OrganizationId),
    target: &TargetKey,
    given: &Judgement,
    judged: &[Judged],
) -> Result<(), TelmoniError> {
    let mut tx = telmoni_shared::db::tenant_session::project_scope(&state.db, project_id).await?;
    let mode = crate::db::content_mode(&mut tx, project_id).await?;
    // A typed evaluator's state never names its corrections, so none is kept for one.
    let ids: Vec<Uuid> = judged.iter().map(|one| one.evaluator).collect();
    let prompts: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM (SELECT DISTINCT ON (id) id, kind FROM telemetry.evaluators
                          WHERE project_id = $1 AND id = ANY($2) ORDER BY id, version DESC) newest
          WHERE kind = 'prompt'",
    )
    .bind(project_id)
    .bind(&ids)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    let judged: Vec<&Judged> = judged
        .iter()
        .filter(|one| prompts.contains(&one.evaluator))
        .collect();
    if judged.is_empty()
        || mode != ContentMode::On
        || !crate::capacity::has_room_for_content(state, organization_id).await
    {
        return Ok(());
    }
    let (kind, run, span) = match target {
        TargetKey::Run(run) => (EvaluationTarget::Run, *run, None),
        TargetKey::Span(run, span) => (EvaluationTarget::Span, *run, Some(span.clone())),
        TargetKey::Session(_) => return Ok(()),
    };
    if crate::erasure::was_deleted(state, project_id, run).await {
        return Ok(());
    }
    let Some((variables, _, trace_id)) =
        super::run::variables_of(state, project_id, kind, Some(run), span.as_ref(), None).await?
    else {
        return Ok(());
    };
    // Dated as the run's content is, so retention takes it with that content.
    let mut tx = telmoni_shared::db::tenant_session::project_scope(&state.db, project_id).await?;
    let dated: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT min(received_at) FROM telemetry.plain_content WHERE project_id = $1 AND run_id = $2",
    )
    .bind(project_id)
    .bind(run)
    .fetch_one(tx.conn())
    .await?;
    tx.commit().await?;
    let Some(received_at) = dated else {
        return Ok(());
    };
    let input = variables.get("input").map(|t| cut(t)).unwrap_or_default();
    let output = variables.get("output").map(|t| cut(t)).unwrap_or_default();
    for one in judged {
        let Some(span_id) = span_of(one.evaluator, target) else {
            continue;
        };
        let example = json!({
            "input": input, "output": output,
            "judged": said(&one.judgement), "corrected": said(given),
        })
        .to_string();
        crate::content::forget_named(state, project_id, run, std::slice::from_ref(&span_id))
            .await?;
        crate::content::keep(
            state,
            project_id,
            received_at,
            vec![crate::ingest::batch::SpanContent {
                run_id: run,
                trace_id: trace_id.clone(),
                span_id,
                fields: vec![(FIELD.to_owned(), example)],
            }],
        )
        .await?;
    }
    Ok(())
}

/// The corrections of `target` kept for any evaluator, taken back with the
/// member's score.
///
/// # Errors
///
/// As [`crate::content::keep`].
pub async fn forget_target(
    state: &AppState,
    project_id: &ProjectId,
    target: &TargetKey,
    evaluators: &[Uuid],
) -> Result<(), TelmoniError> {
    let spans: Vec<String> = evaluators
        .iter()
        .filter_map(|evaluator| span_of(*evaluator, target))
        .collect();
    let run = match target {
        TargetKey::Run(run) | TargetKey::Span(run, _) => *run,
        TargetKey::Session(_) => return Ok(()),
    };
    if spans.is_empty() {
        return Ok(());
    }
    crate::content::forget_named(state, project_id, run, &spans).await
}

/// The evaluator's newest corrections as its prompt's `{{examples}}`, each
/// what it read, what it said and what a person said instead, and how many;
/// empty where it has none.
///
/// # Errors
///
/// As [`crate::content::keep`].
pub async fn rendered(
    state: &AppState,
    project_id: &ProjectId,
    evaluator: Uuid,
) -> Result<(String, usize), TelmoniError> {
    let kept = crate::content::open_prefixed_for_evaluation(
        state,
        project_id,
        FIELD,
        &prefix_of(evaluator),
        GIVEN_MAX,
    )
    .await?;
    let mut out = String::new();
    let mut count = 0;
    for (n, text) in kept.iter().enumerate() {
        let Ok(example) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        let field = |key: &str| {
            example
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        out.push_str(&format!(
            "Example {}:\nInput: {}\nOutput: {}\nYou said: {}. A person corrected it to: {}.\n\n",
            n + 1,
            field("input"),
            field("output"),
            field("judged"),
            field("corrected"),
        ));
        count += 1;
    }
    Ok((out.trim_end().to_owned(), count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_example_is_kept_under_its_evaluator_and_target_and_a_session_never() {
        let evaluator = Uuid::nil();
        let run = Uuid::from_u128(1);
        assert_eq!(
            span_of(evaluator, &TargetKey::Run(run)).as_deref(),
            Some(
                "example/00000000-0000-0000-0000-000000000000/run/00000000-0000-0000-0000-000000000001"
            )
        );
        assert!(
            span_of(evaluator, &TargetKey::Span(run, "00f067aa0ba902b7".into())).is_some_and(|s| s
                .starts_with(&prefix_of(evaluator))
                && s.ends_with("/00f067aa0ba902b7"))
        );
        assert_eq!(span_of(evaluator, &TargetKey::Session("conv".into())), None);
        assert_eq!(cut(&"x".repeat(TEXT_MAX + 5)).chars().count(), TEXT_MAX);
    }
}
