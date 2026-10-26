//! Alert precision: each member's word on an incident, useful or noise, one
//! a person an incident and changed at will, given behind sign-in from the
//! link every notice carries; and the share of the project's votes over a
//! window that call its incidents useful, overall and alert by alert, which
//! heads the project's alerts and steers them.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{HasProject, Scoped};
use telmoni_shared::{ProjectId, UserId, Verdict};

/// An incident's votes, and the asker's own.
#[derive(Debug, Clone, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct Tally {
    pub useful: i64,
    pub noise: i64,
    pub yours: Option<Verdict>,
}

/// One alert's votes over a window.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RuleTally {
    pub rule_id: Uuid,
    pub rule_name: String,
    pub useful: i64,
    pub noise: i64,
}

/// The person's vote on one of the project's incidents, given or changed.
/// Answers whether the project has the incident.
pub async fn vote<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    incident_id: Uuid,
    by: &UserId,
    verdict: Verdict,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "INSERT INTO telemetry.incident_votes (incident_id, project_id, user_id, verdict)
         SELECT id, project_id, $3, $4 FROM telemetry.incidents
          WHERE project_id = $1 AND id = $2
         ON CONFLICT (incident_id, user_id) DO UPDATE
            SET verdict = EXCLUDED.verdict, voted_at = now()",
    )
    .bind(project_id)
    .bind(incident_id)
    .bind(by)
    .bind(verdict)
    .execute(tx.conn())
    .await?
    .rows_affected()
        > 0)
}

/// An incident's votes, and `by`'s own.
pub async fn tally<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    incident_id: Uuid,
    by: &UserId,
) -> sqlx::Result<Tally> {
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE verdict = 'useful') AS useful,
                count(*) FILTER (WHERE verdict = 'noise') AS noise,
                max(verdict) FILTER (WHERE user_id = $3) AS yours
           FROM telemetry.incident_votes WHERE project_id = $1 AND incident_id = $2",
    )
    .bind(project_id)
    .bind(incident_id)
    .bind(by)
    .fetch_one(tx.conn())
    .await
}

/// The votes on the project's incidents opened since `since`, alert by
/// alert, every alert that has an incident since among them.
pub async fn precision<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    since: DateTime<Utc>,
) -> sqlx::Result<Vec<RuleTally>> {
    sqlx::query_as(
        "SELECT rule.id AS rule_id, rule.name AS rule_name,
                count(vote.verdict) FILTER (WHERE vote.verdict = 'useful') AS useful,
                count(vote.verdict) FILTER (WHERE vote.verdict = 'noise') AS noise
           FROM telemetry.incidents AS incident
           JOIN telemetry.rules AS rule ON rule.id = incident.rule_id
           LEFT JOIN telemetry.incident_votes AS vote ON vote.incident_id = incident.id
          WHERE incident.project_id = $1 AND incident.opened_at >= $2
          GROUP BY rule.id, rule.name
          ORDER BY lower(rule.name), rule.id",
    )
    .bind(project_id)
    .bind(since)
    .fetch_all(tx.conn())
    .await
}
