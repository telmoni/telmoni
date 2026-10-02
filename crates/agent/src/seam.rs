//! What this module does for the modules beside it —
//! [`telmoni_shared::seam::Agent`]: auth calls it when a person or an
//! organization goes. What it holds of a project that left an organization,
//! it finds and removes itself ([`crate::retention`]).

use std::sync::Arc;

use async_trait::async_trait;
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::seam::Agent;
use telmoni_shared::{OrganizationId, TelmoniError, UserId};

use crate::AppState;
use crate::db::{self, AgentLane};

/// The module as auth holds it.
pub struct AgentSeam(pub Arc<AppState>);

/// How an address is found in free text: the whole address, never one
/// inside a longer one (`jo.ann@…`, `…@example.com.au`). A full stop after
/// it ends the sentence, not the address.
fn address_pattern(address: &str) -> String {
    format!(
        "(?<![[:alnum:]_.+-]){}(?![[:alnum:]_+-]|\\.[[:alnum:]])",
        literal_pattern(address)
    )
}

/// How a name is found: as whole words, never inside another ("Ed Li" in
/// "connected list"), and only a name of two words or more — one ("Ada")
/// is too many other people's.
fn name_pattern(name: &str) -> Option<String> {
    (name.split_whitespace().count() >= 2)
        .then(|| format!("(?<![[:alnum:]_]){}(?![[:alnum:]_])", literal_pattern(name)))
}

/// How many fence-and-scrub rounds one erase takes before it gives up and is
/// retried: a round only follows one whose scrub found the person somewhere
/// new, so a handful is far past what an erasure meets.
const ERASE_ROUNDS: usize = 8;

/// The erase itself: rounds of fence, scrub and close until a scrub finds
/// the person nowhere its fence did not reach.
///
/// ⚠ **Each round fences before it scrubs, and a scrub that finds the person
/// where its fence did not reach runs again behind one that does**: an
/// answer there could land between the scrub's reading and its commit,
/// neither scrubbed nor withheld. Every step is a short transaction of its
/// own: every save in the fenced organizations waits on the fence's locks.
async fn erase_rounds(
    state: Arc<AppState>,
    user_id: UserId,
    addresses: Vec<String>,
    names: Vec<String>,
    mut reached: Vec<String>,
) -> Result<u64, TelmoniError> {
    let mut gone = 0;
    for _ in 0..ERASE_ROUNDS {
        let fence = fence(&state, &user_id, &reached).await?;
        let scrubbed = db::erase_person(&state.db, &user_id, &fence, &addresses, &names).await;
        // Up again whether the scrub landed or not: a failed one is retried
        // behind a fence of its own, and until then every answer in these
        // organizations would be withheld.
        let closed = close(&state, &user_id, &fence).await;
        let (round, found) = scrubbed?;
        closed?;
        gone += round;
        let beyond: Vec<String> = found
            .into_iter()
            .filter(|o| fence.organizations.binary_search(o).is_err())
            .collect();
        if beyond.is_empty() {
            tracing::info!(user_id = %user_id, gone, "agent erasure: the person's conversations and passages removed");
            return Ok(gone);
        }
        reached = fence.organizations;
        reached.extend(beyond);
    }
    Err(TelmoniError::Internal(
        "the agent's erasure kept finding the person in more organizations; it is retried".into(),
    ))
}

/// How many times a fence is asked to go down, or up, before the erase
/// gives up on it. Each waits only the role's short lock wait for a key
/// (`db::fence_erasure`), so a busy moment is asked again, not waited out.
const FENCE_TRIES: u32 = 4;

/// Put a fence down, asked again after a pause when it fails.
async fn fence(state: &AppState, user_id: &UserId, reached: &[String]) -> sqlx::Result<db::Fence> {
    let mut tries = 1;
    loop {
        let fenced = async {
            let mut tx = maintenance_scope(&state.db, AgentLane).await?;
            let fence = db::fence_erasure(&mut tx, user_id, reached).await?;
            tx.commit().await?;
            Ok(fence)
        }
        .await;
        match fenced {
            Err(e) if tries < FENCE_TRIES => {
                tracing::info!(user_id = %user_id, error = %e, "an agent erasure's fence did not go down; asking again");
                tokio::time::sleep(std::time::Duration::from_secs(1 << tries)).await;
                tries += 1;
            }
            fenced => return fenced,
        }
    }
}

/// Lift a fence, asked again after a pause when it fails: a fence left down
/// holds every answer in its organizations until it lapses, a quarter of an
/// hour after the scrub's last step.
async fn close(state: &AppState, user_id: &UserId, fence: &db::Fence) -> sqlx::Result<()> {
    let mut tries = 1;
    loop {
        let closed = async {
            let mut tx = maintenance_scope(&state.db, AgentLane).await?;
            db::close_erasure(&mut tx, user_id, fence).await?;
            tx.commit().await
        }
        .await;
        match closed {
            Err(e) if tries < FENCE_TRIES => {
                tracing::info!(user_id = %user_id, error = %e, "an agent erasure's fence did not lift; asking again");
                tokio::time::sleep(std::time::Duration::from_secs(1 << tries)).await;
                tries += 1;
            }
            closed => return closed,
        }
    }
}

/// Rows of each kind an organization's purge removes per transaction.
const PURGE_CHUNK: i64 = 500;

/// A string as a case-insensitive regex matches it literally.
fn literal_pattern(name: &str) -> String {
    let mut out = String::with_capacity(name.len() * 2);
    for c in name.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[async_trait]
impl Agent for AgentSeam {
    async fn erase_person(
        &self,
        user_id: &UserId,
        address: &str,
        name: Option<&str>,
        organizations: &[OrganizationId],
    ) -> Result<u64, TelmoniError> {
        // Only the account's own address takes the address path, which has no
        // organization limit: a display name is free text, and one that held
        // an `@` would otherwise be scrubbed from every tenant.
        let address = address.trim();
        let addresses: Vec<String> = (address.contains('@'))
            .then(|| address_pattern(address))
            .into_iter()
            .collect();
        let names: Vec<String> = name
            .map(str::trim)
            .and_then(name_pattern)
            .into_iter()
            .collect();
        let reached: Vec<String> = organizations
            .iter()
            .map(|o| o.as_str().to_owned())
            .collect();
        // ⚠ **A task of its own, awaited.** The callers give up on it — the
        // account deletion's budget, the sweep's, a request the person left —
        // and a dropped erase between its fence and its close left every
        // answer in the person's organizations withheld until a retry came.
        // Spawned, it runs to its close whoever stops waiting.
        let state = self.0.clone();
        let user = user_id.clone();
        tokio::spawn(async move {
            let erased = erase_rounds(state, user.clone(), addresses, names, reached).await;
            // Said here: whoever stopped waiting for it never hears.
            if let Err(e) = &erased {
                tracing::warn!(user_id = %user, error = %e, "agent erasure failed; it is retried");
            }
            erased
        })
        .await
        .map_err(|e| TelmoniError::internal("the agent's erasure stopped", e))?
    }

    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError> {
        let mut tx = maintenance_scope(&self.0.db, AgentLane).await?;
        db::revoke_leases(&mut tx).await?;
        tx.commit().await?;
        let mut gone = 0;
        loop {
            let mut tx = maintenance_scope(&self.0.db, AgentLane).await?;
            let went = db::purge_organization(&mut tx, organization_id, PURGE_CHUNK).await?;
            tx.commit().await?;
            gone += went;
            if went == 0 {
                break;
            }
        }
        tracing::info!(organization_id = %organization_id, gone, "agent purge: the organization's rows removed");
        Ok(gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_full_name_is_scrubbed_and_every_pattern_matches_literally() {
        assert!(name_pattern("Ada Lovelace").is_some());
        assert!(name_pattern("Ada").is_none());
        assert_eq!(literal_pattern("a.b+c@x.com"), r"a\.b\+c@x\.com");
        assert!(address_pattern("a.b@x.com").contains(r"a\.b@x\.com"));
    }
}
