//! Incidents: a monitor's spells of needing someone, and the notices that
//! open and close them. One opens as a monitor's state moves into failed,
//! stalled, over a bound or late, and resolves as it is healthy again or its
//! owner pauses it — one open incident a monitor, however many runs fail
//! while it stands, in the table the alert rules will write too. A monitor
//! flipping in and out of trouble more than [`FLAP_THRESHOLD`] times in
//! [`FLAP_WINDOW_SECS`] is flapping: its incident stays open, marked so,
//! until it has held one state for the window, which the ticker sees
//! ([`steady`]). Written where the state is written, in its transaction,
//! each opening and resolution a notice through the outbox.

use chrono::{DateTime, Duration, Utc};
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped};
use telmoni_shared::{IncidentDetail, MonitorState, OrganizationId, ProjectId, TelmoniError};

use crate::db::{self, IncidentRow, MonitorRow, NewIncident};
use crate::notify;
use crate::outbox;

/// The window the flips are counted in, an hour, and how many in it make a
/// monitor flapping: more than six.
pub const FLAP_WINDOW_SECS: i32 = 60 * 60;
pub const FLAP_THRESHOLD: i64 = 6;

/// What moved a monitor into the state that opens an incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Its run ended in error, with the exit code where the ping gave one.
    RunFailed { exit_code: Option<i32> },
    /// A run it expected never came within its grace.
    RunNeverCame,
    /// Its run never said it ended, and was closed at its ceiling.
    RunNeverEnded,
    /// A run it expected has not started, and its grace is running out.
    RunLate,
    /// Its run went quiet past its stall timeout.
    RunStalled,
    /// Its run crossed its step cap or its cost cap.
    RunOverBound,
}

impl Trigger {
    /// What the row keeps of it, on the wire contract.
    #[must_use]
    pub const fn detail(self) -> IncidentDetail {
        match self {
            Self::RunFailed { .. } => IncidentDetail::RunFailed,
            Self::RunNeverCame => IncidentDetail::RunNeverCame,
            Self::RunNeverEnded => IncidentDetail::RunNeverEnded,
            Self::RunLate => IncidentDetail::RunLate,
            Self::RunStalled => IncidentDetail::RunStalled,
            Self::RunOverBound => IncidentDetail::RunOverBound,
        }
    }

    /// The word the row keeps, and the notice's `detail`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.detail().as_str()
    }

    /// The trigger a kept word names, its exit code gone with the row.
    pub(crate) fn from_word(word: &str) -> Option<Self> {
        Some(match word.parse::<IncidentDetail>().ok()? {
            IncidentDetail::RunFailed => Self::RunFailed { exit_code: None },
            IncidentDetail::RunNeverCame => Self::RunNeverCame,
            IncidentDetail::RunNeverEnded => Self::RunNeverEnded,
            IncidentDetail::RunLate => Self::RunLate,
            IncidentDetail::RunStalled => Self::RunStalled,
            IncidentDetail::RunOverBound => Self::RunOverBound,
        })
    }

    /// The one a state implies, where its writer named none.
    const fn for_state(state: MonitorState) -> Option<Self> {
        Some(match state {
            MonitorState::Failed => Self::RunFailed { exit_code: None },
            MonitorState::Stalled => Self::RunStalled,
            MonitorState::OverBound => Self::RunOverBound,
            MonitorState::Late => Self::RunLate,
            _ => return None,
        })
    }

    /// What happened, as a clause after "when".
    pub(crate) fn clause(self) -> String {
        match self {
            Self::RunFailed {
                exit_code: Some(code),
            } => format!("its run ended in error with exit code {code}"),
            Self::RunFailed { exit_code: None } => "its run ended in error".to_string(),
            Self::RunNeverCame => "a run it expected never came within its grace".to_string(),
            Self::RunNeverEnded => {
                "its run never said it ended and was closed at its ceiling".to_string()
            }
            Self::RunLate => "a run it expected had not started as its grace ran".to_string(),
            Self::RunStalled => "its run went quiet past its stall timeout".to_string(),
            Self::RunOverBound => "its run crossed its step cap or its cost cap".to_string(),
        }
    }
}

/// Whether a monitor in `state` needs someone: an incident stands open while
/// it does.
#[must_use]
pub const fn alerting(state: MonitorState) -> bool {
    matches!(
        state,
        MonitorState::Failed | MonitorState::Stalled | MonitorState::OverBound | MonitorState::Late
    )
}

/// How an incident closed, its word the row's and the notice's `resolution`.
pub use telmoni_shared::IncidentResolution as Resolution;

/// What a state change, or a tick, did to the monitor's incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// One opened, and the project told.
    Opened,
    /// The open one resolved, and the project told.
    Resolved,
    /// The open one kept open and marked flapping; nothing told.
    Flapping,
    /// The open one's flapping mark cleared: an ordinary incident from here.
    Steadied,
    /// Nothing written.
    Nothing,
}

/// The monitor's incident after its state moved from `from` to the one its
/// row holds, under the lock its caller holds on the row: one opened as it
/// moves into trouble, the open one resolved as it is healthy again or
/// paused, and nothing else moved — a run starting or failing again under an
/// open incident changes it nothing. `trigger` is what moved it, where its
/// writer knows; `now` the clock the state was written at.
pub async fn on_change(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    monitor: &MonitorRow,
    from: MonitorState,
    trigger: Option<Trigger>,
    now: DateTime<Utc>,
) -> Result<Change, TelmoniError> {
    let to = monitor.state;
    if from == to {
        return Ok(Change::Nothing);
    }
    let open = db::open_incident(tx, project_id, monitor.id).await?;
    match (open, to) {
        (None, to) if alerting(to) => {
            let Some(trigger) = trigger.or_else(|| Trigger::for_state(to)) else {
                return Ok(Change::Nothing);
            };
            let flips = db::flips_since(tx, project_id, monitor.id, now - window()).await? + 1;
            let incident = db::insert_incident(
                tx,
                project_id,
                organization_id,
                &NewIncident {
                    monitor_id: monitor.id,
                    cause: to,
                    detail: trigger.as_str(),
                    opened_at: now,
                    flapping: flips > FLAP_THRESHOLD,
                },
            )
            .await?;
            outbox::enqueue_notice(
                tx,
                project_id,
                organization_id,
                &notify::incident_opened(monitor, &incident, trigger),
            )
            .await?;
            tracing::info!(
                project_id = %project_id,
                monitor_id = %monitor.id,
                incident_id = %incident.id,
                cause = %to,
                detail = trigger.as_str(),
                flapping = incident.flapping,
                "telemetry: an incident opened"
            );
            Ok(Change::Opened)
        }
        (Some(open), MonitorState::Healthy) => {
            if open.flapping {
                return Ok(Change::Nothing);
            }
            let flips = db::flips_since(tx, project_id, monitor.id, now - window()).await? + 1;
            if flips > FLAP_THRESHOLD {
                db::set_flapping(tx, project_id, open.id, true).await?;
                tracing::info!(
                    project_id = %project_id,
                    monitor_id = %monitor.id,
                    incident_id = %open.id,
                    flips,
                    "telemetry: a monitor is flapping; its incident stays open"
                );
                return Ok(Change::Flapping);
            }
            resolve(
                tx,
                project_id,
                organization_id,
                monitor,
                &open,
                Resolution::Recovered,
                now,
            )
            .await?;
            Ok(Change::Resolved)
        }
        (Some(open), MonitorState::Paused) => {
            resolve(
                tx,
                project_id,
                organization_id,
                monitor,
                &open,
                Resolution::Paused,
                now,
            )
            .await?;
            Ok(Change::Resolved)
        }
        _ => Ok(Change::Nothing),
    }
}

/// A flapping incident let go of once its monitor has held one state for
/// the window: resolved where the monitor is healthy, closed as paused where
/// its owner paused it, and its mark cleared where it still needs someone or
/// a run is going, so it stands as an ordinary incident from here. The
/// ticker reads for such monitors and calls this on each visit.
pub async fn steady(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    monitor: &MonitorRow,
    now: DateTime<Utc>,
) -> Result<Change, TelmoniError> {
    let Some(open) = db::open_incident(tx, project_id, monitor.id).await? else {
        return Ok(Change::Nothing);
    };
    if !open.flapping || monitor.state_changed_at + window() > now {
        return Ok(Change::Nothing);
    }
    match monitor.state {
        MonitorState::Healthy => {
            resolve(
                tx,
                project_id,
                organization_id,
                monitor,
                &open,
                Resolution::Recovered,
                now,
            )
            .await?;
            Ok(Change::Resolved)
        }
        MonitorState::Paused | MonitorState::New => {
            resolve(
                tx,
                project_id,
                organization_id,
                monitor,
                &open,
                Resolution::Paused,
                now,
            )
            .await?;
            Ok(Change::Resolved)
        }
        _ => {
            db::set_flapping(tx, project_id, open.id, false).await?;
            tracing::info!(
                project_id = %project_id,
                monitor_id = %monitor.id,
                incident_id = %open.id,
                state = %monitor.state,
                "telemetry: a monitor held one state for an hour; its incident is no longer flapping"
            );
            Ok(Change::Steadied)
        }
    }
}

fn window() -> Duration {
    Duration::seconds(i64::from(FLAP_WINDOW_SECS))
}

/// The open incident closed, with how, and the project told.
async fn resolve(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    monitor: &MonitorRow,
    open: &IncidentRow,
    resolution: Resolution,
    now: DateTime<Utc>,
) -> Result<(), TelmoniError> {
    let Some(closed) =
        db::resolve_incident(tx, project_id, open.id, resolution.as_str(), now).await?
    else {
        return Ok(());
    };
    outbox::enqueue_notice(
        tx,
        project_id,
        organization_id,
        &notify::incident_resolved(monitor, &closed, resolution),
    )
    .await?;
    tracing::info!(
        project_id = %project_id,
        monitor_id = %monitor.id,
        incident_id = %closed.id,
        resolution = resolution.as_str(),
        "telemetry: an incident resolved"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four states that need someone open an incident; the rest never do.
    #[test]
    fn the_states_that_need_someone_are_the_alerting_ones() {
        for state in MonitorState::all() {
            assert_eq!(
                alerting(state),
                matches!(
                    state,
                    MonitorState::Failed
                        | MonitorState::Stalled
                        | MonitorState::OverBound
                        | MonitorState::Late
                ),
                "{state}"
            );
            assert_eq!(
                Trigger::for_state(state).is_some(),
                alerting(state),
                "{state}"
            );
        }
    }

    /// Every trigger's word reads back as itself, its exit code aside.
    #[test]
    fn a_triggers_word_reads_back() {
        for trigger in [
            Trigger::RunFailed { exit_code: Some(3) },
            Trigger::RunNeverCame,
            Trigger::RunNeverEnded,
            Trigger::RunLate,
            Trigger::RunStalled,
            Trigger::RunOverBound,
        ] {
            let back = Trigger::from_word(trigger.as_str()).expect("a word");
            assert_eq!(back.as_str(), trigger.as_str());
        }
        assert!(Trigger::from_word("rule").is_none());
    }
}
