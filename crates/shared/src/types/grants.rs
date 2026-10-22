//! [`Grants`] — what an organization may take in, as a deployment that sells
//! plans answers it through [`crate::seam::Entitlements`].

use crate::error::{PlanError, TelmoniError};

/// What an organization may take in, as the deployment that sells plans
/// answers it ([`crate::seam::Entitlements`]); a binary that sells none
/// answers [`Grants::unlimited`] for every organization. A door reads it
/// before it takes in anything new and never before a read: an organization
/// keeps what it already holds, whatever its grants say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    /// Set while the organization is paused: it keeps its projects, its
    /// data and its members, and takes in nothing new, each door refusing
    /// with this. Why it is paused is the deployment's to say, in the
    /// refusal's own words.
    pub paused: Option<Paused>,
}

/// The refusal a paused organization's doors answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paused {
    /// The one thing that lifts it, the 402's `entitlement`.
    pub entitlement: String,
    /// The sentence a person reads, which names nobody: every member of the
    /// organization, and every agent sending to it, is answered with it.
    pub detail: String,
}

impl Grants {
    /// Everything, for every organization: a binary that sells nothing.
    #[must_use]
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// Whether something new may arrive for the organization: a span, a
    /// question, an invitation. Refused, as a 402, while it is paused.
    pub fn admit_new(&self) -> Result<(), TelmoniError> {
        match &self.paused {
            Some(paused) => Err(PlanError::LimitReached {
                entitlement: paused.entitlement.clone(),
                detail: paused.detail.clone(),
            }
            .into()),
            None => Ok(()),
        }
    }
}

/// How many people an organization may hold, as the deployment that sells
/// plans answers it ([`crate::seam::Entitlements::member_ceiling`]): its
/// members and everyone it has invited, counted together, since an
/// invitation is a seat promised. Held to where a person is promised one and
/// where they take it; never where one already seated is concerned, so a
/// roster past it keeps everybody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberCeiling {
    /// How many the organization may hold, those invited among them.
    pub limit: u32,
    /// The one thing that lifts it, the 402's `entitlement`.
    pub entitlement: String,
    /// The sentence a person reads, which names nobody: the inviter and
    /// the invited are answered with it alike.
    pub detail: String,
}

impl MemberCeiling {
    /// Whether one more person may be promised a seat, or take one, with
    /// `held` already held. Refused, as a 402, at the ceiling.
    pub fn admit(&self, held: u64) -> Result<(), TelmoniError> {
        if held < u64::from(self.limit) {
            return Ok(());
        }
        Err(PlanError::LimitReached {
            entitlement: self.entitlement.clone(),
            detail: self.detail.clone(),
        }
        .into())
    }
}

/// How many monitors an organization may hold across its projects, as the
/// deployment that sells plans answers it
/// ([`crate::seam::Entitlements::monitor_ceiling`]). Held where one is made,
/// by a person or by a first ping or run; never where one already made is
/// concerned, so a project past it keeps every monitor it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorCeiling {
    /// How many the organization may hold.
    pub limit: u32,
    /// The one thing that lifts it, the 402's `entitlement`.
    pub entitlement: String,
    /// The sentence a person reads, which names nobody.
    pub detail: String,
}

impl MonitorCeiling {
    /// Whether one more monitor may be made, with `held` already held.
    /// Refused, as a 402, at the ceiling.
    pub fn admit(&self, held: u64) -> Result<(), TelmoniError> {
        if held < u64::from(self.limit) {
            return Ok(());
        }
        Err(PlanError::LimitReached {
            entitlement: self.entitlement.clone(),
            detail: self.detail.clone(),
        }
        .into())
    }
}

/// How many minutes' worth of pings may come at once: five, so every monitor
/// a plan holds can ping its start and its end in the same moment, which is
/// what its ceiling is sized for, and one more for a clock running early.
pub const PING_BURST_MINUTES: u32 = 6;

/// What an answer to an organization's machines costs on top of its bytes:
/// the TLS connection a script opens for each request, as `PLANS.md`
/// measures a crontab's.
pub const CONNECTION_BYTES: u64 = 6_500;

/// Pings a minute, in millionths, as a schedule's need is summed: whole
/// numbers, so five monitors due every five minutes need exactly the two a
/// minute Hobby holds.
pub const MICRO_PINGS: u64 = 1_000_000;

/// The machine lanes' ceilings an organization's plan sets, as the
/// deployment that sells plans answers them
/// ([`crate::seam::Entitlements::lane_ceilings`]): pings a minute, the same
/// per ping key, and data out a UTC day. Neither is sold on its own, so a
/// refusal past one is a 429 that says when it lifts; a schedule that would
/// need more pings than the plan holds is a 402 naming what lifts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneCeilings {
    /// Pings a minute across the organization's projects, and per ping key.
    pub pings_per_minute: u32,
    /// Bytes a UTC day: what the read API and the CLI's lanes answer, and
    /// every export handed over.
    pub data_out_per_day: u64,
    /// What lifts the pings ceiling: the 402's `entitlement` a schedule past
    /// it is refused with.
    pub entitlement: String,
    /// The sentence that refusal carries, which names nobody.
    pub detail: String,
}

impl LaneCeilings {
    /// How many pings may come at once.
    #[must_use]
    pub const fn ping_burst(&self) -> u32 {
        self.pings_per_minute.saturating_mul(PING_BURST_MINUTES)
    }

    /// Whether the organization's schedules may come to `needed` pings a
    /// minute, in millionths, from the `before` they come to now: refused, as
    /// a 402, past the ceiling, unless the change asks no more than before,
    /// so an organization already past it — one that fell to a smaller plan —
    /// can always lower its schedules.
    pub fn admit_schedules(&self, before: u64, needed: u64) -> Result<(), TelmoniError> {
        if needed <= u64::from(self.pings_per_minute).saturating_mul(MICRO_PINGS)
            || needed <= before
        {
            return Ok(());
        }
        Err(PlanError::LimitReached {
            entitlement: self.entitlement.clone(),
            detail: self.detail.clone(),
        }
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_monitor_ceiling_admits_one_more_until_it_is_held() {
        let ceiling = MonitorCeiling {
            limit: 5,
            entitlement: "pro".into(),
            detail: "The plan holds five.".into(),
        };
        assert!(ceiling.admit(4).is_ok());
        let pd = ceiling
            .admit(5)
            .expect_err("a monitor past the ceiling")
            .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/plan/limit-reached");
        assert_eq!(pd.status, 402);
        assert_eq!(pd.detail.as_deref(), Some("The plan holds five."));
        assert!(ceiling.admit(u64::MAX).is_err());
    }

    fn hobby() -> LaneCeilings {
        LaneCeilings {
            pings_per_minute: 2,
            data_out_per_day: 5_000_000,
            entitlement: "pro".into(),
            detail: "Hobby's monitors ping two times a minute together.".into(),
        }
    }

    /// Five monitors due every five minutes need exactly what Hobby holds,
    /// and a burst of every start and end at once fits.
    #[test]
    fn schedules_up_to_the_ceiling_are_admitted_and_one_past_it_refused() {
        let ceiling = hobby();
        let every_five_minutes = 2 * MICRO_PINGS / 5;
        assert!(ceiling.admit_schedules(0, 5 * every_five_minutes).is_ok());
        assert!(
            ceiling.ping_burst() >= 10,
            "five starts and five ends at once"
        );
        let pd = ceiling
            .admit_schedules(5 * every_five_minutes, 5 * every_five_minutes + 1)
            .expect_err("one past the ceiling")
            .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/plan/limit-reached");
        assert_eq!(pd.status, 402);
        assert_eq!(
            pd.detail.as_deref(),
            Some("Hobby's monitors ping two times a minute together.")
        );
    }

    /// An organization already past its ceiling may lower its schedules,
    /// never raise them.
    #[test]
    fn past_the_ceiling_a_schedule_may_only_come_down() {
        let ceiling = hobby();
        let past = 10 * MICRO_PINGS;
        assert!(ceiling.admit_schedules(past, past - 1).is_ok());
        assert!(ceiling.admit_schedules(past, past).is_ok());
        assert!(ceiling.admit_schedules(past, past + 1).is_err());
    }

    #[test]
    fn a_binary_that_sells_nothing_admits_everything() {
        assert!(Grants::unlimited().admit_new().is_ok());
        assert_eq!(Grants::unlimited(), Grants::default());
    }

    #[test]
    fn a_paused_organization_is_refused_with_the_deployments_words() {
        let grants = Grants {
            paused: Some(Paused {
                entitlement: "pro".into(),
                detail: "This organization is paused.".into(),
            }),
        };
        let pd = grants
            .admit_new()
            .expect_err("a paused organization takes nothing new")
            .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/plan/limit-reached");
        assert_eq!(pd.status, 402);
        assert_eq!(pd.detail.as_deref(), Some("This organization is paused."));
        assert_eq!(
            pd.extensions
                .get("entitlement")
                .and_then(serde_json::Value::as_str),
            Some("pro")
        );
    }

    #[test]
    fn a_ceiling_admits_one_more_until_it_is_held_and_refuses_in_the_deployments_words() {
        let ceiling = MemberCeiling {
            limit: 2,
            entitlement: "pro".into(),
            detail: "The plan holds two.".into(),
        };
        assert!(ceiling.admit(0).is_ok());
        assert!(ceiling.admit(1).is_ok());
        let pd = ceiling
            .admit(2)
            .expect_err("a seat past the ceiling")
            .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/plan/limit-reached");
        assert_eq!(pd.status, 402);
        assert_eq!(pd.detail.as_deref(), Some("The plan holds two."));
        assert_eq!(
            pd.extensions
                .get("entitlement")
                .and_then(serde_json::Value::as_str),
            Some("pro")
        );
        assert!(
            ceiling.admit(u64::MAX).is_err(),
            "a roster past the ceiling promised a seat"
        );
    }
}
