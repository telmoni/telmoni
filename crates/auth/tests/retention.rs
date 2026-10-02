//! The retention sweep: API tokens and unaccepted invitations past their
//! grace windows.
#![expect(clippy::unwrap_used, reason = "test scaffolding")]

use std::sync::Arc;

use sqlx::PgPool;

use telmoni_auth::test_provider::ScriptedProvider;
use telmoni_auth::{AppState, Config};
use telmoni_shared::test_util::service_pool;

const SERVICE_SECRET: &str = "test-service-secret";
const ORGANIZATION: &str = "org_reap";
const PROJECT: &str = "project_reap";

fn state(pool: PgPool) -> Arc<AppState> {
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
    let db = service_pool(&pool, "auth");
    Arc::new(AppState {
        issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
        password: None,
        external: Some(telmoni_auth::test_provider::external(Arc::new(
            ScriptedProvider::new(),
        ))),
        db,
        config,
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(Arc::new(
            telmoni_shared::mail::NoopSender,
        ))),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
        siblings: telmoni_auth::Siblings::default(),
    })
}

/// Four tokens: two past the seven-day grace, one revoked inside it, one live.
async fn seed(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO auth.organizations (external_id, slug) VALUES ($1, 'org-' || md5($1))",
    )
    .bind(ORGANIZATION)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO auth.projects (external_id, organization_id, name, slug)
         VALUES ($1, $2, 'Default project', 'default-project')",
    )
    .bind(PROJECT)
    .bind(ORGANIZATION)
    .execute(pool)
    .await
    .unwrap();

    for (name, hash, revoked, expires) in [
        ("revoked-long-ago", "h-revoked", Some("8 days"), None),
        ("expired-long-ago", "h-expired", None, Some("8 days")),
        ("revoked-yesterday", "h-recent", Some("1 day"), None),
        ("live", "h-live", None, None),
    ] {
        sqlx::query(
            "INSERT INTO auth.api_tokens (organization_id, project_id, name, token_hash,
                                          created_by, revoked_at, expires_at)
             VALUES ($1, $2, $3, $4, 'u_seed',
                     CASE WHEN $5::text IS NULL THEN NULL
                          ELSE now() - $5::interval END,
                     CASE WHEN $6::text IS NULL THEN NULL
                          ELSE now() - $6::interval END)",
        )
        .bind(ORGANIZATION)
        .bind(PROJECT)
        .bind(name)
        .bind(hash)
        .bind(revoked)
        .bind(expires)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Five invitations in each table, one per state: expired long ago, withdrawn
/// long ago, expired long ago but accepted, expired the other day, and live.
/// The first two are stale; the sweep removes them and nothing else.
async fn seed_invitations(pool: &PgPool) {
    for (email, expires, revoked, accepted) in [
        ("expired-long-ago", "-31 days", None, false),
        ("withdrawn-long-ago", "7 days", Some("-31 days"), false),
        ("accepted-long-ago", "-31 days", None, true),
        ("expired-the-other-day", "-2 days", None, false),
        ("live", "7 days", None, false),
    ] {
        for table in ["member_invites", "organization_invites"] {
            let (owner, owner_id, role) = if table == "member_invites" {
                ("project_id", PROJECT, "admin")
            } else {
                ("organization_id", ORGANIZATION, "member")
            };
            sqlx::query(&format!(
                "INSERT INTO auth.{table}
                     (id, {owner}, email, role, token_hash, invited_by, expires_at,
                      revoked_at, accepted_at, accepted_by)
                 VALUES (gen_random_uuid(), $1, $2, $3, gen_random_uuid()::text, 'u_seed',
                         now() + $4::interval,
                         now() + $5::interval,
                         CASE WHEN $6 THEN now() - interval '40 days' END,
                         CASE WHEN $6 THEN 'u_acceptor' END)"
            ))
            .bind(owner_id)
            .bind(format!("{email}@example.test"))
            .bind(role)
            .bind(expires)
            .bind(revoked)
            .bind(accepted)
            .execute(pool)
            .await
            .unwrap();
        }
    }
}

/// The addresses still standing, in both tables, sorted.
async fn invitation_addresses(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT email FROM auth.member_invites
          UNION ALL
         SELECT email FROM auth.organization_invites
          ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn live_token_names(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar::<_, String>("SELECT name FROM auth.api_tokens ORDER BY name")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn the_sweep_removes_only_the_rows_past_their_grace_window(pool: PgPool) {
    seed(&pool).await;
    seed_invitations(&pool).await;
    assert_eq!(live_token_names(&pool).await.len(), 4, "seeded four");
    assert_eq!(invitation_addresses(&pool).await.len(), 10, "seeded ten");

    let swept = telmoni_auth::sweep::retention(&state(pool.clone()))
        .await
        .unwrap();

    assert_eq!(
        swept.tokens, 2,
        "the revoked-long-ago and expired-long-ago rows go, and nothing else"
    );
    assert_eq!(
        live_token_names(&pool).await,
        vec!["live".to_string(), "revoked-yesterday".to_string()],
        "the live token and the one still inside its grace window survive"
    );

    assert_eq!(
        swept.invitations, 4,
        "the expired and the withdrawn invitation, in each table, and nothing else"
    );
    let survivors = invitation_addresses(&pool).await;
    assert_eq!(survivors.len(), 6, "{survivors:?}");
    for stale in ["expired-long-ago", "withdrawn-long-ago"] {
        assert!(
            !survivors.iter().any(|e| e.starts_with(stale)),
            "{stale} survived in one table: {survivors:?}"
        );
    }
    for kept in ["accepted-long-ago", "expired-the-other-day", "live"] {
        assert_eq!(
            survivors.iter().filter(|e| e.starts_with(kept)).count(),
            2,
            "{kept} is kept in both tables: {survivors:?}"
        );
    }
    assert_eq!(swept.sessions, 0, "no session was seeded");
    assert_eq!(swept.grants, 0, "no grant was seeded");
}

/// A second run finds nothing: the counts answer for what this run removed.
#[sqlx::test(migrations = "./migrations")]
async fn the_sweep_is_idempotent(pool: PgPool) {
    seed(&pool).await;
    let state = state(pool.clone());
    let first = telmoni_auth::sweep::retention(&state).await.unwrap();
    assert_eq!(first.tokens, 2);
    let second = telmoni_auth::sweep::retention(&state).await.unwrap();
    assert_eq!(
        second.tokens, 0,
        "the second run reported rows the first removed"
    );
    assert_eq!(live_token_names(&pool).await.len(), 2);
}
