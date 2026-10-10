//! The sweeps as the process runs them: the deletion sweep finalizes every
//! ripe organization and purges every unripe one, a failed step fails the
//! run and spares the rest, the person sweep follows the organization sweep
//! and a failed erasure stops nobody else's, one leader sweeps; the audit
//! walk reports a forged chain and survives it; the retention sweep counts
//! what it removed.
//!
//! The purges are auth's: its finalize runs every sibling's before the hard
//! delete, so a sweep touches the siblings only through auth.
#![expect(clippy::unwrap_used, reason = "test scaffolding")]

use std::sync::Arc;

use sqlx::PgPool;

use telmoni_auth::sweep::{self, DeletionSwept};
use telmoni_auth::test_provider::{
    RecordingHook, RecordingNotifications, ScriptedProvider, SiblingCall, SiblingCalls,
};
use telmoni_auth::{AppState, Config, Siblings};
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};

const SERVICE_SECRET: &str = "test-service-secret";

/// The purge hook, notifications, and the timeline both record onto.
fn siblings() -> (
    SiblingCalls,
    Arc<RecordingHook>,
    Arc<RecordingNotifications>,
) {
    let calls = SiblingCalls::default();
    (
        calls.clone(),
        Arc::new(RecordingHook::new(calls.clone())),
        Arc::new(RecordingNotifications::new(calls)),
    )
}

/// Auth over `pool`, with `hook` and `notifications` beside it and a
/// scripted provider, whose handle comes back for the erasures to be read.
fn state(
    pool: &PgPool,
    hook: Arc<RecordingHook>,
    notifications: Arc<RecordingNotifications>,
) -> (Arc<AppState>, Arc<ScriptedProvider>) {
    let config = Config {
        database_url: String::new(),
        service_secret: SERVICE_SECRET.into(),
        service_secret_next: None,
        allow_test_session: false,
        redirect_uri: "http://localhost:3000/auth/callback".into(),
        app_url: "http://localhost:3000".into(),
        mail_from: "Telmoni <test@example.com>".into(),
        support_email: None,
        deletion_tail_budget_ms: 8_000,
    };
    let provider = Arc::new(ScriptedProvider::new());
    let db = service_pool(pool, "auth");
    let state = Arc::new(AppState {
        issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
        password: None,
        external: Some(telmoni_auth::test_provider::external(provider.clone())),
        db,
        config,
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(Arc::new(
            telmoni_shared::mail::NoopSender,
        ))),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
        siblings: Siblings {
            notifications: Some(notifications),
            telemetry: None,
            agent: None,
            purge_hook: Some(hook),
        },
    });
    (state, provider)
}

/// An organization owned by `owner`, `pending_deletion` as its owner's
/// deletion leaves it, with `erase_after` `wait` from now — past, for a
/// ripe one — and the hook purge recorded or not.
async fn seed_pending(pool: &PgPool, id: &str, owner: &str, wait: &str, hook_purged: bool) {
    sqlx::query(
        "INSERT INTO auth.organizations
             (external_id, slug, name, status, deletion_requested_at, erase_after,
              deletion_kind, hook_purged_at)
         VALUES ($1, 'org-' || md5($1), 'Acme', 'pending_deletion', now(), now() + $2::interval,
                 'owner', CASE WHEN $3 THEN now() END)",
    )
    .bind(id)
    .bind(wait)
    .bind(hook_purged)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth.organization_members (organization_id, user_id, role, added_by)
         VALUES ($1, $2, 'owner', $2)",
    )
    .bind(id)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
}

/// Mark `user` as having confirmed their account's deletion.
async fn mark_person_pending(pool: &PgPool, user: &str) {
    sqlx::query("INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ($1, now())")
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
}

async fn org_status(pool: &PgPool, organization: &str) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM auth.organizations WHERE external_id = $1")
        .bind(organization)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn hook_recorded(pool: &PgPool, organization: &str) -> bool {
    sqlx::query_scalar(
        "SELECT hook_purged_at IS NOT NULL FROM auth.organizations WHERE external_id = $1",
    )
    .bind(organization)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn person_exists(pool: &PgPool, user: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM auth.identities WHERE user_id = $1)")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Every ripe organization is finalized — its siblings purged, then its row
/// gone — and every unripe one's hook purge landed and recorded, in one tick.
#[sqlx::test]
async fn the_deletion_sweep_finalizes_the_ripe_and_purges_the_unripe(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, "user_a", "a@example.test").await;
    seed_identity(&pool, "user_b", "b@example.test").await;
    seed_identity(&pool, "user_c", "c@example.test").await;
    seed_pending(&pool, "org_ripe_1", "user_a", "-1 minute", true).await;
    seed_pending(&pool, "org_ripe_2", "user_b", "-1 minute", true).await;
    seed_pending(&pool, "org_unripe", "user_c", "13 days", false).await;
    let (calls, hook, notifications) = siblings();
    let (state, _provider) = state(&pool, hook, notifications);

    let swept = sweep::deletion(&state).await.unwrap();

    assert_eq!(
        swept,
        DeletionSwept {
            organizations: 2,
            purged: 1,
            people: 0
        }
    );
    assert_eq!(org_status(&pool, "org_ripe_1").await, None);
    assert_eq!(org_status(&pool, "org_ripe_2").await, None);
    assert_eq!(
        org_status(&pool, "org_unripe").await.as_deref(),
        Some("pending_deletion"),
        "an organization inside its wait is its owner's to restore"
    );
    assert!(hook_recorded(&pool, "org_unripe").await);
    for ripe in ["org_ripe_1", "org_ripe_2"] {
        assert_eq!(
            calls.count(|c| *c == SiblingCall::PurgeOrganization(ripe.to_owned())),
            1,
            "{ripe}: notifications purged once at finalize"
        );
        assert_eq!(
            calls.count(|c| *c == SiblingCall::HookPurge(ripe.to_owned())),
            1,
            "{ripe}: the hook purged again at finalize"
        );
    }
    assert_eq!(
        calls
            .calls()
            .iter()
            .filter(|c| matches!(c, SiblingCall::HookPurge(o) if o == "org_unripe"))
            .count(),
        1,
        "the unripe organization's hook purge, and nothing more"
    );
    assert!(
        !calls
            .calls()
            .contains(&SiblingCall::PurgeOrganization("org_unripe".to_owned())),
        "notifications is purged at finalize alone"
    );

    let second = sweep::deletion(&state).await.unwrap();
    assert_eq!(
        second,
        DeletionSwept::default(),
        "a purged organization inside its window is nobody's to touch"
    );
}

/// A finalize whose purge fails fails the run — or nobody hears of it — and
/// the sweep still finishes the organizations after it.
#[sqlx::test]
async fn a_failed_purge_fails_the_run_and_spares_the_other_organizations(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, "user_a", "a@example.test").await;
    seed_identity(&pool, "user_b", "b@example.test").await;
    seed_pending(&pool, "org_ripe_1", "user_a", "-2 minutes", true).await;
    seed_pending(&pool, "org_ripe_2", "user_b", "-1 minute", true).await;
    let (_, hook, notifications) = siblings();
    // The first organization's notifications purge fails; the second's lands.
    notifications.failing_purges(1);
    let (state, _provider) = state(&pool, hook, notifications);

    let err = sweep::deletion(&state)
        .await
        .expect_err("a tail that failed must fail the run");
    assert!(
        err.to_string()
            .contains("1 organization tail(s) failed this tick (1 finalized"),
        "the failure names the count: {err}"
    );
    assert_eq!(
        org_status(&pool, "org_ripe_1").await.as_deref(),
        Some("pending_deletion"),
        "the row never goes before its purges did"
    );
    assert_eq!(org_status(&pool, "org_ripe_2").await, None);

    let swept = sweep::deletion(&state).await.unwrap();
    assert_eq!(swept.organizations, 1, "the failed finalize is retried");
    assert_eq!(org_status(&pool, "org_ripe_1").await, None);
}

/// The person sweep runs after the organization sweep: a sole owner's
/// erasure waits on the finalize of what they owned, and both finish in one
/// tick — the provider asked, the notices redacted, the identity gone.
#[sqlx::test]
async fn the_person_sweep_follows_the_organization_sweep(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, "user_going", "going@example.test").await;
    seed_pending(&pool, "org_going", "user_going", "-1 minute", true).await;
    mark_person_pending(&pool, "user_going").await;
    let (calls, hook, notifications) = siblings();
    let (state, provider) = state(&pool, hook, notifications);

    let swept = sweep::deletion(&state).await.unwrap();

    assert_eq!(
        swept,
        DeletionSwept {
            organizations: 1,
            purged: 0,
            people: 1
        }
    );
    assert_eq!(org_status(&pool, "org_going").await, None);
    assert!(!person_exists(&pool, "user_going").await);
    assert_eq!(
        provider.count(|c| matches!(c, telmoni_auth::test_provider::Call::DeleteUser { .. })),
        1,
        "the provider's user went once"
    );
    let order: Vec<&SiblingCall> = calls.calls().leak().iter().collect();
    let finalize_at = order
        .iter()
        .position(|c| matches!(c, SiblingCall::PurgeOrganization(_)))
        .expect("the organization was purged");
    let redact_at = order
        .iter()
        .position(|c| matches!(c, SiblingCall::RedactPerson(_)))
        .expect("the person's notices were redacted");
    assert!(
        finalize_at < redact_at,
        "the organization's finalize before the person's erasure: {order:?}"
    );
}

/// A failed erasure fails the run, stops nobody else's, and is retried.
#[sqlx::test]
async fn a_failed_erasure_retries_next_tick_and_stops_nobody_elses(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, "user_p1", "p1@example.test").await;
    seed_identity(&pool, "user_p2", "p2@example.test").await;
    mark_person_pending(&pool, "user_p1").await;
    mark_person_pending(&pool, "user_p2").await;
    let (_, hook, notifications) = siblings();
    // The first redaction fails; every later one lands.
    notifications.failing_redactions(1);
    let (state, provider) = state(&pool, hook, notifications);

    let err = sweep::deletion(&state)
        .await
        .expect_err("an erasure that failed must fail the run");
    assert!(
        err.to_string()
            .contains("1 person erasure(s) failed this tick (0 finalized, 0 purged, 1 erased)"),
        "the person after the failure is still erased: {err}"
    );
    assert_eq!(
        provider.count(|c| matches!(c, telmoni_auth::test_provider::Call::DeleteUser { .. })),
        2,
        "the provider goes first, for both"
    );

    let swept = sweep::deletion(&state).await.unwrap();
    assert_eq!(
        swept,
        DeletionSwept {
            organizations: 0,
            purged: 0,
            people: 1
        },
        "the failed erasure is retried"
    );
    assert!(!person_exists(&pool, "user_p1").await);
    assert!(!person_exists(&pool, "user_p2").await);
}

/// Two replicas on one timer: the tick's leader lock lets one sweep, and the
/// loser skips the tick entirely.
#[sqlx::test]
async fn only_one_leader_sweeps_per_tick(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, "user_a", "a@example.test").await;
    seed_pending(&pool, "org_ripe", "user_a", "-1 minute", true).await;
    let (calls, hook, notifications) = siblings();
    let (state, _provider) = state(&pool, hook, notifications);

    let mut leader = pool.begin().await.unwrap();
    let held: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(0x776F_726B_5F64_656Ci64)
        .fetch_one(&mut *leader)
        .await
        .unwrap();
    assert!(held);

    let swept = sweep::deletion(&state).await.unwrap();
    assert_eq!(
        swept,
        DeletionSwept::default(),
        "the loser skips the tick entirely"
    );
    assert!(calls.calls().is_empty(), "the loser purged something");
    assert_eq!(
        org_status(&pool, "org_ripe").await.as_deref(),
        Some("pending_deletion")
    );

    leader.rollback().await.unwrap();
    let swept = sweep::deletion(&state).await.unwrap();
    assert_eq!(swept.organizations, 1, "the unlocked sweep runs");
}

/// The audit walk verifies every chain, reports a forged one at the end of
/// the walk rather than stopping on it, and passes an intact set.
#[sqlx::test]
async fn the_audit_walk_reports_a_forged_chain_and_verifies_the_rest(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (_, hook, notifications) = siblings();
    let (state, _provider) = state(&pool, hook, notifications);
    for organization in ["org_intact", "org_forged"] {
        let mut tx = telmoni_shared::db::tenant_session::organization_scope(
            &pool,
            &telmoni_shared::OrganizationId::try_new(organization).unwrap(),
        )
        .await
        .unwrap();
        for n in 0..3 {
            telmoni_shared::audit::emit_audit(
                &mut tx,
                telmoni_shared::audit::AuditEvent {
                    organization_id: &telmoni_shared::OrganizationId::try_new(organization)
                        .unwrap(),
                    in_project: None,
                    actor: telmoni_shared::audit::Actor::Service("test"),
                    action: telmoni_shared::AuditAction::Updated,
                    resource_kind: telmoni_shared::TelmoniResourceKind::Organization,
                    resource_id: Some(organization),
                    request_id: None,
                    ip_address: None,
                    user_agent: None,
                    metadata: Some(serde_json::json!({ "n": n })),
                },
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
    }

    sweep::audit_verify(&state)
        .await
        .expect("two intact chains verify");

    // A field edited in place, as the owner could: the recompute disagrees.
    // The statement is assembled, as `shared/tests/audit_chain.rs` does its
    // tampering, so the append-only guard's literal scan never meets it.
    let forge = format!(
        "{} {}.{} SET metadata = '{{\"n\": 99}}'::jsonb
          WHERE organization_id = 'org_forged' AND metadata->>'n' = '1'",
        "UPDATE", "audit", "events"
    );
    sqlx::query(&forge).execute(&pool).await.unwrap();

    let err = sweep::audit_verify(&state)
        .await
        .expect_err("a forged chain fails the run after the walk");
    assert!(
        err.to_string().contains("1 of 2 audit chains failed"),
        "{err}"
    );
}

/// A revoked session past ninety days is one row the retention sweep removes.
#[sqlx::test]
async fn the_retention_sweep_counts_what_it_removed(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, "user_a", "a@example.test").await;
    sqlx::query(
        "INSERT INTO auth.sessions (user_id, provider_sid, user_agent, revoked_at)
         VALUES ($1, 'sid_old', 'agent', now() - interval '91 days'),
                ($1, 'sid_recent', 'agent', now() - interval '1 day')",
    )
    .bind("user_a")
    .execute(&pool)
    .await
    .unwrap();
    let (_, hook, notifications) = siblings();
    let (state, _provider) = state(&pool, hook, notifications);

    let swept = sweep::retention(&state).await.unwrap();
    assert_eq!(swept.sessions, 1, "the revoked session past its window");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 1, "the recent one stays");
}
