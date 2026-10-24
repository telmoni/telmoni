//! Platform error taxonomy + RFC 9457 problem-details mapping.

use crate::error_schema::ProblemDetails;

/// Platform error enum.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TelmoniError {
    /// Authentication failure.
    #[error(transparent)]
    Auth(#[from] AuthError),

    /// Authorization failure.
    #[error(transparent)]
    Authz(#[from] AuthzError),

    /// Tenant isolation failure.
    #[error(transparent)]
    Tenant(#[from] TenantError),

    /// Ingest's refusal of an export.
    #[error(transparent)]
    Telemetry(#[from] TelemetryError),

    /// A plan's refusal, from a deployment that sells plans.
    #[error(transparent)]
    Plan(#[from] PlanError),

    /// Database error (surfaced as redacted 500).
    #[error(transparent)]
    Database(#[from] sqlx::Error),

    /// Generic internal server error.
    #[error("internal: {0}")]
    Internal(String),

    /// Internal server error preserving root cause.
    #[error("internal: {context}")]
    InternalSource {
        /// Log context string.
        context: String,
        /// Wrapped root cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Upstream service problem details forwarded verbatim. Boxed: inline, its
    /// 128-odd bytes would set the size of every `Result<_, TelmoniError>`.
    #[error("upstream problem: {}", .0.status)]
    Upstream(Box<ProblemDetails>),

    /// Malformed request body or parameters.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// Client version below server minimum.
    #[error("client {client} below minimum {minimum}")]
    IncompatibleClient {
        /// Presented client version.
        client: String,
        /// Minimum supported version.
        minimum: String,
    },

    /// Outbound email delivery failure.
    #[error("mail delivery failed: {context}")]
    MailDelivery {
        /// Send context for logging.
        context: String,
    },

    /// A body larger than the lane accepts. Maps to **413**. The cap rides
    /// the problem as `max_bytes`, so a CLI can say what would have fit.
    #[error("payload larger than {max_bytes} bytes")]
    PayloadTooLarge {
        /// The most bytes this lane takes.
        max_bytes: u64,
    },

    /// The deployment has not configured the agent (`AGENT_MODEL_PROVIDER`
    /// unset). Maps to **503**: it is the operator's to switch on.
    #[error("the agent is not configured")]
    AgentDisabled,

    /// The model provider could not be reached, refused the call or timed
    /// out. Maps to **502**; the cause stays in the log.
    #[error("agent model unavailable: {context}")]
    AgentModelUnavailable {
        /// What failed, for the log.
        context: String,
        /// Whether asking again later could cure it, which the agent's
        /// retries read.
        failure: ModelFailure,
    },

    /// The person asked more than `AGENT_MESSAGES_PER_HOUR` questions in the
    /// last hour. Maps to **429**, with `retry_after_secs`.
    #[error("agent rate limited; retry after {retry_after_secs}s")]
    AgentRateLimited {
        /// Seconds until the oldest question in the window leaves it.
        retry_after_secs: u64,
    },
}

/// How a model endpoint failed, for whether to ask it again. It rides beside
/// the log's context, never inside it: read back out of the text, an error
/// code the provider chose could pass for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFailure {
    /// It answered, and the answer may be about what it was sent: a refusal,
    /// a 500, a body that does not parse.
    Answered,
    /// The endpoint's state, never what it was sent: no answer in time, no
    /// connection, a body cut short, or an answer that says "not now".
    NotNow,
    /// "Not now" as a rate limit, after the retries: waiting out its window
    /// may lift it.
    RateLimited,
}

impl ModelFailure {
    /// The word the log gives it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::NotNow => "not_now",
            Self::RateLimited => "rate_limited",
        }
    }
}

impl TelmoniError {
    /// Wrap an arbitrary error as an internal 500 while preserving its source
    /// chain for the logs. Neither `context` nor `source` reaches the customer.
    #[must_use]
    pub fn internal<E>(context: impl Into<String>, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::InternalSource {
            context: context.into(),
            source: Box::new(source),
        }
    }

    /// Classify a non-2xx answer from another service reached over HTTP.
    #[must_use]
    pub fn upstream(service: &str, op: &str, status: u16) -> Self {
        if status == 401 || status == 403 {
            tracing::error!(
                service,
                op,
                status,
                "our service credential was rejected by a sibling — check SERVICE_SECRET rotation"
            );
            return AuthError::ServiceCredentialRejected.into();
        }
        tracing::error!(service, op, status, "sibling service call failed");
        Self::Internal(format!("{service} {op} returned {status}"))
    }

    /// Whether the database did not answer at all, rather than refusing a
    /// statement, so a loop over many items stops: the pool's and the socket's
    /// errors, and a server going away — shutting down (`57P01`–`57P03`) or
    /// dropping the connection (class `08`) — which it says once before those take over.
    #[must_use]
    pub fn is_database_unanswered(&self) -> bool {
        match self {
            Self::Database(
                sqlx::Error::PoolTimedOut
                | sqlx::Error::PoolClosed
                | sqlx::Error::WorkerCrashed
                | sqlx::Error::Io(_)
                | sqlx::Error::Tls(_)
                | sqlx::Error::Protocol(_),
            ) => true,
            Self::Database(sqlx::Error::Database(e)) => e.code().is_some_and(|code| {
                code.starts_with("08") || matches!(&*code, "57P01" | "57P02" | "57P03")
            }),
            _ => false,
        }
    }

    /// Render this error as an RFC 9457 problem-details payload.
    #[must_use]
    pub fn to_problem_details(&self) -> ProblemDetails {
        match self {
            Self::Auth(e) => e.to_problem_details(),
            Self::Authz(e) => e.to_problem_details(),
            Self::Tenant(e) => e.to_problem_details(),
            Self::Telemetry(e) => e.to_problem_details(),
            Self::Plan(e) => e.to_problem_details(),
            Self::Database(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                AuthError::Conflict("That already exists. Refresh and try again.".into())
                    .to_problem_details()
            }
            Self::Database(_) => ProblemDetails {
                type_uri: "/errors/internal/database".into(),
                title: "internal server error".into(),
                status: 500,
                detail: None,
                ..Default::default()
            },
            Self::Internal(_) | Self::InternalSource { .. } => ProblemDetails {
                type_uri: "/errors/internal/unknown".into(),
                title: "internal server error".into(),
                status: 500,
                detail: None,
                ..Default::default()
            },
            Self::Upstream(pd) => (**pd).clone(),
            Self::BadRequest(detail) => ProblemDetails {
                type_uri: "/errors/bad-request".into(),
                title: "bad request".into(),
                status: 400,
                detail: Some(detail.clone()),
                ..Default::default()
            },
            Self::IncompatibleClient { client, minimum } => ProblemDetails {
                type_uri: "/errors/incompatible-client".into(),
                title: "incompatible client".into(),
                status: 426,
                detail: Some(format!(
                    "Telmoni CLI {client} is older than the minimum supported {minimum}; upgrade to continue"
                )),
                ..Default::default()
            },
            Self::PayloadTooLarge { max_bytes } => {
                let mut pd = ProblemDetails {
                    type_uri: "/errors/payload-too-large".into(),
                    title: "payload too large".into(),
                    status: 413,
                    detail: Some(format!(
                        "The upload is larger than this lane accepts ({max_bytes} bytes)."
                    )),
                    ..Default::default()
                };
                pd.extensions.insert(
                    "max_bytes".into(),
                    serde_json::Value::Number((*max_bytes).into()),
                );
                pd
            }
            Self::AgentDisabled => ProblemDetails {
                type_uri: "/errors/agent/disabled".into(),
                title: "agent not configured".into(),
                status: 503,
                detail: Some("the operator of this deployment has not configured the agent".into()),
                ..Default::default()
            },
            Self::AgentModelUnavailable { .. } => ProblemDetails {
                type_uri: "/errors/agent/model-unavailable".into(),
                title: "agent model unavailable".into(),
                status: 502,
                detail: Some(
                    "the model behind the agent did not answer; nothing was changed, try again \
                     shortly"
                        .into(),
                ),
                ..Default::default()
            },
            Self::AgentRateLimited { retry_after_secs } => {
                let mut pd = ProblemDetails {
                    type_uri: "/errors/agent/rate-limited".into(),
                    title: "agent rate limited".into(),
                    status: 429,
                    detail: Some(format!(
                        "you have asked the agent as many questions as this hour allows; \
                         retry after {retry_after_secs}s"
                    )),
                    ..Default::default()
                };
                pd.extensions.insert(
                    "retry_after_secs".into(),
                    serde_json::Value::Number((*retry_after_secs).into()),
                );
                pd
            }
            Self::MailDelivery { .. } => ProblemDetails {
                type_uri: "/errors/mail/delivery-failed".into(),
                title: "email delivery failed".into(),
                status: 502,
                detail: Some(
                    "the email could not be sent — delivery is misconfigured or the provider \
                     is unavailable on our side; nothing was lost, try again shortly"
                        .into(),
                ),
                ..Default::default()
            },
        }
    }
}

/// Authentication-domain errors (who are you).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthError {
    /// No or missing credentials. Maps to 401.
    #[error("unauthenticated")]
    Unauthenticated,

    /// Bearer/session token past expiry. Maps to 401.
    #[error("token expired")]
    TokenExpired,

    /// Token malformed, wrong signature, or unknown prefix. Maps to 401.
    #[error("invalid token")]
    InvalidToken,

    /// The inter-service `x-service-secret` was missing or wrong. Maps to
    /// 401 with its own type URI, so a botched `SERVICE_SECRET` rotation is
    /// told apart from a customer's rejected token rather than read as one.
    #[error("service credential rejected")]
    ServiceCredentialRejected,

    /// The person or key could not be checked: the provider's key set is
    /// unreadable with nothing cached, or auth could not read its own
    /// database, or is not yet up in this process. Maps to
    /// **503**, never a 401: a bearer nobody could look at is not a refused one.
    #[error("identity could not be confirmed")]
    IdentityUnavailable,

    /// Auth-scoped resource not in caller's reach (user, project, token). Maps to 404.
    #[error("not found: {0}")]
    NotFound(String),

    /// Auth-scoped client-side validation failure. Maps to 400.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// A URL asked for an organization that none may go by: not a slug's
    /// shape, a word the console's own paths use, or a blocked word. A type
    /// of its own, so a form asking for a name and a URL at once can say which
    /// of the two was refused. Maps to 400.
    #[error("bad url: {0}")]
    BadUrl(String),

    /// Auth-scoped state conflict (duplicate slug, conflicting email). Maps to 409.
    #[error("conflict: {0}")]
    Conflict(String),

    /// The address's domain signs in through the deployment's identity
    /// provider alone (`AUTH_DOMAINS_WITH_SSO_ENFORCEMENT`): the login form
    /// refuses it, and the sign-in page offers the provider instead. Maps to
    /// 403.
    #[error("single sign-on required: {0}")]
    SsoRequired(String),

    /// A new person refused while the deployment admits only those invited
    /// (`AUTH_DISABLE_SIGNUP`), at the form or at a provider: its own type, so
    /// the console can say so after a provider's round trip. Maps to 403.
    #[error("sign-ups closed: {0}")]
    SignUpClosed(String),
}

impl AuthError {
    /// Render this error as RFC 9457 problem-details.
    #[must_use]
    pub fn to_problem_details(&self) -> ProblemDetails {
        let (slug, status, title) = match self {
            Self::Unauthenticated => ("unauthenticated", 401, "unauthenticated"),
            Self::TokenExpired => ("token-expired", 401, "token expired"),
            Self::InvalidToken => ("invalid-token", 401, "invalid token"),
            Self::ServiceCredentialRejected => (
                "service-credential-rejected",
                401,
                "service credential rejected",
            ),
            Self::IdentityUnavailable => (
                "identity-unavailable",
                503,
                "identity temporarily unavailable",
            ),
            Self::NotFound(_) => ("not-found", 404, "not found"),
            Self::BadRequest(_) => ("bad-request", 400, "bad request"),
            Self::BadUrl(_) => ("bad-url", 400, "bad url"),
            Self::Conflict(_) => ("conflict", 409, "conflict"),
            Self::SsoRequired(_) => ("sso-required", 403, "single sign-on required"),
            Self::SignUpClosed(_) => ("sign-up-closed", 403, "sign-ups closed"),
        };
        let detail = match self {
            Self::NotFound(m)
            | Self::BadRequest(m)
            | Self::BadUrl(m)
            | Self::Conflict(m)
            | Self::SsoRequired(m)
            | Self::SignUpClosed(m) => Some(m.clone()),
            _ => None,
        };
        ProblemDetails {
            type_uri: format!("/errors/auth/{slug}"),
            title: title.into(),
            status,
            detail,
            ..Default::default()
        }
    }
}

/// Authorisation-domain errors (credentials valid, action denied).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthzError {
    /// Generic forbidden — credentials valid, the action is not permitted. Maps to 403.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// Role-gated action; caller's role is insufficient. Maps to 403.
    #[error("role {actual} insufficient (need {required})")]
    InsufficientRole {
        /// Role required by the action.
        required: crate::Role,
        /// Caller's actual role.
        actual: crate::Role,
    },
}

impl AuthzError {
    /// Render this error as RFC 9457 problem-details.
    #[must_use]
    pub fn to_problem_details(&self) -> ProblemDetails {
        let (slug, title, detail) = match self {
            Self::Forbidden(m) => ("forbidden", "forbidden", Some(m.clone())),
            Self::InsufficientRole { required, actual } => (
                "insufficient-role",
                "insufficient role",
                Some(format!("required {required}, have {actual}")),
            ),
        };
        ProblemDetails {
            type_uri: format!("/errors/authz/{slug}"),
            title: title.into(),
            status: 403,
            detail,
            ..Default::default()
        }
    }
}

/// Ingest's refusals of an export to `/otlp/v1/traces`. On that lane the
/// body is OTLP's `Status` rather than a problem document, its message
/// opening with the type, so an exporter and a person read the same name.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TelemetryError {
    /// The body could not be read or did not arrive within the lane's wait,
    /// did not decode as OTLP, or held no span or more than the batch allows. Maps to 400.
    #[error("invalid batch: {0}")]
    InvalidBatch(String),

    /// Past the batch's cap once decompressed. Maps to 413, with the cap as
    /// `max_bytes`.
    #[error("batch too large: more than {max_bytes} bytes once decompressed")]
    BatchTooLarge {
        /// The most bytes an export may unfold to.
        max_bytes: usize,
    },

    /// ClickHouse, or the key service a project's hashing key is sealed
    /// under, did not answer, or a move or a purge held the project as its
    /// first export went to make that key: nothing was kept. Maps to 503,
    /// with a `Retry-After` of one second, the answer an exporter retries.
    #[error("store unavailable: {0}")]
    StoreUnavailable(String),
}

impl TelemetryError {
    /// Render this error as RFC 9457 problem-details.
    #[must_use]
    pub fn to_problem_details(&self) -> ProblemDetails {
        match self {
            Self::InvalidBatch(detail) => ProblemDetails {
                type_uri: "/errors/telemetry/invalid-batch".into(),
                title: "invalid batch".into(),
                status: 400,
                detail: Some(detail.clone()),
                ..Default::default()
            },
            Self::BatchTooLarge { max_bytes } => {
                let mut pd = ProblemDetails {
                    type_uri: "/errors/telemetry/batch-too-large".into(),
                    title: "batch too large".into(),
                    status: 413,
                    detail: Some(format!(
                        "the body is larger than {max_bytes} bytes once decompressed"
                    )),
                    ..Default::default()
                };
                pd.extensions
                    .insert("max_bytes".into(), serde_json::Value::from(*max_bytes));
                pd
            }
            Self::StoreUnavailable(detail) => {
                let mut pd = ProblemDetails {
                    type_uri: "/errors/telemetry/store-unavailable".into(),
                    title: "store unavailable".into(),
                    status: 503,
                    detail: Some(detail.clone()),
                    ..Default::default()
                };
                pd.extensions
                    .insert("retry_after_secs".into(), serde_json::Value::from(1_u64));
                pd
            }
        }
    }
}

/// A plan's refusals. Nothing in the core decides one: they come from a
/// deployment that sells plans, through [`crate::seam::Entitlements`] and
/// [`crate::seam::OwnershipHook`], so a binary that sells none never answers
/// one.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlanError {
    /// The organization's plan does not hold what was asked. Maps to **402**,
    /// with the one thing that lifts it as `entitlement`.
    #[error("plan limit reached; lifted by {entitlement}")]
    LimitReached {
        /// The one thing that lifts the refusal. Rides the problem as the
        /// `entitlement` extension, for a program to switch on.
        entitlement: String,
        /// The sentence a person reads, in the deployment's words.
        detail: String,
    },
}

impl PlanError {
    /// Render this error as RFC 9457 problem-details.
    #[must_use]
    pub fn to_problem_details(&self) -> ProblemDetails {
        match self {
            Self::LimitReached {
                entitlement,
                detail,
            } => {
                let mut pd = ProblemDetails {
                    type_uri: "/errors/plan/limit-reached".into(),
                    title: "plan limit reached".into(),
                    status: 402,
                    detail: Some(detail.clone()),
                    ..Default::default()
                };
                pd.extensions.insert(
                    "entitlement".into(),
                    serde_json::Value::String(entitlement.clone()),
                );
                pd
            }
        }
    }
}

/// Tenant-isolation errors (rate-limit, cross-project access, feature flags).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TenantError {
    /// Per-project rate-limit window exceeded. Maps to 429 (carries `Retry-After`).
    #[error("rate limited; retry after {retry_after_secs}s")]
    RateLimited {
        /// Seconds the caller should wait before retrying.
        retry_after_secs: u64,
    },

    /// Past a ceiling the organization's plan sets on a machine lane — data
    /// out a day — never sold on its own, so the refusal says when it lifts.
    /// Maps to 429 (carries `Retry-After`), as a rate limit.
    #[error("past the plan's ceiling; retry after {retry_after_secs}s")]
    PastCeiling {
        /// The sentence the caller reads: which ceiling, and when it lifts.
        detail: String,
        /// Seconds until it lifts.
        retry_after_secs: u64,
    },

    /// Action would cross an project boundary that isn't permitted. Maps to 403.
    #[error("cross-tenant access denied")]
    CrossTenantDenied,

    /// A feature flag is OFF for this caller. Maps to **503**
    #[error("feature `{flag}` is off")]
    FeatureOff {
        /// Which switch. Rides the problem as the `flag` extension, for a
        /// program to switch on; the prose never prints the key.
        flag: crate::Flag,
    },
}

impl TenantError {
    /// Render this error as RFC 9457 problem-details.
    #[must_use]
    pub fn to_problem_details(&self) -> ProblemDetails {
        match self {
            Self::RateLimited { retry_after_secs } => {
                let mut pd = ProblemDetails {
                    type_uri: "/errors/tenant/rate-limited".into(),
                    title: "rate limited".into(),
                    status: 429,
                    detail: Some(format!("retry after {retry_after_secs}s")),
                    ..Default::default()
                };
                pd.extensions.insert(
                    "retry_after_secs".into(),
                    serde_json::Value::Number((*retry_after_secs).into()),
                );
                pd
            }
            Self::PastCeiling {
                detail,
                retry_after_secs,
            } => {
                let mut pd = ProblemDetails {
                    type_uri: "/errors/tenant/rate-limited".into(),
                    title: "rate limited".into(),
                    status: 429,
                    detail: Some(detail.clone()),
                    ..Default::default()
                };
                pd.extensions.insert(
                    "retry_after_secs".into(),
                    serde_json::Value::Number((*retry_after_secs).into()),
                );
                pd
            }
            Self::CrossTenantDenied => ProblemDetails {
                type_uri: "/errors/tenant/cross-tenant-denied".into(),
                title: "cross-tenant access denied".into(),
                status: 403,
                detail: None,
                ..Default::default()
            },
            Self::FeatureOff { flag } => {
                let mut pd = ProblemDetails {
                    type_uri: "/errors/tenant/feature-off".into(),
                    title: "feature switched off".into(),
                    status: 503,
                    detail: Some(flag.off_detail().into()),
                    ..Default::default()
                };
                pd.extensions.insert(
                    "flag".into(),
                    serde_json::Value::String(flag.as_str().into()),
                );
                pd.extensions.insert(
                    "retry_after_secs".into(),
                    serde_json::Value::Number(FEATURE_OFF_RETRY_SECS.into()),
                );
                pd
            }
        }
    }
}

/// The `Retry-After` a switched-off lane answers with.
pub const FEATURE_OFF_RETRY_SECS: u64 = 60;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_errors_map_to_stable_type_uris_and_statuses() {
        let cases: &[(AuthError, &str, u16, &str)] = &[
            (
                AuthError::Unauthenticated,
                "/errors/auth/unauthenticated",
                401,
                "unauthenticated",
            ),
            (
                AuthError::TokenExpired,
                "/errors/auth/token-expired",
                401,
                "token expired",
            ),
            (
                AuthError::InvalidToken,
                "/errors/auth/invalid-token",
                401,
                "invalid token",
            ),
            (
                AuthError::ServiceCredentialRejected,
                "/errors/auth/service-credential-rejected",
                401,
                "service credential rejected",
            ),
            (
                AuthError::IdentityUnavailable,
                "/errors/auth/identity-unavailable",
                503,
                "identity temporarily unavailable",
            ),
            (
                AuthError::NotFound("user".into()),
                "/errors/auth/not-found",
                404,
                "not found",
            ),
            (
                AuthError::BadRequest("bad slug".into()),
                "/errors/auth/bad-request",
                400,
                "bad request",
            ),
            (
                AuthError::BadUrl("not a slug".into()),
                "/errors/auth/bad-url",
                400,
                "bad url",
            ),
            (
                AuthError::Conflict("slug taken".into()),
                "/errors/auth/conflict",
                409,
                "conflict",
            ),
        ];
        for (err, type_uri, status, title) in cases {
            let pd = err.to_problem_details();
            assert_eq!(pd.type_uri, *type_uri);
            assert_eq!(pd.status, *status);
            assert_eq!(pd.title, *title);
        }
    }

    #[test]
    fn auth_message_variants_carry_detail_credential_variants_do_not() {
        let pd = AuthError::BadRequest("slug too long".into()).to_problem_details();
        assert_eq!(pd.detail.as_deref(), Some("slug too long"));
        assert!(
            AuthError::InvalidToken
                .to_problem_details()
                .detail
                .is_none()
        );
        assert!(
            AuthError::TokenExpired
                .to_problem_details()
                .detail
                .is_none()
        );
    }

    #[test]
    fn authz_errors_are_403_with_detail() {
        let pd = AuthzError::Forbidden("not your project".into()).to_problem_details();
        assert_eq!(pd.type_uri, "/errors/authz/forbidden");
        assert_eq!(pd.status, 403);
        assert_eq!(pd.detail.as_deref(), Some("not your project"));

        let pd = AuthzError::InsufficientRole {
            required: crate::Role::Admin,
            actual: crate::Role::Member,
        }
        .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/authz/insufficient-role");
        assert_eq!(pd.status, 403);
        assert_eq!(pd.detail.as_deref(), Some("required admin, have member"));
    }

    /// A server going away is unanswered; a refusal, a wait ended or a deadlock is not.
    #[test]
    fn a_server_going_away_is_unanswered_and_a_refusal_is_not() {
        #[derive(Debug)]
        struct Said(&'static str);
        impl std::fmt::Display for Said {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        impl std::error::Error for Said {}
        impl sqlx::error::DatabaseError for Said {
            fn message(&self) -> &str {
                self.0
            }
            fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
                Some(std::borrow::Cow::Borrowed(self.0))
            }
            fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
                self
            }
            fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
                self
            }
            fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
                self
            }
            fn kind(&self) -> sqlx::error::ErrorKind {
                sqlx::error::ErrorKind::Other
            }
        }
        let said = |code| TelmoniError::Database(sqlx::Error::Database(Box::new(Said(code))));
        for code in ["57P01", "57P02", "57P03", "08000", "08003", "08006"] {
            assert!(said(code).is_database_unanswered(), "{code}");
        }
        for code in ["55P03", "57014", "40P01", "23505", "42501"] {
            assert!(!said(code).is_database_unanswered(), "{code}");
        }
        assert!(TelmoniError::Database(sqlx::Error::PoolTimedOut).is_database_unanswered());
        assert!(!TelmoniError::Database(sqlx::Error::RowNotFound).is_database_unanswered());
    }

    #[test]
    fn database_and_internal_are_redacted_500s() {
        let pd = TelmoniError::Database(sqlx::Error::PoolTimedOut).to_problem_details();
        assert_eq!(pd.type_uri, "/errors/internal/database");
        assert_eq!(pd.status, 500);
        assert!(pd.detail.is_none());

        let pd = TelmoniError::Internal("schema `auth` unreachable".into()).to_problem_details();
        assert_eq!(pd.type_uri, "/errors/internal/unknown");
        assert_eq!(pd.status, 500);
        assert_eq!(pd.title, "internal server error");
        assert!(pd.detail.is_none());
    }

    #[test]
    fn mail_delivery_is_a_distinct_502_with_a_redacted_detail() {
        let pd = TelmoniError::MailDelivery {
            context: "deletion code to user@example.com".into(),
        }
        .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/mail/delivery-failed");
        assert_eq!(pd.status, 502);
        assert_eq!(pd.title, "email delivery failed");
        let detail = pd.detail.expect("static detail present");
        assert!(!detail.contains("example.com"));
    }

    #[test]
    fn rate_limited_exposes_retry_after_extension() {
        let pd = TenantError::RateLimited {
            retry_after_secs: 30,
        }
        .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/tenant/rate-limited");
        assert_eq!(pd.status, 429);
        assert_eq!(
            pd.extensions
                .get("retry_after_secs")
                .and_then(serde_json::Value::as_u64),
            Some(30)
        );
    }

    /// A plan's ceiling is refused as a rate limit is, saying which and when it lifts.
    #[test]
    fn past_a_ceiling_is_a_rate_limit_with_its_own_sentence() {
        let pd = TenantError::PastCeiling {
            detail: "This organization's data out for the day is spent.".into(),
            retry_after_secs: 3_600,
        }
        .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/tenant/rate-limited");
        assert_eq!(pd.status, 429);
        assert_eq!(
            pd.detail.as_deref(),
            Some("This organization's data out for the day is spent.")
        );
        assert_eq!(
            pd.extensions
                .get("retry_after_secs")
                .and_then(serde_json::Value::as_u64),
            Some(3_600)
        );
    }

    /// A sibling rejecting OUR service secret must be distinguishable from a
    /// sibling that is merely broken, or a half-finished rotation surfaces
    /// fleet-wide as anonymous 500s that read like an outage.
    #[test]
    fn a_sibling_rejecting_our_secret_is_told_apart_from_a_sibling_being_broken() {
        for status in [401u16, 403] {
            let pd = TelmoniError::upstream("notifications", "purge", status).to_problem_details();
            assert_eq!(
                pd.type_uri, "/errors/auth/service-credential-rejected",
                "{status} must name the credential, not a generic failure"
            );
        }
        for status in [500u16, 502, 404, 429] {
            let pd = TelmoniError::upstream("notifications", "purge", status).to_problem_details();
            assert_ne!(
                pd.type_uri, "/errors/auth/service-credential-rejected",
                "{status} is not a credential failure"
            );
        }
    }

    /// The model's own error text is the log's, never the customer's: a
    /// provider echoes request fragments back in its refusals.
    #[test]
    fn agent_errors_map_to_their_statuses_without_the_cause() {
        let pd = TelmoniError::AgentDisabled.to_problem_details();
        assert_eq!(
            (pd.type_uri.as_str(), pd.status),
            ("/errors/agent/disabled", 503)
        );

        let pd = TelmoniError::AgentModelUnavailable {
            context: "anthropic 401: invalid x-api-key sk-ant-abc".into(),
            failure: ModelFailure::Answered,
        }
        .to_problem_details();
        assert_eq!(
            (pd.type_uri.as_str(), pd.status),
            ("/errors/agent/model-unavailable", 502)
        );
        assert!(!pd.detail.unwrap_or_default().contains("sk-ant"));

        let pd = TelmoniError::AgentRateLimited {
            retry_after_secs: 90,
        }
        .to_problem_details();
        assert_eq!(pd.status, 429);
        assert_eq!(
            pd.extensions
                .get("retry_after_secs")
                .and_then(serde_json::Value::as_u64),
            Some(90)
        );
    }

    /// The refusal is the deployment's to word; the core carries its words
    /// and the one thing that lifts it, and invents neither.
    #[test]
    fn a_plan_limit_is_a_402_naming_what_lifts_it() {
        let pd = TelmoniError::from(PlanError::LimitReached {
            entitlement: "pro".into(),
            detail: "Move it to Pro.".into(),
        })
        .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/plan/limit-reached");
        assert_eq!(pd.status, 402);
        assert_eq!(pd.title, "plan limit reached");
        assert_eq!(pd.detail.as_deref(), Some("Move it to Pro."));
        assert_eq!(
            pd.extensions
                .get("entitlement")
                .and_then(serde_json::Value::as_str),
            Some("pro")
        );
    }

    #[test]
    fn cross_tenant_denied_is_403() {
        let pd = TenantError::CrossTenantDenied.to_problem_details();
        assert_eq!(pd.type_uri, "/errors/tenant/cross-tenant-denied");
        assert_eq!(pd.status, 403);
    }

    #[test]
    fn internal_source_preserves_the_cause_chain() {
        use std::error::Error;
        let src = std::io::Error::other("boom");
        let err = TelmoniError::internal("widget load", src);
        assert_eq!(err.to_problem_details().status, 500);
        assert!(err.to_problem_details().detail.is_none());
        assert_eq!(
            err.source().map(ToString::to_string).as_deref(),
            Some("boom")
        );
    }

    #[test]
    fn upstream_problem_is_forwarded_verbatim() {
        let sibling = ProblemDetails {
            type_uri: "/errors/tenant/rate-limited".into(),
            title: "rate limited".into(),
            status: 429,
            detail: Some("retry after 5s".into()),
            ..Default::default()
        };
        let pd = TelmoniError::Upstream(Box::new(sibling)).to_problem_details();
        assert_eq!(pd.status, 429);
        assert_eq!(pd.type_uri, "/errors/tenant/rate-limited");
        assert_eq!(pd.detail.as_deref(), Some("retry after 5s"));
    }

    #[test]
    fn a_closed_sign_up_is_403_of_its_own_type() {
        let pd = TelmoniError::from(AuthError::SignUpClosed(
            "sign-ups are closed; ask somebody to invite you".into(),
        ))
        .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/auth/sign-up-closed");
        assert_eq!(pd.status, 403);
        assert_eq!(
            pd.detail.as_deref(),
            Some("sign-ups are closed; ask somebody to invite you")
        );
    }

    #[test]
    fn bad_request_is_400_with_detail() {
        let pd = TelmoniError::BadRequest("unknown field `surprise`".into()).to_problem_details();
        assert_eq!(pd.type_uri, "/errors/bad-request");
        assert_eq!(pd.status, 400);
        assert_eq!(pd.detail.as_deref(), Some("unknown field `surprise`"));
    }

    #[test]
    fn incompatible_client_is_426_with_actionable_detail() {
        let pd = TelmoniError::IncompatibleClient {
            client: "0.0.5".into(),
            minimum: "0.1.0".into(),
        }
        .to_problem_details();
        assert_eq!(pd.type_uri, "/errors/incompatible-client");
        assert_eq!(pd.status, 426);
        assert_eq!(pd.title, "incompatible client");
        assert_eq!(
            pd.detail.as_deref(),
            Some(
                "Telmoni CLI 0.0.5 is older than the minimum supported 0.1.0; upgrade to continue"
            )
        );
    }
}
