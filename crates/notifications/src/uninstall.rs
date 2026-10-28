//! Uninstalling a vendor's app from a workspace once no connection there uses
//! it. Slack's `apps.uninstall` ends every connection in the workspace, so the
//! count that decides it is made under the workspace's lock, which an install
//! takes too. A delete files the uninstall in its own transaction, so neither a
//! crash nor a caller that stops waiting loses it; an install refused after its
//! exchange files one on a task of its own, so only the process ending between the
//! two can.
//! Each is settled right after its commit, and the retention sweep settles what
//! that left.

use std::collections::BTreeMap;
use std::sync::Arc;

use telmoni_shared::db::tenant_session::{Binding, Scoped, maintenance_scope};
use telmoni_shared::envelope::KekError;
use telmoni_shared::{Redacted, TelmoniError};

use crate::AppState;
use crate::connector::slack::SlackConnector;
use crate::connector::{ConnectionStatus, TearDownError};
use crate::db::{self, NotificationsLane, SealedConnection, Uninstall};

/// How long an uninstall is tried, an hour apart, before it is given up on.
const GIVE_UP_AFTER_SECS: i64 = 24 * 60 * 60;

/// An uninstall filed this recently is the one its delete is settling.
const SETTLED_AFTER_SECS: i64 = 60;

/// How many uninstalls one sweep tries: a vendor that hangs holds each a while.
const SWEEP_PAGE: i64 = 50;

/// Files an uninstall for each workspace `deleted` reached, with the token of
/// the first of its connections there not revoked, whose token still works.
pub(crate) async fn file<B: Binding>(
    tx: &mut Scoped<'_, B>,
    deleted: &[SealedConnection],
) -> sqlx::Result<Vec<Uninstall>> {
    let mut live: Vec<&SealedConnection> = deleted
        .iter()
        .filter(|c| c.provider.installed_per_workspace() && c.status != ConnectionStatus::Revoked)
        .collect();
    live.sort_by_key(|c| c.id);
    let mut first = BTreeMap::new();
    for connection in live {
        first
            .entry((
                connection.provider.as_str(),
                connection.external_workspace_id.as_str(),
            ))
            .or_insert(connection);
    }
    let mut filed = Vec::with_capacity(first.len());
    for connection in first.into_values() {
        filed.extend(db::file_uninstall(tx, connection).await?);
    }
    Ok(filed)
}

/// Settles `filed` on a task of its own, so that auth's budget or a dropped
/// request cannot cut it short; what does not settle is the sweep's.
pub(crate) fn settle_soon(state: &Arc<AppState>, filed: Vec<Uninstall>) {
    if filed.is_empty() {
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        for uninstall in &filed {
            if let Err(e) = settle(&state, uninstall).await {
                tracing::warn!(uninstall_id = %uninstall.id, error = %e,
                    "an uninstall could not be settled; the retention sweep tries again");
            }
        }
    });
}

/// Files and settles the uninstall an install refused after the vendor's exchange
/// may need, as a delete's: the app is installed, and its token held by nobody.
pub(crate) async fn file_refused(state: &Arc<AppState>, refused: &db::NewConnection<'_>) {
    let filed = async {
        let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
        let filed = db::file_uninstall_for(&mut tx, refused).await?;
        tx.commit().await?;
        Ok::<_, TelmoniError>(filed)
    }
    .await;
    match filed {
        Ok(filed) => settle_soon(state, filed.into_iter().collect()),
        Err(e) => tracing::error!(
            uninstall_id = %refused.id,
            provider = %refused.provider,
            error = %e,
            "a refused install's uninstall could not be filed; the app stays installed in its \
             workspace"
        ),
    }
}

/// What settling an uninstall came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// Sent, refused for good, or not needed: the row is gone.
    Done,
    /// It did not go through: the row stays, its try counted.
    Again,
}

/// Settles one uninstall: the workspace counted under its lock, the app
/// uninstalled outside any transaction where nothing uses it, the row removed
/// or the try counted.
pub async fn settle(state: &AppState, uninstall: &Uninstall) -> Result<Settled, TelmoniError> {
    // Opened before the count, so the vendor is asked right after it.
    let opened = open(state, uninstall).await;
    let provider = uninstall.provider;
    let workspace = uninstall.external_workspace_id.as_str();
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    db::lock_workspace(&mut tx, provider, workspace).await?;
    if db::count_workspace_connections(&mut tx, provider, workspace).await? > 0 {
        db::forget_uninstall(&mut tx, uninstall.id).await?;
        tx.commit().await?;
        tracing::info!(uninstall_id = %uninstall.id,
            "other connections share this workspace; the app stays installed");
        return Ok(Settled::Done);
    }
    tx.commit().await?;

    let sent = match opened {
        Ok((slack, token)) => slack.tear_down(&state.egress, &token).await,
        Err(why) => Err(why),
    };
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let settled = match sent {
        Ok(()) => {
            db::forget_uninstall(&mut tx, uninstall.id).await?;
            Settled::Done
        }
        Err(TearDownError::Refused(why)) => {
            tracing::warn!(uninstall_id = %uninstall.id, provider = %provider, error = %why,
                "the uninstall was refused for good; the app stays as the vendor has it");
            db::forget_uninstall(&mut tx, uninstall.id).await?;
            Settled::Done
        }
        Err(TearDownError::Unanswered(why)) => {
            tracing::warn!(uninstall_id = %uninstall.id, provider = %provider, error = %why,
                "the uninstall did not go through; it is tried again");
            match db::uninstall_tried(&mut tx, uninstall.id).await? {
                Some(_) => Settled::Again,
                None => Settled::Done,
            }
        }
    };
    tx.commit().await?;
    Ok(settled)
}

/// Slack's app, the one installed per workspace (the table's CHECK holds an
/// uninstall to it), and the opened token an uninstall is sent with.
async fn open(
    state: &AppState,
    uninstall: &Uninstall,
) -> Result<(Arc<SlackConnector>, Redacted), TearDownError> {
    let Some(slack) = state.connectors.slack.clone() else {
        return Err(TearDownError::Unanswered(format!(
            "{} is not configured on this replica",
            uninstall.provider
        )));
    };
    let Some(kek) = state.kek.as_ref() else {
        return Err(TearDownError::Unanswered(
            "CONNECTOR_KEK is not configured on this replica".into(),
        ));
    };
    let row = kek
        .open_row(
            &state.http,
            &state.vault,
            uninstall.id,
            &uninstall.wrapped_dek,
            uninstall.key_version,
        )
        .await
        .map_err(|e| match e {
            KekError::Unavailable(why) => {
                TearDownError::Unanswered(format!("kek unavailable: {why}"))
            }
            KekError::Invalid(why) => {
                TearDownError::Refused(format!("the wrapped key would not open: {why}"))
            }
        })?;
    let token = row
        .open(uninstall.token())
        .map_err(|e| TearDownError::Refused(format!("the token would not open: {e}")))?;
    Ok((slack, token))
}

/// What one sweep's pass over the filed uninstalls came to.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    pub settled: u64,
    pub again: u64,
    pub given_up: u64,
}

/// Gives up the uninstalls tried for a day, then settles a page of those
/// their deletes did not. Ends where Postgres stops answering.
pub async fn sweep(state: &AppState) -> Result<Swept, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let given_up = db::give_up_uninstalls(&mut tx, GIVE_UP_AFTER_SECS).await?;
    let due = db::uninstalls_due(&mut tx, SETTLED_AFTER_SECS, SWEEP_PAGE).await?;
    tx.commit().await?;
    for uninstall in &given_up {
        tracing::error!(uninstall_id = %uninstall.id, provider = %uninstall.provider,
            workspace = %uninstall.external_workspace_id, attempts = uninstall.attempts,
            "an app could not be uninstalled in a day of tries; given up, it stays installed \
             in the workspace");
    }
    let mut swept = Swept {
        given_up: u64::try_from(given_up.len()).unwrap_or(u64::MAX),
        ..Swept::default()
    };
    for uninstall in &due {
        match settle(state, uninstall).await {
            Ok(Settled::Done) => swept.settled += 1,
            Ok(Settled::Again) => swept.again += 1,
            Err(e) if e.is_database_unanswered() => return Err(e),
            Err(e) => {
                tracing::warn!(uninstall_id = %uninstall.id, error = %e,
                    "an uninstall could not be settled; the next sweep tries again");
                swept.again += 1;
            }
        }
    }
    Ok(swept)
}
