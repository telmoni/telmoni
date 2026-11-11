//! The codes auth mails a person to confirm what they asked: five of a
//! purpose a day and a wait between two, whichever lane asks, held here and
//! not by a console.
#![expect(clippy::unwrap_used, reason = "test scaffolding: fixture setup")]

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::db::confirmation_codes;
use telmoni_auth::test_provider::{ScriptedProvider, as_person};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::acting::Acting;
use telmoni_shared::mail::{Mail, MailError, MailSender};
use telmoni_shared::seam::{Auth as _, Confirmation};
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{
    OrganizationId, OrganizationRole, ProjectId, TelmoniError, TenantError, UserId,
};

const SERVICE_SECRET: &str = "test-service-secret";
const OWNER: &str = "user_codes_1";
const EMAIL: &str = "ada@example.com";
const ORGANIZATION: &str = "org_codes_1";
const PROJECT: &str = "project_codes_1";

/// Every mail handed to the transport.
#[derive(Default)]
struct Outbox(Mutex<Vec<Mail>>);

#[async_trait::async_trait]
impl MailSender for Outbox {
    async fn send(&self, mail: &Mail) -> Result<(), MailError> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

impl Outbox {
    fn sent(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

fn state(pool: &PgPool, outbox: Arc<Outbox>) -> Arc<AppState> {
    let db = service_pool(pool, ServiceRole::Auth);
    Arc::new(AppState {
        issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
        password: None,
        external: Some(telmoni_auth::test_provider::external(Arc::new(
            ScriptedProvider::new(),
        ))),
        db,
        config: Config {
            database_url: String::new(),
            service_secret: SERVICE_SECRET.into(),
            service_secret_next: None,
            allow_test_session: false,
            redirect_uri: "http://localhost:3000/auth/callback".into(),
            app_url: "http://localhost:3000".into(),
            mail_from: "Telmoni <test@example.com>".into(),
            support_email: None,
            deletion_tail_budget_ms: 8_000,
        },
        siblings: telmoni_auth::Siblings::default(),
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(outbox)),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
    })
}

/// The owner, and the organization and project the code names.
async fn seed(pool: &PgPool) {
    apply_audit_migrations(pool).await;
    seed_identity(pool, OWNER, EMAIL).await;
    sqlx::query(
        "INSERT INTO auth.organizations (id, external_id, slug, name, shard_key)
         VALUES ($1, $2, 'acme', 'Acme', $3)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(ORGANIZATION)
    .bind(uuid::Uuid::new_v4())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth.projects (external_id, organization_id, name, slug)
         VALUES ($1, $2, 'Agents', 'agents')",
    )
    .bind(PROJECT)
    .bind(ORGANIZATION)
    .execute(pool)
    .await
    .unwrap();
}

/// Move every code the owner was sent back past the wait between two.
async fn wait_out_the_resend(pool: &PgPool) {
    sqlx::query(
        "UPDATE auth.confirmation_codes
            SET created_at = created_at - make_interval(secs => $2::int)
          WHERE user_id = $1",
    )
    .bind(OWNER)
    .bind(confirmation_codes::RESEND_WAIT_SECS)
    .execute(pool)
    .await
    .unwrap();
}

/// A second code straight after the first is refused until the wait is out.
#[sqlx::test]
async fn a_second_code_waits_out_the_resend(pool: PgPool) {
    seed(&pool).await;
    let outbox = Arc::new(Outbox::default());
    let app = router(state(&pool, outbox.clone()));
    let ask = || async {
        let request = Request::builder()
            .method("POST")
            .uri("/internal/v1/me/deletion-code")
            .header("x-service-secret", SERVICE_SECRET);
        let request = as_person(request, &pool, OWNER)
            .await
            .body(Body::empty())
            .unwrap();
        app.clone().oneshot(request).await.unwrap()
    };
    assert_eq!(ask().await.status(), StatusCode::ACCEPTED);
    let refused = ask().await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after: i32 = refused.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (1..=confirmation_codes::RESEND_WAIT_SECS).contains(&retry_after),
        "{retry_after}"
    );
    assert_eq!(outbox.sent(), 1, "the press inside the wait mailed a code");

    wait_out_the_resend(&pool).await;
    assert_eq!(ask().await.status(), StatusCode::ACCEPTED);
    assert_eq!(outbox.sent(), 2);
}

/// ⚠ The code that takes a project's content mode off `off` is mailed to its
/// owner five times a day; the sixth asking is refused a day and mails nothing.
#[sqlx::test]
async fn a_content_mode_code_is_mailed_five_times_a_day(pool: PgPool) {
    seed(&pool).await;
    let outbox = Arc::new(Outbox::default());
    let state = state(&pool, outbox.clone());
    let acting = Acting {
        user_id: UserId::try_new(OWNER).unwrap(),
        organization_id: OrganizationId::try_new(ORGANIZATION).unwrap(),
        organization_role: Some(OrganizationRole::Owner),
        project: None,
        session_id: None,
        expires_at: i64::MAX,
    };
    let project = ProjectId::try_new(PROJECT).unwrap();
    for asked in 1..=5 {
        if let Err(refused) = state
            .mail_confirmation(&acting, Confirmation::ContentMode(&project))
            .await
        {
            panic!("asking {asked} was refused: {refused}");
        }
        wait_out_the_resend(&pool).await;
    }
    let refused = state
        .mail_confirmation(&acting, Confirmation::ContentMode(&project))
        .await
        .expect_err("the sixth asking of a day");
    assert!(
        matches!(
            refused,
            TelmoniError::Tenant(TenantError::RateLimited {
                retry_after_secs: 86_400
            })
        ),
        "{refused}"
    );
    assert_eq!(outbox.sent(), 5, "the sixth asking mailed a code");
}

/// The same five a day hold every code a lane mails: a sixth deletion code.
#[sqlx::test]
async fn a_sixth_deletion_code_of_a_day_is_refused(pool: PgPool) {
    seed(&pool).await;
    let outbox = Arc::new(Outbox::default());
    let app = router(state(&pool, outbox.clone()));
    for asked in 1..=6 {
        let request = Request::builder()
            .method("POST")
            .uri("/internal/v1/me/deletion-code")
            .header("x-service-secret", SERVICE_SECRET);
        let request = as_person(request, &pool, OWNER)
            .await
            .body(Body::empty())
            .unwrap();
        let status = app.clone().oneshot(request).await.unwrap().status();
        let wanted = if asked <= 5 {
            StatusCode::ACCEPTED
        } else {
            StatusCode::TOO_MANY_REQUESTS
        };
        assert_eq!(status, wanted, "asking {asked}");
        wait_out_the_resend(&pool).await;
    }
    assert_eq!(outbox.sent(), 5, "the sixth asking mailed a code");
}
