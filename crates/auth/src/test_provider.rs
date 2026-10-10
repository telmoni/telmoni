//! In-process test doubles for the integration suites: a scripted
//! [`AuthProvider`] whose every call is recorded and every answer scripted
//! or a sensible default, the issuer a suite mints sessions with and the
//! bearers it signs people in by, and the modules beside auth —
//! notifications, telemetry and the purge hook — as recorders a suite
//! scripts the same way.
//!
//! The router's tests exercise the lanes around the provider — the exchange
//! cache, the session rows, the deletion saga, the email-change pair — and
//! none of that depends on which provider is wired in. Scripting the trait
//! directly keeps those tests about the lanes; the provider implementations
//! test their own wire formats against HTTP stubs.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;

use telmoni_shared::db::tenant_session::person_scope;
use telmoni_shared::seam::{Emitted, Notice, Notifications, PurgeHook, Telemetry};
use telmoni_shared::{NotificationKind, OrganizationId, ProjectId, TelmoniError, UserId};

use crate::db::access_tokens;
use crate::issuer::{BEARER_TTL_SECS, Issuer, hash, secret};
use crate::provider::{
    AuthProvider, Authenticated, ConfirmedEmail, EmailChangeChallenge, PasswordResetLink, Subject,
};

/// One call auth made on a module beside it, as the recorders keep it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiblingCall {
    /// `Notifications::emit`.
    Emit {
        /// The organization the notice is for.
        organization_id: String,
        /// The project, when the notice is a project's.
        project_id: Option<String>,
        /// The wire-stable kind.
        kind: NotificationKind,
        /// The person the notice names.
        subject_user_id: Option<String>,
        /// The one-line title.
        title: String,
    },
    /// `Notifications::purge_organization`.
    PurgeOrganization(String),
    /// `Notifications::purge_project`.
    PurgeProject(String),
    /// `Notifications::redact_person`.
    RedactPerson(String),
    /// `Telemetry::purge_organization`.
    TelemetryPurgeOrganization(String),
    /// `Telemetry::purge_project`.
    TelemetryPurgeProject(String),
    /// `Telemetry::move_project`.
    TelemetryMove {
        /// The project moved.
        project_id: String,
        /// The organization its settings were moved to.
        organization_id: String,
    },
    /// `PurgeHook::purge_organization`.
    HookPurge(String),
}

/// The order every call landed in, across both recorders when they share
/// one: the finalize's purges are asserted as a sequence.
#[derive(Clone, Default)]
pub struct SiblingCalls(Arc<Mutex<Vec<SiblingCall>>>);

impl SiblingCalls {
    fn record(&self, call: SiblingCall) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
    }

    /// Every call so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<SiblingCall> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// How many recorded calls `pred` admits.
    pub fn count(&self, pred: impl Fn(&SiblingCall) -> bool) -> usize {
        self.calls().iter().filter(|c| pred(c)).count()
    }

    /// Forget every call so far.
    pub fn reset(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

/// A queue of scripted answers: the front is spent first, and an empty
/// queue answers the default.
struct Answers<T>(Mutex<VecDeque<Result<T, TelmoniError>>>);

impl<T> Default for Answers<T> {
    fn default() -> Self {
        Self(Mutex::new(VecDeque::new()))
    }
}

impl<T> Answers<T> {
    fn push(&self, answer: Result<T, TelmoniError>) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(answer);
    }

    fn take(&self) -> Option<Result<T, TelmoniError>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }
}

/// What a failing sibling answers.
fn sibling_down(what: &str) -> TelmoniError {
    TelmoniError::Internal(format!("{what} is down"))
}

/// A suite's hook, run on every call as it lands.
type Observer = Box<dyn Fn(&SiblingCall) + Send + Sync>;

/// Notifications as auth holds it, recording every call. Unscripted, every
/// purge and redaction answers `Ok(1)` and every emit lands; a suite scripts
/// the failures it wants and a delay for a race it stages.
pub struct RecordingNotifications {
    calls: SiblingCalls,
    purge_organization: Answers<u64>,
    purge_project: Answers<u64>,
    redact_person: Answers<u64>,
    down: std::sync::atomic::AtomicBool,
    delay: Mutex<Option<Duration>>,
    observer: Mutex<Option<Observer>>,
}

impl RecordingNotifications {
    /// A recorder writing its calls to `calls`.
    #[must_use]
    pub fn new(calls: SiblingCalls) -> Self {
        Self {
            calls,
            purge_organization: Answers::default(),
            purge_project: Answers::default(),
            redact_person: Answers::default(),
            down: std::sync::atomic::AtomicBool::new(false),
            delay: Mutex::new(None),
            observer: Mutex::new(None),
        }
    }

    /// Fail every purge and redaction from now on, past whatever is scripted.
    pub fn down(&self) -> &Self {
        self.down.store(true, std::sync::atomic::Ordering::SeqCst);
        self
    }

    /// The unscripted answer: `Ok(1)`, or the failure while down.
    fn default_answer(&self) -> Result<u64, TelmoniError> {
        if self.down.load(std::sync::atomic::Ordering::SeqCst) {
            Err(sibling_down("notifications"))
        } else {
            Ok(1)
        }
    }

    /// Script the next `purge_organization` answer.
    pub fn on_purge_organization(&self, answer: Result<u64, TelmoniError>) -> &Self {
        self.purge_organization.push(answer);
        self
    }

    /// Script the next `purge_project` answer.
    pub fn on_purge_project(&self, answer: Result<u64, TelmoniError>) -> &Self {
        self.purge_project.push(answer);
        self
    }

    /// Script the next `redact_person` answer.
    pub fn on_redact_person(&self, answer: Result<u64, TelmoniError>) -> &Self {
        self.redact_person.push(answer);
        self
    }

    /// Fail the next `n` organization purges.
    pub fn failing_purges(&self, n: usize) -> &Self {
        for _ in 0..n {
            self.on_purge_organization(Err(sibling_down("notifications")));
        }
        self
    }

    /// Fail the next `n` redactions.
    pub fn failing_redactions(&self, n: usize) -> &Self {
        for _ in 0..n {
            self.on_redact_person(Err(sibling_down("notifications")));
        }
        self
    }

    /// Hold every purge and redaction for `delay` before answering.
    pub fn delaying(&self, delay: Duration) -> &Self {
        *self
            .delay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(delay);
        self
    }

    /// Run `observe` on every call, before it is answered.
    pub fn observing(&self, observe: impl Fn(&SiblingCall) + Send + Sync + 'static) -> &Self {
        *self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(observe));
        self
    }

    async fn record(&self, call: SiblingCall) {
        if let Some(observe) = self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            observe(&call);
        }
        self.calls.record(call);
        let delay = *self
            .delay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
    }
}

#[async_trait]
impl Notifications for RecordingNotifications {
    async fn emit(
        &self,
        organization_id: &OrganizationId,
        project_id: Option<&ProjectId>,
        notice: Notice<'_>,
    ) -> Result<Emitted, TelmoniError> {
        self.record(SiblingCall::Emit {
            organization_id: organization_id.to_string(),
            project_id: project_id.map(ToString::to_string),
            kind: notice.kind,
            subject_user_id: notice.subject_user_id.map(ToString::to_string),
            title: notice.title.to_owned(),
        })
        .await;
        Ok(Emitted {
            feed_id: uuid::Uuid::now_v7(),
            deduplicated: false,
        })
    }

    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError> {
        self.record(SiblingCall::PurgeOrganization(organization_id.to_string()))
            .await;
        self.purge_organization
            .take()
            .unwrap_or_else(|| self.default_answer())
    }

    async fn purge_project(&self, project_id: &ProjectId) -> Result<u64, TelmoniError> {
        self.record(SiblingCall::PurgeProject(project_id.to_string()))
            .await;
        self.purge_project
            .take()
            .unwrap_or_else(|| self.default_answer())
    }

    async fn redact_person(&self, user_id: &UserId) -> Result<u64, TelmoniError> {
        self.record(SiblingCall::RedactPerson(user_id.to_string()))
            .await;
        self.redact_person
            .take()
            .unwrap_or_else(|| self.default_answer())
    }
}

/// Telemetry as auth holds it, recording every call. Unscripted, every purge
/// answers `Ok(1)` and every move lands; a suite scripts the failures it
/// wants and a move held past its budget.
pub struct RecordingTelemetry {
    calls: SiblingCalls,
    purge_organization: Answers<u64>,
    purge_project: Answers<u64>,
    move_project: Answers<()>,
    move_delays: Mutex<VecDeque<Duration>>,
    /// A pool to ask from, and the organization whose chain lock to ask about
    /// at each move.
    chain_probe: Mutex<Option<(PgPool, OrganizationId)>>,
    /// For each move since probing began, whether that lock was held.
    chain_held: Mutex<Vec<bool>>,
}

impl RecordingTelemetry {
    /// A recorder writing its calls to `calls`.
    #[must_use]
    pub fn new(calls: SiblingCalls) -> Self {
        Self {
            calls,
            purge_organization: Answers::default(),
            purge_project: Answers::default(),
            move_project: Answers::default(),
            move_delays: Mutex::new(VecDeque::new()),
            chain_probe: Mutex::new(None),
            chain_held: Mutex::new(Vec::new()),
        }
    }

    /// At each move from now on, ask from a session of `pool`'s whether
    /// `organization`'s audit chain lock is held: the lock telemetry's
    /// content switch takes to learn where a project went, which an accept
    /// must hold while it moves the project's settings.
    pub fn probing_chain_lock(&self, pool: PgPool, organization: OrganizationId) -> &Self {
        *self
            .chain_probe
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((pool, organization));
        self
    }

    /// Whether the probed chain lock was held at each move, in order.
    #[must_use]
    pub fn chain_held_at_moves(&self) -> Vec<bool> {
        self.chain_held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Fail the next `n` organization purges.
    pub fn failing_purges(&self, n: usize) -> &Self {
        for _ in 0..n {
            self.purge_organization.push(Err(sibling_down("telemetry")));
        }
        self
    }

    /// Fail the next `n` project purges.
    pub fn failing_project_purges(&self, n: usize) -> &Self {
        for _ in 0..n {
            self.purge_project.push(Err(sibling_down("telemetry")));
        }
        self
    }

    /// Fail the next `n` moves.
    pub fn failing_moves(&self, n: usize) -> &Self {
        for _ in 0..n {
            self.move_project.push(Err(sibling_down("telemetry")));
        }
        self
    }

    /// Hold the next move for `delay` before answering: one over the
    /// caller's budget is given up on, as a slow database's would be.
    pub fn delaying_next_move(&self, delay: Duration) -> &Self {
        self.move_delays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(delay);
        self
    }
}

#[async_trait]
impl Telemetry for RecordingTelemetry {
    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError> {
        self.calls.record(SiblingCall::TelemetryPurgeOrganization(
            organization_id.to_string(),
        ));
        self.purge_organization.take().unwrap_or(Ok(1))
    }

    async fn purge_project(&self, project_id: &ProjectId) -> Result<u64, TelmoniError> {
        self.calls
            .record(SiblingCall::TelemetryPurgeProject(project_id.to_string()));
        self.purge_project.take().unwrap_or(Ok(1))
    }

    async fn move_project(
        &self,
        project_id: &ProjectId,
        organization_id: &OrganizationId,
    ) -> Result<(), TelmoniError> {
        self.calls.record(SiblingCall::TelemetryMove {
            project_id: project_id.to_string(),
            organization_id: organization_id.to_string(),
        });
        let probe = self
            .chain_probe
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some((pool, organization)) = probe {
            let held = telmoni_shared::test_util::chain_lock_held(&pool, &organization).await;
            self.chain_held
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(held);
        }
        let delay = self
            .move_delays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        self.move_project.take().unwrap_or(Ok(()))
    }
}

/// The purge hook as auth holds it, recording every call. Unscripted, every
/// purge lands.
pub struct RecordingHook {
    calls: SiblingCalls,
    purge: Answers<()>,
    down: std::sync::atomic::AtomicBool,
    delay: Mutex<Option<Duration>>,
}

impl RecordingHook {
    /// A recorder writing its calls to `calls`.
    #[must_use]
    pub fn new(calls: SiblingCalls) -> Self {
        Self {
            calls,
            purge: Answers::default(),
            down: std::sync::atomic::AtomicBool::new(false),
            delay: Mutex::new(None),
        }
    }

    /// Fail every purge from now on, past whatever is scripted.
    pub fn down(&self) -> &Self {
        self.down.store(true, std::sync::atomic::Ordering::SeqCst);
        self
    }

    /// Script the next purge's answer.
    pub fn on_purge(&self, answer: Result<(), TelmoniError>) -> &Self {
        self.purge.push(answer);
        self
    }

    /// Fail the next `n` purges.
    pub fn failing(&self, n: usize) -> &Self {
        for _ in 0..n {
            self.on_purge(Err(sibling_down("the purge hook")));
        }
        self
    }

    /// Hold every purge for `delay` before answering.
    pub fn delaying(&self, delay: Duration) -> &Self {
        *self
            .delay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(delay);
        self
    }
}

#[async_trait]
impl PurgeHook for RecordingHook {
    async fn purge_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<(), TelmoniError> {
        self.calls
            .record(SiblingCall::HookPurge(organization_id.to_string()));
        let delay = *self
            .delay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        self.purge.take().unwrap_or_else(|| {
            if self.down.load(std::sync::atomic::Ordering::SeqCst) {
                Err(sibling_down("the purge hook"))
            } else {
                Ok(())
            }
        })
    }
}

/// The console's origin every test issuer names.
pub const TEST_APP_URL: &str = "http://localhost:3000";

/// The subject the scripted provider names unless told otherwise.
pub const TEST_SUBJECT: &str = "user_01TEST";

/// An issuer over `db`, with the address verification on. What a suite
/// hands `AppState::issuer`.
#[must_use]
pub fn test_issuer(db: PgPool) -> Arc<Issuer> {
    test_issuer_verifying(db, true)
}

/// [`test_issuer`] with `VERIFY_EMAIL` as given.
#[must_use]
pub fn test_issuer_verifying(db: PgPool, verify_email: bool) -> Arc<Issuer> {
    Arc::new(Issuer::new(db, TEST_APP_URL, verify_email))
}

/// A live bearer for `user_id` in session `sid`: an access-token row written
/// as the issuer writes one, so a suite's request is resolved by the same
/// lookup a real one is. `pool` is the suite's own, the owner's. The
/// person's identity must be recorded first (`seed_identity`): a bearer names
/// only somebody auth knows.
#[expect(
    clippy::panic,
    reason = "test support: a bearer that cannot be minted is a failing suite"
)]
pub async fn bearer_in(pool: &PgPool, user_id: &str, sid: &str) -> String {
    let user =
        UserId::try_new(user_id).unwrap_or_else(|e| panic!("{user_id:?} is not a user id: {e:?}"));
    let token = secret();
    let mut tx = person_scope(pool, &user)
        .await
        .unwrap_or_else(|e| panic!("a scope for {user_id}: {e}"));
    access_tokens::create(
        &mut tx,
        &user,
        sid,
        &hash(&token),
        chrono::Utc::now() + chrono::Duration::seconds(BEARER_TTL_SECS),
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "a bearer for {user_id}: {e} — seed_identity first: a bearer names only somebody \
             auth knows"
        )
    });
    tx.commit()
        .await
        .unwrap_or_else(|e| panic!("a bearer for {user_id}: {e}"));
    token
}

/// [`bearer_in`], in a session of its own that nothing has recorded, so no
/// sign-out or revoke of another session reaches it.
pub async fn bearer(pool: &PgPool, user_id: &str) -> String {
    bearer_in(
        pool,
        user_id,
        &format!("ses_test_{}", uuid::Uuid::new_v4().simple()),
    )
    .await
}

/// Put a fresh [`bearer`] for `user_id` on a request builder — the one line
/// every person request in a suite needs.
pub async fn as_person(
    builder: axum::http::request::Builder,
    pool: &PgPool,
    user_id: &str,
) -> axum::http::request::Builder {
    builder.header(
        "authorization",
        format!("Bearer {}", bearer(pool, user_id).await),
    )
}

/// The external provider slot for a suite: `provider`, under the policy
/// every suite assumes unless it says otherwise — sign-ups through it open,
/// no linking by address.
#[must_use]
pub fn external(provider: Arc<dyn AuthProvider>) -> crate::external::External {
    crate::external::External {
        provider,
        allow_sign_up: true,
        link_by_email: false,
    }
}

/// One call the suite's router made on the provider, with what it sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    /// `authorize_url`.
    AuthorizeUrl {
        /// The callback the flow returns to.
        redirect_uri: String,
        /// The CSRF state.
        state: String,
        /// Whether the sign-up screen was asked for.
        sign_up: bool,
        /// The address to pre-fill, when one was collected.
        login_hint: Option<String>,
    },
    /// `exchange_code`.
    Exchange {
        /// The authorization code.
        code: String,
    },
    /// `logout_url`.
    LogoutUrl {
        /// The id token the browser's sign-out names, when the session had one.
        id_token: Option<String>,
        /// Where the browser comes back to.
        return_to: String,
    },
    /// `delete_user`.
    DeleteUser {
        /// The provider's subject erased.
        subject: String,
    },
    /// `create_password_reset`.
    PasswordReset {
        /// The address the link is minted for.
        email: String,
    },
    /// `send_email_change`.
    SendEmailChange {
        /// The provider's subject changing their address.
        subject: String,
        /// The address the provider mails its code to.
        new_email: String,
    },
    /// `confirm_email_change`.
    ConfirmEmailChange {
        /// The provider's subject changing their address.
        subject: String,
        /// The code the provider mailed.
        code: String,
    },
}

/// What the next call of each kind answers. Empty means the default.
#[derive(Default)]
struct Script {
    exchange: VecDeque<Result<Authenticated, TelmoniError>>,
    delete_user: VecDeque<Result<(), TelmoniError>>,
    password_reset: VecDeque<Result<PasswordResetLink, TelmoniError>>,
    send_email_change: VecDeque<Result<EmailChangeChallenge, TelmoniError>>,
    confirm_email_change: VecDeque<Result<ConfirmedEmail, TelmoniError>>,
}

/// The scripted provider. Build one with [`ScriptedProvider::new`], script
/// the answers a test needs, wire it into the router, and read
/// [`ScriptedProvider::calls`] afterwards.
pub struct ScriptedProvider {
    /// The origin the URLs this provider builds start with, and its id.
    pub base: String,
    calls: Mutex<Vec<Call>>,
    script: Mutex<Script>,
}

impl Default for ScriptedProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// An exchange's default answer: [`TEST_SUBJECT`], verified, named Ada
/// Lovelace, with an id token for the sign-out.
#[must_use]
pub fn authenticated(sub: &str) -> Authenticated {
    Authenticated {
        subject: Subject {
            sub: sub.to_owned(),
            email: Some("ada@example.com".into()),
            email_verified: true,
            given_name: Some("Ada".into()),
            family_name: Some("Lovelace".into()),
        },
        id_token: Some(format!("id_token_for_{sub}")),
        auth_method: None,
    }
}

impl ScriptedProvider {
    /// A provider at `https://idp.telmoni.invalid` with nothing scripted.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: telmoni_shared::test_util::id_token::TEST_PROVIDER_ISSUER.to_owned(),
            calls: Mutex::new(Vec::new()),
            script: Mutex::new(Script::default()),
        }
    }

    fn record(&self, call: Call) {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
    }

    fn script<R>(&self, f: impl FnOnce(&mut Script) -> R) -> R {
        f(&mut self
            .script
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Every call made so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// How many recorded calls `pred` admits.
    pub fn count(&self, pred: impl Fn(&Call) -> bool) -> usize {
        self.calls().iter().filter(|c| pred(c)).count()
    }

    /// Script the next `exchange_code` answer.
    pub fn on_exchange(&self, answer: Result<Authenticated, TelmoniError>) -> &Self {
        self.script(|s| s.exchange.push_back(answer));
        self
    }

    /// Script the next `delete_user` answer.
    pub fn on_delete_user(&self, answer: Result<(), TelmoniError>) -> &Self {
        self.script(|s| s.delete_user.push_back(answer));
        self
    }

    /// Script the next `create_password_reset` answer.
    pub fn on_password_reset(&self, answer: Result<PasswordResetLink, TelmoniError>) -> &Self {
        self.script(|s| s.password_reset.push_back(answer));
        self
    }

    /// Script the next `send_email_change` answer.
    pub fn on_send_email_change(
        &self,
        answer: Result<EmailChangeChallenge, TelmoniError>,
    ) -> &Self {
        self.script(|s| s.send_email_change.push_back(answer));
        self
    }

    /// Script the next `confirm_email_change` answer.
    pub fn on_confirm_email_change(&self, answer: Result<ConfirmedEmail, TelmoniError>) -> &Self {
        self.script(|s| s.confirm_email_change.push_back(answer));
        self
    }
}

#[async_trait]
impl AuthProvider for ScriptedProvider {
    fn id(&self) -> &str {
        &self.base
    }

    fn name(&self) -> &str {
        "the test provider"
    }

    async fn authorize_url(
        &self,
        redirect_uri: &str,
        state: &str,
        sign_up: bool,
        login_hint: Option<&str>,
    ) -> Result<String, TelmoniError> {
        self.record(Call::AuthorizeUrl {
            redirect_uri: redirect_uri.to_owned(),
            state: state.to_owned(),
            sign_up,
            login_hint: login_hint.map(str::to_owned),
        });
        let mut url = format!(
            "{}/authorize?redirect_uri={redirect_uri}&state={state}",
            self.base
        );
        if sign_up {
            url.push_str("&prompt=create");
        }
        if let Some(hint) = login_hint {
            url.push_str("&login_hint=");
            url.push_str(hint);
        }
        Ok(url)
    }

    async fn exchange_code(
        &self,
        code: &str,
        _redirect_uri: &str,
    ) -> Result<Authenticated, TelmoniError> {
        self.record(Call::Exchange {
            code: code.to_owned(),
        });
        self.script(|s| s.exchange.pop_front())
            .unwrap_or_else(|| Ok(authenticated(TEST_SUBJECT)))
    }

    async fn logout_url(&self, id_token: Option<&str>, return_to: &str) -> String {
        self.record(Call::LogoutUrl {
            id_token: id_token.map(str::to_owned),
            return_to: return_to.to_owned(),
        });
        format!(
            "{}/logout?id_token_hint={}&return_to={return_to}",
            self.base,
            id_token.unwrap_or("")
        )
    }

    async fn delete_user(&self, subject: &str) -> Result<(), TelmoniError> {
        self.record(Call::DeleteUser {
            subject: subject.to_owned(),
        });
        self.script(|s| s.delete_user.pop_front()).unwrap_or(Ok(()))
    }

    async fn create_password_reset(&self, email: &str) -> Result<PasswordResetLink, TelmoniError> {
        self.record(Call::PasswordReset {
            email: email.to_owned(),
        });
        self.script(|s| s.password_reset.pop_front())
            .unwrap_or_else(|| {
                Ok(PasswordResetLink {
                    url: format!("{}/password-reset/tok_test", self.base),
                    expires_at: "2026-12-31T00:00:00.000Z".into(),
                })
            })
    }

    async fn send_email_change(
        &self,
        subject: &str,
        new_email: &str,
    ) -> Result<EmailChangeChallenge, TelmoniError> {
        self.record(Call::SendEmailChange {
            subject: subject.to_owned(),
            new_email: new_email.to_owned(),
        });
        self.script(|s| s.send_email_change.pop_front())
            .unwrap_or_else(|| {
                Ok(EmailChangeChallenge {
                    new_email: new_email.to_owned(),
                    expires_at: "2026-12-31T00:00:00.000Z".into(),
                })
            })
    }

    async fn confirm_email_change(
        &self,
        subject: &str,
        code: &str,
    ) -> Result<ConfirmedEmail, TelmoniError> {
        self.record(Call::ConfirmEmailChange {
            subject: subject.to_owned(),
            code: code.to_owned(),
        });
        self.script(|s| s.confirm_email_change.pop_front())
            .unwrap_or_else(|| {
                // The last address a send opened a change for, which is what
                // a real provider confirms; nothing else is a sensible default.
                let pending = self.calls().into_iter().rev().find_map(|c| match c {
                    Call::SendEmailChange { new_email, .. } => Some(new_email),
                    _ => None,
                });
                match pending {
                    Some(email) => Ok(ConfirmedEmail {
                        email,
                        email_verified: true,
                    }),
                    None => Err(telmoni_shared::AuthError::BadRequest(
                        "invalid or expired confirmation code".into(),
                    )
                    .into()),
                }
            })
    }
}
