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
            Some(paused) => Err(limit_reached(&paused.entitlement, &paused.detail)),
            None => Ok(()),
        }
    }
}

/// The 402 a plan's refusal answers: what lifts it, and the sentence read.
fn limit_reached(entitlement: &str, detail: &str) -> TelmoniError {
    PlanError::LimitReached {
        entitlement: entitlement.to_owned(),
        detail: detail.to_owned(),
    }
    .into()
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
    /// Whether `held` fills it: the one rule, which the roster pages read
    /// rather than restate.
    pub fn reached(&self, held: u64) -> bool {
        held >= u64::from(self.limit)
    }

    /// Whether one more person may be promised a seat, or take one, with
    /// `held` already held. Refused, as a 402, at the ceiling.
    pub fn admit(&self, held: u64) -> Result<(), TelmoniError> {
        if !self.reached(held) {
            return Ok(());
        }
        Err(limit_reached(&self.entitlement, &self.detail))
    }
}

/// An organization's alerts and their deliveries, as the deployment that
/// sells plans answers them ([`crate::seam::Entitlements::alert_ceilings`]):
/// how many alerts it may hold across its projects, paused ones counted,
/// held when one is made; and how many notices its alerts may send its
/// connectors a UTC day, the feed never counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertCeilings {
    /// Alerts across its projects, paused ones among them.
    pub alerts: u32,
    /// Deliveries a UTC day to its connectors.
    pub deliveries_per_day: u32,
    /// The one thing that lifts them, the 402's `entitlement`.
    pub entitlement: String,
    /// The sentence a person reads, which names nobody.
    pub detail: String,
}

impl AlertCeilings {
    /// Whether one more alert may be made, with `held` already held. Refused,
    /// as a 402, at the ceiling.
    pub fn admit(&self, held: u64) -> Result<(), TelmoniError> {
        if held < u64::from(self.alerts) {
            return Ok(());
        }
        Err(limit_reached(&self.entitlement, &self.detail))
    }
}

/// What an answer to an organization's machines costs on top of its bytes:
/// the TLS connection a script opens for each request, measured at 6.5 KB.
pub const CONNECTION_BYTES: u64 = 6_500;

/// The machine lanes' ceiling an organization's plan sets, as the deployment
/// that sells plans answers it
/// ([`crate::seam::Entitlements::lane_ceilings`]): data out a UTC day. It is
/// not sold on its own, so a refusal past it is a 429 that says when it
/// lifts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneCeilings {
    /// Bytes a UTC day: what the read API and the CLI's lanes answer, and
    /// every export handed over.
    pub data_out_per_day: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn an_alert_past_the_plans_number_is_refused_in_the_deployments_words() {
        let ceilings = AlertCeilings {
            alerts: 2,
            deliveries_per_day: 500,
            entitlement: "pro".into(),
            detail: "Hobby holds two alerts.".into(),
        };
        assert!(ceilings.admit(1).is_ok());
        let pd = ceilings
            .admit(2)
            .expect_err("the third of two")
            .to_problem_details();
        assert_eq!(
            (pd.status, pd.type_uri.as_str()),
            (402, "/errors/plan/limit-reached")
        );
        assert_eq!(pd.detail.as_deref(), Some("Hobby holds two alerts."));
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
        assert!(!ceiling.reached(1));
        assert!(ceiling.reached(2));
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
