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
    /// Set while the organization is dormant: it keeps its projects, its
    /// data and its members, and takes in nothing new, each door refusing
    /// with this. Why it is dormant is the deployment's to say, in the
    /// refusal's own words.
    pub dormant: Option<Dormant>,
}

/// The refusal a dormant organization's doors answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dormant {
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
    /// question, an invitation. Refused, as a 402, while it is dormant.
    pub fn admit_new(&self) -> Result<(), TelmoniError> {
        match &self.dormant {
            Some(dormant) => Err(PlanError::LimitReached {
                entitlement: dormant.entitlement.clone(),
                detail: dormant.detail.clone(),
            }
            .into()),
            None => Ok(()),
        }
    }
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
    fn a_dormant_organization_is_refused_with_the_deployments_words() {
        let grants = Grants {
            dormant: Some(Dormant {
                entitlement: "pro".into(),
                detail: "This organization is dormant.".into(),
            }),
        };
        let pd = grants
            .admit_new()
            .expect_err("a dormant organization takes nothing new")
            .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/plan/limit-reached");
        assert_eq!(pd.status, 402);
        assert_eq!(pd.detail.as_deref(), Some("This organization is dormant."));
        assert_eq!(
            pd.extensions
                .get("entitlement")
                .and_then(serde_json::Value::as_str),
            Some("pro")
        );
    }
}
