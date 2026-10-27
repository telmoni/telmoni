//! What telemetry tells the console's open pages, so one draws itself again
//! rather than polls: a project's spans arrived (`spans:ingested`, naming the
//! batch's runs and nothing a span holds). Sent on the organization's channel, which
//! the console's stream passes on to the project's readers alone
//! (`web/app/api/v1/events/route.ts`); on a task of its own, never waited on,
//! and lost while Redis stands in, the page's Refresh being there for that.

use std::sync::Arc;

use serde::Serialize;
use uuid::Uuid;

use crate::AppState;

/// What an organization's channel begins with, its id after: unkeyed, since
/// an id is a minted value that names nobody. The console's publisher spells
/// it from the wire contract.
pub const ORGANIZATION_CHANNEL: &str = "bfev:organization:";

fn channel(organization_id: &str) -> String {
    format!("{ORGANIZATION_CHANNEL}{organization_id}")
}

/// How many run ids a nudge carries. A trace's page needs its own among
/// them, and an export is most often one run or a few, so the cap is rarely
/// met; past it `more` tells the page its run may be among the rest.
const RUNS_MAX: usize = 32;

/// The event's name, as the console's `RealtimeEventSchema` knows it from the
/// wire contract.
pub const SPANS_INGESTED: &str = "spans:ingested";

#[derive(Debug, Serialize)]
struct Event<D> {
    #[serde(rename = "type")]
    kind: &'static str,
    data: D,
}

/// A batch of a project's spans, kept.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Spans<'a> {
    organization_id: &'a str,
    project_id: &'a str,
    runs: Vec<String>,
    more: bool,
}

/// The batch's runs as the nudge names them: each once, in the order the
/// batch first held it, up to the cap; and whether the batch held more.
fn runs(run_ids: impl IntoIterator<Item = Uuid>) -> (Vec<Uuid>, bool) {
    let mut runs: Vec<Uuid> = Vec::with_capacity(RUNS_MAX);
    for run in run_ids {
        if runs.contains(&run) {
            continue;
        }
        if runs.len() == RUNS_MAX {
            return (runs, true);
        }
        runs.push(run);
    }
    (runs, false)
}

/// The nudge for a batch whose kept rows belong to `run_ids`, in the batch's
/// order with its repeats: the JSON the console reads. Plain strings and a
/// bool encode without fail; the error is kept so the caller skips the
/// publish rather than sending a message of nothing.
fn spans_message(
    organization_id: &str,
    project_id: &str,
    run_ids: impl IntoIterator<Item = Uuid>,
) -> serde_json::Result<String> {
    let (runs, more) = runs(run_ids);
    serde_json::to_string(&Event {
        kind: SPANS_INGESTED,
        data: Spans {
            organization_id,
            project_id,
            runs: runs.iter().map(Uuid::to_string).collect(),
            more,
        },
    })
}

/// Tell the organization's open consoles that a batch of `project_id`'s is
/// in. Nothing here waits: the message is built now, cheaply, and published
/// on a task of its own (`publish`), so the export's answer is never held
/// for Redis.
pub fn spans_ingested(
    state: Arc<AppState>,
    organization_id: &str,
    project_id: &str,
    run_ids: impl IntoIterator<Item = Uuid>,
) {
    match spans_message(organization_id, project_id, run_ids) {
        Ok(message) => publish(
            state,
            organization_id,
            message,
            "telling the console of an export",
        ),
        Err(error) => tracing::debug!(
            project_id = %project_id,
            error = %error,
            "ingest: the console's nudge did not encode"
        ),
    }
}

/// The publish, on a task of its own: the connection is taken and the message
/// sent there, so no answer waits for Redis. A failure is handed to the
/// replica as every other call's is (`Redis::stood_in`), which logs its kind
/// alone and has the next calls stand in rather than wait out a timeout each.
fn publish(state: Arc<AppState>, organization_id: &str, message: String, what: &'static str) {
    let channel = channel(organization_id);
    tokio::spawn(async move {
        let Some(mut connection) = state.redis.connection().await else {
            return;
        };
        let published: Result<(), redis::RedisError> = redis::cmd("PUBLISH")
            .arg(&channel)
            .arg(&message)
            .query_async(&mut connection)
            .await;
        if let Err(error) = published {
            state.redis.stood_in(what, &error);
        }
    });
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    /// The console subscribes by the name its own publisher spells, so the
    /// two spellings are one contract.
    #[test]
    fn the_channel_is_the_consoles() {
        assert_eq!(
            channel("org_0miIn74svvAWv1Kk"),
            "bfev:organization:org_0miIn74svvAWv1Kk"
        );
    }

    #[test]
    fn a_nudge_names_each_run_once_and_nothing_else() {
        let run = Uuid::new_v4();
        let other = Uuid::new_v4();
        let parsed: Value = serde_json::from_str(&message_of(&[run, other, run])).expect("json");
        assert_eq!(parsed["type"], "spans:ingested");
        assert_eq!(parsed["data"]["organizationId"], "org_1");
        assert_eq!(parsed["data"]["projectId"], "project_1");
        assert_eq!(
            parsed["data"]["runs"],
            Value::from(vec![run.to_string(), other.to_string()])
        );
        assert_eq!(parsed["data"]["more"], false);
        assert_eq!(
            parsed["data"].as_object().map(serde_json::Map::len),
            Some(4),
            "ids and a flag: no span's field rides along"
        );
    }

    #[test]
    fn past_the_cap_the_rest_is_a_flag() {
        let runs: Vec<Uuid> = (0..RUNS_MAX + 5).map(|_| Uuid::new_v4()).collect();
        let parsed: Value = serde_json::from_str(&message_of(&runs)).expect("json");
        assert_eq!(
            parsed["data"]["runs"].as_array().map(Vec::len),
            Some(RUNS_MAX)
        );
        assert_eq!(parsed["data"]["more"], true);

        let exact: Vec<Uuid> = (0..RUNS_MAX).map(|_| Uuid::new_v4()).collect();
        let parsed: Value = serde_json::from_str(&message_of(&exact)).expect("json");
        assert_eq!(
            parsed["data"]["more"], false,
            "the cap itself is not past it"
        );
    }

    /// The nudge for `runs`, which plain ids and a bool always encode.
    fn message_of(runs: &[Uuid]) -> String {
        spans_message("org_1", "project_1", runs.iter().copied()).expect("encodes")
    }
}
